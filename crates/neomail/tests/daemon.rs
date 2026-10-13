//! `neomail daemon` and the account locks it shares with `neomail sync` and `neomail mark`, end
//! to end. The only server is a closed port on 127.0.0.1, and only the children spawned here are
//! signalled.
#![cfg(unix)]
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::{BufRead, BufReader};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};

#[path = "support/fake_jev.rs"]
mod fake_jev;
use fake_jev::FakeJev;

/// Long enough for a slow CI runner; only reached when something is wrong.
const PATIENCE: Duration = Duration::from_secs(60);

/// The start line's words on classifying when there is no jev key.
const NO_KEY: &str = "classifying off: no jev key (run `neomail key set` or set JEV_API_KEY)";
/// The start line's words on classifying while the config does not switch it on.
const OFF: &str = "classifying off (`daemon = true` under [classify] in the config turns it on)";

/// The binary with its data and config in `data`, the debug build's file keyring there (empty,
/// so never the system's), no jev key and no network but 127.0.0.1.
fn command(data: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("BROWSER", "true")
        .env_remove("JEV_API_KEY")
        .env("NEOMAIL_TEST_KEYRING", data.join("test-keyring.json"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE");
    c
}

/// A config with one IMAP account, "work", on a port of 127.0.0.1 that nothing listens on.
fn closed_port_config(data: &Path) {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    std::fs::write(
        data.join("config.toml"),
        format!(
            "[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
             password_command = \"echo not-a-secret\"\n\
             imap = {{ host = \"127.0.0.1\", port = {port} }}\n"
        ),
    )
    .unwrap();
}

/// A spawned child, killed and reaped when it goes out of scope, so a test that fails half way
/// never leaves a daemon running.
struct Spawned(Child);

impl Deref for Spawned {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for Spawned {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        // Child::kill does nothing once the child has been waited for, so a reused pid is safe.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawns with standard error piped and returns its lines as they come.
fn spawn(mut c: Command) -> (Spawned, Receiver<String>) {
    let mut child = c
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    (Spawned(child), lines)
}

/// The next line of standard error, which must come within [`PATIENCE`].
fn next_line(child: &mut Child, lines: &Receiver<String>) -> String {
    lines.recv_timeout(PATIENCE).unwrap_or_else(|e| {
        let _ = child.kill();
        panic!("no line from neomail: {e}");
    })
}

/// SIGTERM to our own child, never to anything else.
fn terminate(child: &Child) {
    let pid = child.id();
    assert!(pid > 1, "not a child's pid: {pid}");
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn a_failing_daemon_reports_once_keeps_running_refuses_a_second_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    let (mut daemon, lines) = spawn(command(data, &["daemon", "--interval", "30s"]));
    // The session bus is a path that does not exist.
    let notifications = if cfg!(target_os = "linux") {
        "notifications off (no notification service)"
    } else {
        "notifications off (not supported here yet)"
    };
    assert_eq!(
        next_line(&mut daemon, &lines),
        format!(
            "neomail daemon: syncing 1 account every 30 seconds, {notifications}, {OFF}; \
             Ctrl-C stops"
        )
    );
    let failure = next_line(&mut daemon, &lines);
    // HH:MM:SS, then the line `neomail sync` prints for the account.
    let (time, report) = failure.split_once(' ').unwrap();
    assert!(
        time.len() == 8 && time.bytes().filter(|&b| b == b':').count() == 2,
        "{failure}"
    );
    assert!(report.starts_with("neomail: work: "), "{failure}");
    assert!(daemon.try_wait().unwrap().is_none(), "it keeps running");

    let second = command(data, &["daemon"]).output().unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&second.stderr),
        format!(
            "neomail: neomail daemon is already running for {}\n",
            data.display()
        )
    );

    terminate(&daemon);
    let status = daemon.wait().unwrap();
    assert_eq!(status.code(), Some(0), "{status:?}");
    let rest: Vec<String> = lines.iter().collect();
    assert!(rest.is_empty(), "the failure is reported once: {rest:?}");
    let mut stdout = String::new();
    std::io::Read::read_to_string(&mut daemon.stdout.take().unwrap(), &mut stdout).unwrap();
    assert_eq!(stdout, "", "nothing changed, so nothing is reported");
    assert!(
        data.join("daemon.lock").exists(),
        "a lock file is never deleted"
    );
}

#[test]
fn no_notify_says_so_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    let args = ["daemon", "--interval", "30s", "--no-notify"];
    let (mut daemon, lines) = spawn(command(data, &args));
    let start = next_line(&mut daemon, &lines);
    terminate(&daemon);
    daemon.wait().unwrap();
    assert_eq!(
        start,
        format!(
            "neomail daemon: syncing 1 account every 30 seconds, notifications off (--no-notify), \
             {OFF}; Ctrl-C stops"
        )
    );
}

/// The daemon's start line with `data`'s config, with a jev key when `key`.
fn start_line(data: &Path, key: bool) -> String {
    let mut c = command(data, &["daemon", "--interval", "30s", "--no-notify"]);
    if key {
        c.env("JEV_API_KEY", "made-up-key");
    }
    let (mut daemon, lines) = spawn(c);
    let start = next_line(&mut daemon, &lines);
    terminate(&daemon);
    daemon.wait().unwrap();
    start
}

/// Appends `text` to `data`'s config.
fn append_config(data: &Path, text: &str) {
    let config = std::fs::read_to_string(data.join("config.toml")).unwrap();
    std::fs::write(data.join("config.toml"), config + text).unwrap();
}

const HEAD: &str =
    "neomail daemon: syncing 1 account every 30 seconds, notifications off (--no-notify)";

#[test]
fn the_start_line_says_classifying_is_on_only_when_switched_on_with_a_key_and_the_consent() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    // The backend is a closed port too: nothing is sent, as there is no new mail.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    append_config(
        data,
        &format!("\n[classify]\nurl = \"http://127.0.0.1:{port}\"\n"),
    );
    // A key and a yes are not enough: the config has not switched it on.
    std::fs::write(data.join("classify-consent"), "127.0.0.1 daemon\n").unwrap();
    assert_eq!(
        start_line(data, true),
        format!("{HEAD}, {OFF}; Ctrl-C stops")
    );

    append_config(data, "daemon = true\n");
    let needs_consent = format!(
        "{HEAD}, classifying off: 127.0.0.1 needs your consent once; run `neomail classify` on \
         a terminal; Ctrl-C stops"
    );
    std::fs::remove_file(data.join("classify-consent")).unwrap();
    assert_eq!(start_line(data, true), needs_consent);
    std::fs::write(data.join("classify-consent"), "localhost\n").unwrap();
    assert_eq!(start_line(data, true), needs_consent);
    // Any yes `neomail classify` stored for the host: the host alone, as it stores it now, or
    // with `daemon`, as it stored it while its question named the daemon.
    for consent in ["127.0.0.1\n", "127.0.0.1 daemon\n"] {
        std::fs::write(data.join("classify-consent"), consent).unwrap();
        assert_eq!(
            start_line(data, true),
            format!("{HEAD}, classifying new mail with 127.0.0.1; Ctrl-C stops"),
            "{consent:?}"
        );
    }
    assert_eq!(
        start_line(data, false),
        format!("{HEAD}, {NO_KEY}; Ctrl-C stops")
    );
}

