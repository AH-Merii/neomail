//! `neomail daemon` and the account locks it shares with `neomail sync`, end to end. The only
//! server is a closed port on 127.0.0.1, and only the children spawned here are signalled.
#![cfg(unix)]
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Long enough for a slow CI runner; only reached when something is wrong.
const PATIENCE: Duration = Duration::from_secs(60);

/// The binary with its data and config in `data`, no keyring and no network but 127.0.0.1.
fn command(data: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("BROWSER", "true")
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

/// Spawns with standard error piped and returns its lines as they come.
fn spawn(mut c: Command) -> (Child, Receiver<String>) {
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
    (child, lines)
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
            "neomail daemon: syncing 1 account every 30 seconds, {notifications}; Ctrl-C stops"
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
        "neomail daemon: syncing 1 account every 30 seconds, notifications off (--no-notify); \
         Ctrl-C stops"
    );
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