#[test]
fn a_yes_to_neomail_classify_does_not_switch_the_daemon_on() {
    let jev = FakeJev::start();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    append_config(data, &format!("\n[classify]\nurl = \"{}\"\n", jev.url()));
    let mut cache = Cache::open(&data.join("cache.sqlite3")).unwrap();
    let inbox = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let raw = "Message-ID: <m1@acme.example>\r\nFrom: orders@shop.example\r\n\
               To: you@example.com\r\nSubject: Your order\r\n\r\nShipped.\r\n";
    let message = FetchedMessage {
        uid: Uid::new(1).unwrap(),
        flags: Flags::new(Vec::<String>::new()),
        received: Some(chrono::Utc::now()),
        raw: raw.as_bytes().to_vec(),
    };
    cache.insert_messages(inbox.id, &[message]).unwrap();
    drop(cache);
    let o = command(data, &["classify", "--yes"])
        .env("JEV_API_KEY", "made-up-key")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(jev.count(), 1);
    assert_eq!(
        start_line(data, true),
        format!("{HEAD}, {OFF}; Ctrl-C stops")
    );
    assert_eq!(jev.count(), 1);
}

#[test]
fn sync_waits_for_an_account_the_daemon_is_syncing_then_runs() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    // Held as the daemon holds it while it syncs "work".
    std::fs::create_dir(data.join("locks")).unwrap();
    let lock = std::fs::File::create(data.join("locks/work.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();

    let (mut sync, lines) = spawn(command(data, &["sync"]));
    assert_eq!(
        next_line(&mut sync, &lines),
        "neomail: waiting for neomail daemon to finish syncing work"
    );
    assert!(sync.try_wait().unwrap().is_none(), "it waits");
    drop(lock);
    let status = sync.wait().unwrap();
    let rest: Vec<String> = lines.iter().collect();
    // It ran: the closed port is the failure, reported as `sync` always does.
    assert_eq!(status.code(), Some(1), "{rest:?}");
    assert_eq!(rest.len(), 1, "{rest:?}");
    assert!(rest[0].starts_with("neomail: work: "), "{rest:?}");
}

/// `mark` takes the account's lock as `sync` does: it says so once, waits, then runs.
#[test]
fn mark_waits_for_an_account_the_daemon_is_syncing_then_runs() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    closed_port_config(data);
    let id = {
        let mut cache = nm_core::Cache::open(&data.join("cache.sqlite3")).unwrap();
        let mailbox = cache
            .reset_mailbox("work", "INBOX", nm_core::UidValidity::new(1).unwrap())
            .unwrap();
        let message = nm_core::FetchedMessage {
            uid: nm_core::Uid::new(1).unwrap(),
            flags: nm_core::Flags::default(),
            received: None,
            raw: b"Subject: hi\r\n\r\nBody\r\n".to_vec(),
        };
        cache.insert_messages(mailbox.id, &[message]).unwrap();
        cache.list(None, 1).unwrap()[0].id.to_string()
    };
    std::fs::create_dir(data.join("locks")).unwrap();
    let lock = std::fs::File::create(data.join("locks/work.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();

    let (mut mark, lines) = spawn(command(data, &["mark", "read", &id]));
    assert_eq!(
        next_line(&mut mark, &lines),
        "neomail: waiting for neomail daemon to finish syncing work"
    );
    assert!(mark.try_wait().unwrap().is_none(), "it waits");
    drop(lock);
    let status = mark.wait().unwrap();
    let rest: Vec<String> = lines.iter().collect();
    // It ran: the closed port is the failure.
    assert_eq!(status.code(), Some(1), "{rest:?}");
    assert_eq!(rest.len(), 1, "{rest:?}");
    assert!(
        rest[0].starts_with(&format!(
            "neomail: work: message {id} may not be marked read: "
        )),
        "{rest:?}"
    );
}

#[test]
fn without_accounts_or_with_a_bad_interval_the_daemon_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let start = "neomail: no accounts yet. Start with:\n  neomail account add\n  \
                 neomail sync && neomail list\nGuide: https://github.com/AH-Merii/neomail#try-it\n";
    let o = command(data, &["daemon"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&o.stderr), start);

    std::fs::write(data.join("config.toml"), "# nothing yet\n").unwrap();
    let o = command(data, &["daemon"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&o.stderr), start);

    std::fs::write(data.join("config.toml"), "[[accounts]\n").unwrap();
    let o = command(data, &["daemon"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "an invalid config");

    closed_port_config(data);
    for bad in ["5", "10s", "1d", "-1m"] {
        let flag = format!("--interval={bad}");
        let o = command(data, &["daemon", &flag]).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("--interval"),
            "{bad}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    assert!(!data.join("daemon.lock").exists(), "nothing started");
}

#[test]
fn the_help_names_the_switch_both_key_sources_the_cap_per_sync_and_the_categories() {
    let dir = tempfile::tempdir().unwrap();
    let o = command(dir.path(), &["daemon", "--help"]).output().unwrap();
    assert!(o.status.success());
    let help = String::from_utf8(o.stdout).unwrap();
    let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
    for part in [
        "Classifying in the background is off until `daemon = true` under [classify] in the \
         config switches it on",
        "While it is off, the daemon sends nothing to the classifier and writes no groups",
        "there is a key (`neomail key set` or JEV_API_KEY)",
        "After classifying, it writes each Outlook message's group as a category; \
         `classify.labels = false` turns that off",
        "at most 100 messages each time it syncs (a round, a push sync or an Outlook check); \
         the rest wait for the next time it syncs",
    ] {
        assert!(flat.contains(part), "{part}: {help}");
    }
}
