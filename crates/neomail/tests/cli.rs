//! `neomail list`, `show` and the offline failure paths of `sync`, against a temporary data
//! directory. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use chrono::DateTime;
use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::Value;

const LONG_SUBJECT: &str =
    "A rather long subject line that will certainly not fit in eighty columns at all";

fn message(uid: u32, received: i64, from: &str, subject: &str, seen: bool) -> FetchedMessage {
    let raw = format!(
        "From: {from}\r\nTo: me@mail.example\r\nSubject: {subject}\r\n\
         Date: Sat, 10 Oct 2026 09:30:00 +0000\r\n\r\nHello from {uid}.\r\n"
    );
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::new(if seen { vec![Flags::SEEN] } else { vec![] }),
        received: DateTime::from_timestamp(received, 0),
        raw: raw.into_bytes(),
    }
}

/// A data directory with three messages in "work" and one in "home".
fn seeded() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let work = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    cache
        .insert_messages(
            work.id,
            &[
                message(1, 100, "Ada Lovelace <ada@mail.example>", "Oldest", true),
                message(2, 300, "bob@mail.example", LONG_SUBJECT, false),
                message(
                    3,
                    200,
                    "Carol <carol@mail.example>",
                    "Escape \x1b[31m here",
                    false,
                ),
            ],
        )
        .unwrap();
    let home = cache
        .reset_mailbox("home", "INBOX", UidValidity::new(9).unwrap())
        .unwrap();
    cache
        .insert_messages(home.id, &[message(5, 50, "dan@mail.example", "Home", true)])
        .unwrap();
    dir
}

/// The binary with its data and config in `data`, no keyring, and none of the caller's colour
/// or pager settings.
fn command(data: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args);
    isolate(&mut c, data);
    c
}

/// Gives `c` the environment `command` gives the binary.
fn isolate(c: &mut Command, data: &Path) {
    c.env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("TERM", "xterm-256color");
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "NEOMAIL_NO_INPUT",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "PAGER",
        "NEOMAIL_PAGER",
        "LESS",
    ] {
        c.env_remove(var);
    }
}

fn neomail(data: &Path, args: &[&str]) -> Output {
    command(data, args).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn json(o: &Output) -> Value {
    assert!(o.status.success(), "{}", stderr(o));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn list_off_a_terminal_is_tsv_newest_first() {
    let dir = seeded();
    let o = neomail(dir.path(), &["list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 4, "{out}");
    for row in &rows {
        assert_eq!(row.len(), 6, "{row:?}");
    }
    // Nothing is truncated, and the escape in Carol's subject became a space.
    assert_eq!(
        rows[0][1..],
        [
            "*",
            "2026-10-10T09:30:00Z",
            "work",
            "bob@mail.example",
            LONG_SUBJECT
        ]
    );
    assert_eq!(
        rows[1][4..],
        ["Carol <carol@mail.example>", "Escape  [31m here"]
    );
    assert_eq!(rows[2][1], "");
    assert_eq!(rows[3][3..], ["home", "dan@mail.example", "Home"]);
    // The ids are the JSON ids.
    let v = json(&neomail(dir.path(), &["list", "--json"]));
    for (row, item) in rows.iter().zip(v.as_array().unwrap()) {
        assert_eq!(row[0], item["id"].to_string());
        assert_eq!(row[2], item["date"].as_str().unwrap());
    }
    assert_eq!(stdout(&neomail(dir.path(), &["list", "--plain"])), out);

    let o = neomail(dir.path(), &["list", "--account", "work", "--limit", "2"]);
    assert_eq!(stdout(&o).lines().count(), 2);
    let o = neomail(dir.path(), &["list", "--limit", "all"]);
    assert_eq!(stdout(&o), out);
    let o = neomail(dir.path(), &["list", "--limit", "1"]);
    assert_eq!(stdout(&o).lines().count(), 1);
    for bad in ["0", "-3", "every"] {
        let o = neomail(dir.path(), &["list", "--limit", bad]);
        assert_eq!(o.status.code(), Some(2), "{bad}");
    }
    let o = neomail(dir.path(), &["sync", "--limit", "all"]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "parsed, then fails for want of a config"
    );
}

#[test]
fn output_flags_conflict() {
    let dir = seeded();
    for args in [
        &["list", "--plain", "--json"][..],
        &["list", "--json", "--jsonl"],
        &["list", "--plain", "--jsonl"],
        &["account", "list", "--json", "--plain"],
        &["show", "1", "--json", "--raw"],
        &["show", "1", "--jsonl", "--json"],
    ] {
        let o = neomail(dir.path(), args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(o.stdout.is_empty(), "{args:?}");
    }
}

/// `ARGS --jsonl` prints one line per element of `ARGS --json`'s array.
fn assert_jsonl_matches_json(data: &Path, args: &[&str]) {
    let array = json(&neomail(data, &[args, &["--json"]].concat()));
    let o = neomail(data, &[args, &["--jsonl"]].concat());
    assert!(o.status.success(), "{}", stderr(&o));
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!lines.is_empty());
    assert_eq!(Value::Array(lines), array, "{args:?}");
}

#[test]
fn list_json_shape() {
    let dir = seeded();
    let v = json(&neomail(
        dir.path(),
        &["list", "--json", "--account", "work"],
    ));
    let items = v.as_array().unwrap();
    assert_eq!(items.len(), 3);
    let first = &items[0];
    assert_eq!(first["account"], "work");
    assert_eq!(first["uid"], 2);
    assert_eq!(first["unread"], true);
    assert_eq!(first["subject"], LONG_SUBJECT);
    assert_eq!(first["from"]["address"], "bob@mail.example");
    assert_eq!(first["from"]["name"], Value::Null);
    assert_eq!(first["date"], "2026-10-10T09:30:00Z");
    assert_eq!(items[2]["flags"], serde_json::json!(["\\Seen"]));
    assert!(first["id"].is_i64());
}

#[test]
fn show_human_json_and_raw() {
    let dir = seeded();
    let id = json(&neomail(dir.path(), &["list", "--json"]))[2]["id"]
        .as_i64()
        .unwrap()
        .to_string();

    let o = neomail(dir.path(), &["show", &id]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("From:    Ada Lovelace <ada@mail.example>"),
        "{out}"
    );
    assert!(out.contains("To:      me@mail.example"), "{out}");
    assert!(out.contains("Subject: Oldest"), "{out}");
    assert!(out.contains("Hello from 1."), "{out}");

    let v = json(&neomail(dir.path(), &["show", &id, "--json"]));
    assert_eq!(v["body_type"], "text/plain");
    assert_eq!(v["from"][0]["name"], "Ada Lovelace");
    assert_eq!(v["attachments"], serde_json::json!([]));
    assert_eq!(v["unread"], false);

    let o = neomail(dir.path(), &["show", &id, "--raw"]);
    assert!(o.stdout.starts_with(b"From: Ada Lovelace"));
}

#[cfg(unix)]
#[test]
fn cache_files_are_made_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = seeded();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let cache = dir.path().join("cache.sqlite3");
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o644)).unwrap();
    // An open connection keeps the -wal and -shm files, which SQLite creates with the cache
    // file's mode, from being removed when neomail exits.
    let _open = Cache::open(&cache).unwrap();
    let mode = |file: &str| {
        let meta = std::fs::metadata(dir.path().join(file)).unwrap();
        meta.permissions().mode() & 0o777
    };
    assert_eq!(mode("cache.sqlite3-wal"), 0o644);
    assert_eq!(mode("cache.sqlite3-shm"), 0o644);

    assert!(neomail(dir.path(), &["list"]).status.success());
    for file in ["cache.sqlite3", "cache.sqlite3-wal", "cache.sqlite3-shm"] {
        assert_eq!(mode(file), 0o600, "{file}");
    }
}

#[test]
fn config_errors_give_the_place_but_not_the_text() {
    let dir = tempfile::tempdir().unwrap();
    // A secret pasted without quotes: a syntax error on line 3.
    std::fs::write(
        dir.path().join("config.toml"),
        "[[accounts]]\nname = \"work\"\npassword = s3cret-fake-value\n",
    )
    .unwrap();
    let o = neomail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(all.contains("line 3, column"), "{all}");
    assert!(!all.contains("s3cret"), "{all}");
    assert_eq!(all.lines().count(), 1, "{all}");
}

#[test]
fn empty_neomail_config_counts_as_unset() {
    let dir = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_neomail"))
        .arg("sync")
        .env("NEOMAIL_DATA_DIR", dir.path())
        .env("NEOMAIL_CONFIG", "")
        .env("HOME", dir.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    // Not a usage error (2): the default path is used, and it holds no config.
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.contains("no config file at"), "{err}");
    assert!(err.contains(".config/neomail/config.toml"), "{err}");
}

/// install.sh tells our binary from d99kris/nmail and others by this prefix.
#[test]
fn version_names_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail(dir.path(), &["--version"]);
    assert!(o.status.success());
    assert_eq!(
        stdout(&o),
        format!("neomail {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn failures_are_clear_and_non_zero() {
    let dir = seeded();
    let o = neomail(dir.path(), &["show", "999"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("no cached message with id 999"),
        "{}",
        stderr(&o)
    );

    assert_eq!(neomail(dir.path(), &["show", "abc"]).status.code(), Some(2));
    assert_eq!(
        neomail(dir.path(), &["list", "--limit", "0"]).status.code(),
        Some(2)
    );

    let o = neomail(dir.path(), &["list", "--account", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("home, work"), "{}", stderr(&o));

    let o = neomail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no config file"), "{}", stderr(&o));
}

const ADD_HINT: &str = "no accounts yet; add one with `neomail account add`";

#[test]
fn without_a_config_list_show_and_sync_name_the_next_command() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail(dir.path(), &["list"]);
    assert!(o.status.success());
    assert!(stdout(&o).is_empty());
    assert_eq!(stderr(&o), format!("neomail: {ADD_HINT}\n"));
    assert_eq!(
        json(&neomail(dir.path(), &["list", "--json"])),
        serde_json::json!([])
    );
    for args in [&["show", "1"][..], &["list", "--account", "work"]] {
        let o = neomail(dir.path(), args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert_eq!(stderr(&o), format!("neomail: {ADD_HINT}\n"), "{args:?}");
    }
    let o = neomail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.contains("no config file at "), "{err}");
    assert!(err.contains(ADD_HINT), "{err}");
    assert!(
        err.contains("https://github.com/AH-Merii/neomail#try-it"),
        "{err}"
    );
    assert!(
        !dir.path().join("cache.sqlite3").exists(),
        "list must not create the cache"
    );

    // With a config but nothing cached, the next command is sync.
    std::fs::write(dir.path().join("config.toml"), KEYRING_CONFIG).unwrap();
    let o = neomail(dir.path(), &["list"]);
    assert!(o.status.success());
    assert!(stderr(&o).contains("run `neomail sync`"), "{}", stderr(&o));
    let o = neomail(dir.path(), &["show", "1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("run `neomail sync`"), "{}", stderr(&o));
}

#[test]
fn bare_neomail_prints_the_start_without_a_config_and_help_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail_stdin(dir.path(), &[], "");
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).is_empty());
    assert_eq!(
        stderr(&o),
        "neomail: no accounts yet. Start with:\n  neomail account add\n  \
         neomail sync && neomail list\nGuide: https://github.com/AH-Merii/neomail#try-it\n"
    );
    std::fs::write(dir.path().join("config.toml"), KEYRING_CONFIG).unwrap();
    let o = neomail_stdin(dir.path(), &[], "");
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(err.contains("Usage: neomail"), "{err}");
    assert!(err.contains("Examples:"), "{err}");
    assert!(!err.contains("docs/"), "help links are absolute: {err}");
}

#[test]
fn password_command_output_never_appears_in_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[[accounts]]\nname = \"work\"\nusername = \"user@mail.example\"\n\
         password_command = \"echo hunter2; exit 1\"\nimap = { host = \"imap.mail.example\" }\n",
    )
    .unwrap();
    let o = neomail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(all.contains("password_command failed"), "{all}");
    assert!(!all.contains("hunter2"), "{all}");
    // Off a terminal, sync shows no progress line: stderr holds only the error.
    assert_eq!(stderr(&o).lines().count(), 1, "{all}");
    assert!(!all.contains('\r') && !all.contains("fetching"), "{all:?}");
}

const KEYRING_CONFIG: &str = "[[accounts]]\nname = \"work\"\nusername = \"user@mail.example\"\n\
                              imap = { host = \"imap.mail.example\" }\n";
const FAKE_PASSWORD: &str = "s3cret-fake-password";

/// Runs `neomail` with `input` on standard input and a D-Bus session bus that does not exist, so
/// nothing reaches a real keyring.
fn neomail_stdin(data: &Path, args: &[&str], input: &str) -> Output {
    let mut child = command(data, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // neomail may exit before it reads, closing the pipe.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn set_password_and_delete_password_check_the_account() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), KEYRING_CONFIG).unwrap();

    let input = format!("{FAKE_PASSWORD}\n");
    let o = neomail_stdin(dir.path(), &["account", "set-password", "nope"], &input);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        all.contains("no account named \"nope\"; configured: work"),
        "{all}"
    );
    assert!(!all.contains(FAKE_PASSWORD), "{all}");

    let o = neomail_stdin(dir.path(), &["account", "delete-password", "nope"], "");
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("configured: work"), "{}", stderr(&o));
}

/// No Secret Service on the bus: every keyring command fails in one line with the hint, and the
/// password given on stdin is never echoed. Linux only: on macOS this would reach the keychain.
#[cfg(target_os = "linux")]
#[test]
fn keyring_unavailable_is_one_line_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), KEYRING_CONFIG).unwrap();
    let input = format!("{FAKE_PASSWORD}\n");
    for args in [
        &["account", "set-password", "work"][..],
        &["account", "delete-password", "work"],
        &["sync"],
    ] {
        let o = neomail_stdin(dir.path(), args, &input);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let (out, err) = (stdout(&o), stderr(&o));
        assert!(out.is_empty(), "{args:?}: {out}");
        assert!(err.contains("the OS keyring is unavailable ("), "{err}");
        assert!(
            err.contains("set password_command in the config instead"),
            "{err}"
        );
        assert_eq!(err.lines().count(), 1, "{err}");
        assert!(!err.contains(FAKE_PASSWORD), "{err}");
    }
    // The keyring is checked before the password is read: with nothing on standard input, the
    // error is the keyring, not the empty password.
    let o = neomail_stdin(dir.path(), &["account", "set-password", "work"], "");
    let err = stderr(&o);
    assert!(err.contains("the OS keyring is unavailable ("), "{err}");
    assert!(!err.contains("the password is empty"), "{err}");
}

const GMAIL_CONFIG: &str = "[[accounts]]\nname = \"gmail\"\nusername = \"you@example.com\"\n\
                            oauth = { provider = \"google\", client_id = \"1234-abc.apps.googleusercontent.com\" }\n";

#[test]
fn add_off_a_terminal_names_the_missing_flags() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail_stdin(dir.path(), &["account", "add", "gmail", "--no-browser"], "");
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(err.contains("not a terminal"), "{err}");
    assert!(err.contains("pass --email and --client-id"), "{err}");
    let o = neomail_stdin(
        dir.path(),
        &[
            "account",
            "add",
            "gmail",
            "--email",
            "you@example.com",
            "--no-browser",
        ],
        "",
    );
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("pass --client-id ("), "{}", stderr(&o));
    assert!(!dir.path().join("config.toml").exists());
}

#[test]
fn password_and_oauth_commands_refuse_the_other_kind_of_account() {
    let dir = tempfile::tempdir().unwrap();
    let config = format!("{GMAIL_CONFIG}{KEYRING_CONFIG}");
    std::fs::write(dir.path().join("config.toml"), config).unwrap();
    let input = format!("{FAKE_PASSWORD}\n");
    for (args, hint) in [
        (
            &["account", "set-password", "gmail"][..],
            "use `neomail account login gmail`",
        ),
        (
            &["account", "delete-password", "gmail"],
            "use `neomail account logout gmail`",
        ),
        (
            &["account", "login", "work", "--no-browser"],
            "use `neomail account set-password work`",
        ),
        (
            &["account", "logout", "work"],
            "use `neomail account delete-password work`",
        ),
    ] {
        let o = neomail_stdin(dir.path(), args, &input);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let err = stderr(&o);
        assert!(err.contains(hint), "{args:?}: {err}");
        assert!(!err.contains(FAKE_PASSWORD), "{err}");
    }
}

/// No Secret Service on the bus: every OAuth command fails in one line before any sign-in
/// starts (no URL is printed, so no browser could open). Linux only, like the test above.
#[cfg(target_os = "linux")]
#[test]
fn oauth_commands_without_a_keyring_fail_before_signing_in() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), GMAIL_CONFIG).unwrap();
    let input = format!("{FAKE_PASSWORD}\n");
    for args in [
        &["account", "logout", "gmail"][..],
        &["account", "login", "gmail", "--no-browser"],
        &["sync"],
        &[
            "account",
            "add",
            "gmail",
            "--name",
            "other",
            "--email",
            "you@example.com",
            "--client-id",
            "1234-abc.apps.googleusercontent.com",
            "--no-browser",
        ],
    ] {
        let o = neomail_stdin(dir.path(), args, &input);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let (out, err) = (stdout(&o), stderr(&o));
        assert!(out.is_empty(), "{args:?}: {out}");
        assert!(err.contains("the OS keyring is unavailable ("), "{err}");
        assert!(err.contains("OAuth sign-ins are kept only there"), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
        assert!(!err.contains(FAKE_PASSWORD), "{err}");
        assert!(!err.contains("accounts.google.com"), "{err}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
        GMAIL_CONFIG
    );
}

#[test]
fn add_without_a_provider_off_a_terminal_lists_the_providers() {
    let dir = tempfile::tempdir().unwrap();
    for (args, why) in [
        (&["account", "add"][..], "standard input is not a terminal"),
        (&["account", "add", "--no-input"], "--no-input"),
    ] {
        let o = neomail_stdin(dir.path(), args, "");
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let err = stderr(&o);
        assert!(err.starts_with(&format!("neomail: {why}")), "{err}");
        assert!(
            err.contains("one of gmail, gmail-app-password, proton, imap"),
            "{err}"
        );
        assert!(!err.contains("Which kind of account?"), "no menu: {err}");
    }
    assert!(!dir.path().join("config.toml").exists());
}

const PIN: &str = "AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:\
                   AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD:AB:CD";

#[test]
fn no_input_and_missing_values_are_usage_errors_that_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let proton = ["account", "add", "proton", "--username", "you@example.com"];
    let o = neomail_stdin(dir.path(), &proton, &format!("{FAKE_PASSWORD}\n"));
    assert_eq!(o.status.code(), Some(2));
    let err = stderr(&o);
    assert!(err.contains("pass --cert-sha256"), "{err}");
    assert!(!err.contains(FAKE_PASSWORD), "{err}");

    let imap = ["account", "add", "imap", "--no-input", "--name", "work"];
    let o = neomail_stdin(dir.path(), &imap, "");
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(
        stderr(&o),
        "neomail: --no-input (or NEOMAIL_NO_INPUT) forbids prompts: pass --username and --host\n"
    );
    // NEOMAIL_NO_INPUT=1 is the same as --no-input.
    let mut child = Command::new(env!("CARGO_BIN_EXE_neomail"))
        .args(["account", "add", "imap", "--name", "work"])
        .env("NEOMAIL_NO_INPUT", "1")
        .env("NEOMAIL_CONFIG", dir.path().join("config.toml"))
        .env("HOME", dir.path())
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("forbids prompts"), "{}", stderr(&o));

    let o = neomail_stdin(dir.path(), &["account", "add", "proton", "--host", "h"], "");
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("--host does not apply to `neomail account add proton`"),
        "{}",
        stderr(&o)
    );
    assert!(!dir.path().join("config.toml").exists());
}

#[test]
fn add_proton_without_a_check_then_list_and_check_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail_stdin(
        dir.path(),
        &[
            "account",
            "add",
            "proton",
            "--username",
            "you@example.com",
            "--cert-sha256",
            PIN,
            "--password-command",
            "pass show bridge",
            "--no-check",
        ],
        "",
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("Next: neomail sync --account proton"),
        "{}",
        stdout(&o)
    );
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "{}\n{KEYRING_CONFIG}\n{GMAIL_CONFIG}",
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap()
        ),
    )
    .unwrap();

    // Off a terminal: TSV, name first, `pinned` or empty.
    let o = neomail_stdin(dir.path(), &["account", "list"], "");
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "proton\tpassword_command\tyou@example.com\t127.0.0.1\t1143\tstarttls\tpinned\n\
         work\tpassword\tuser@mail.example\timap.mail.example\t993\ttls\t\n\
         gmail\toauth\tyou@example.com\timap.gmail.com\t993\ttls\t\n"
    );
    let v = json(&neomail(dir.path(), &["account", "list", "--json"]));
    assert_eq!(
        v[0],
        serde_json::json!({
            "name": "proton", "login": "password_command", "username": "you@example.com",
            "host": "127.0.0.1", "port": 1143, "security": "starttls", "cert_pinned": true
        })
    );
    assert_jsonl_matches_json(dir.path(), &["account", "list"]);

    let o = neomail_stdin(dir.path(), &["config", "check"], "");
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), "ok: 3 accounts\n");
}

#[test]
fn config_path_and_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let o = neomail(dir.path(), &["config", "path"]);
    assert!(o.status.success());
    assert_eq!(stdout(&o), format!("{}\n", path.display()));
    let other = dir.path().join("other.toml");
    let o = neomail(
        dir.path(),
        &["config", "path", "--config", other.to_str().unwrap()],
    );
    assert_eq!(stdout(&o), format!("{}\n", other.display()));

    let o = neomail(dir.path(), &["config", "check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no config file at"), "{}", stderr(&o));
    let o = neomail(dir.path(), &["account", "list"]);
    assert!(o.status.success());
    assert!(stdout(&o).is_empty());
    assert_eq!(
        stderr(&o),
        "neomail: no accounts yet; add one with `neomail account add`\n"
    );
    let o = neomail(dir.path(), &["account", "list", "--json"]);
    assert_eq!(stdout(&o), "[]\n");

    // A secret pasted without quotes on line 3.
    std::fs::write(
        &path,
        "[[accounts]]\nname = \"work\"\npassword = s3cret-fake-value\n",
    )
    .unwrap();
    let o = neomail(dir.path(), &["config", "check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).is_empty());
    let err = stderr(&o);
    assert!(
        err.starts_with(&format!("{}:3:12: ", path.display())),
        "{err}"
    );
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(!err.contains("s3cret"), "{err}");

    std::fs::write(&path, KEYRING_CONFIG).unwrap();
    let o = neomail(dir.path(), &["config", "check"]);
    assert_eq!(stdout(&o), "ok: 1 account\n");
}

/// The ids of `list --json`, newest first: bob's, Carol's, Ada's, then Home.
fn ids(data: &Path) -> Vec<String> {
    json(&neomail(data, &["list", "--json"]))
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].to_string())
        .collect()
}

#[test]
fn list_jsonl_lines_are_the_json_elements() {
    let dir = seeded();
    assert_jsonl_matches_json(dir.path(), &["list"]);
    assert_jsonl_matches_json(dir.path(), &["list", "--account", "work", "--limit", "all"]);
}

#[test]
fn show_several_ids() {
    let dir = seeded();
    let ids = ids(dir.path());
    let o = neomail(dir.path(), &["show", &ids[2], &ids[3]]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let first = out.find("Subject: Oldest").unwrap();
    let separator = out.find(&format!("── message {} ──", ids[3])).unwrap();
    let second = out.find("Subject: Home").unwrap();
    assert!(first < separator && separator < second, "{out}");
    assert_eq!(out.matches("── message").count(), 1, "{out}");

    // JSON: one literal id is an object, several are an array, JSONL is one line each.
    let v = json(&neomail(dir.path(), &["show", &ids[2], "--json"]));
    assert_eq!(v["subject"], "Oldest");
    let v = json(&neomail(dir.path(), &["show", &ids[2], &ids[3], "--json"]));
    assert_eq!(v[0]["subject"], "Oldest");
    assert_eq!(v[1]["subject"], "Home");
    let o = neomail(dir.path(), &["show", &ids[2], &ids[3], "--jsonl"]);
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(Value::Array(lines), v);
    let o = neomail(dir.path(), &["show", &ids[2], "--jsonl"]);
    assert_eq!(stdout(&o).lines().count(), 1);

    // --raw takes exactly one id.
    let o = neomail(dir.path(), &["show", &ids[2], &ids[3], "--raw"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    assert!(
        stderr(&o).contains("--raw takes exactly one id"),
        "{}",
        stderr(&o)
    );
    let o = neomail_stdin(
        dir.path(),
        &["show", "-", "--raw"],
        &format!("{}\n", ids[2]),
    );
    assert!(o.stdout.starts_with(b"From: Ada Lovelace"));
}

#[test]
fn show_reports_missing_ids_after_showing_the_rest() {
    let dir = seeded();
    let ids = ids(dir.path());
    let o = neomail(dir.path(), &["show", "998", &ids[2], "999"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("Subject: Oldest"), "{}", stdout(&o));
    assert_eq!(
        stderr(&o),
        "neomail: no cached message with id 998; see `neomail list`\n\
         neomail: no cached message with id 999; see `neomail list`\n"
    );
    let o = neomail(dir.path(), &["show", "999", &ids[2], "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    // One literal missing id prints nothing on stdout.
    let o = neomail(dir.path(), &["show", "999", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
}

/// On a terminal stdout and stderr interleave: the missing-id report must come after the
/// messages, or several messages (or the pager) push it off screen.
#[test]
fn show_reports_missing_ids_after_the_output_on_a_shared_stream() {
    let dir = seeded();
    let ids = ids(dir.path());
    let (mut reader, writer) = std::io::pipe().unwrap();
    let mut c = command(dir.path(), &["show", &ids[2], "999"]);
    c.stdout(writer.try_clone().unwrap()).stderr(writer);
    let mut child = c.spawn().unwrap();
    // Drop the parent's write ends, or the read below never sees end of file.
    drop(c);
    let mut merged = String::new();
    std::io::Read::read_to_string(&mut reader, &mut merged).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(1));
    let message = merged.find("Subject: Oldest").expect(&merged);
    let error = merged
        .find("neomail: no cached message with id 999")
        .expect(&merged);
    assert!(message < error, "{merged}");
}

#[test]
fn show_reads_ids_from_standard_input() {
    let dir = seeded();
    let ids = ids(dir.path());
    // TSV lines, as `neomail list | fzf -m` passes them, with a blank line.
    let tsv = stdout(&neomail(dir.path(), &["list"]));
    let picked: String = tsv.lines().skip(2).map(|l| format!("{l}\n\n")).collect();
    let o = neomail_stdin(dir.path(), &["show", "-", "--json"], &picked);
    let v = json(&o);
    let subjects: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["Oldest", "Home"]);

    // Coloured lines, as `neomail list --color always | fzf --ansi -m` passes them.
    let coloured = stdout(&neomail(dir.path(), &["list", "--color", "always"]));
    assert!(coloured.contains('\x1b'), "{coloured:?}");
    let o = neomail_stdin(dir.path(), &["show", "-", "--jsonl"], &coloured);
    assert!(o.status.success(), "{}", stderr(&o));
    let got: Vec<String> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["id"].to_string())
        .collect();
    assert_eq!(got, ids);

    // One id from stdin is still an array: the shape never depends on the count.
    let o = neomail_stdin(
        dir.path(),
        &["show", "-", "--json"],
        &format!("{}\n", ids[0]),
    );
    assert!(json(&o).is_array());

    // A line without an id is a usage error naming it, and nothing is shown.
    let o = neomail_stdin(
        dir.path(),
        &["show", "-"],
        &format!("{}\n\nnot an id\n", ids[0]),
    );
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    assert!(
        stderr(&o).contains("standard input line 3"),
        "{}",
        stderr(&o)
    );
    let o = neomail_stdin(dir.path(), &["show", "-", &ids[0]], "");
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn color_follows_the_flag_and_the_environment() {
    let dir = seeded();
    let id = &ids(dir.path())[2];
    // Off a terminal: no colour unless forced.
    let plain = stdout(&neomail(dir.path(), &["list"]));
    assert!(!plain.contains('\x1b'));
    let forced = command(dir.path(), &["list"])
        .env("CLICOLOR_FORCE", "1")
        .output()
        .unwrap();
    assert!(stdout(&forced).contains("\x1b[33m"), "{}", stdout(&forced));
    let never = command(dir.path(), &["list", "--color", "never"])
        .env("CLICOLOR_FORCE", "1")
        .output()
        .unwrap();
    assert_eq!(stdout(&never), plain);
    // NO_COLOR removes all styling, bold too.
    let o = command(dir.path(), &["show", id])
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(!o.stdout.contains(&0x1b), "{}", stdout(&o));
    let o = neomail(dir.path(), &["show", id, "--color", "always"]);
    assert!(
        stdout(&o).contains("\x1b[1mSubject:\x1b[0m"),
        "{}",
        stdout(&o)
    );
    // Never in JSON, JSONL or raw output.
    for args in [
        &["list", "--json"][..],
        &["list", "--jsonl"],
        &["account", "list", "--json"],
        &["show", id, "--json"],
        &["show", id, "--jsonl"],
        &["show", id, "--raw"],
    ] {
        let o = command(dir.path(), &[args, &["--color", "always"]].concat())
            .env("CLICOLOR_FORCE", "1")
            .output()
            .unwrap();
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert!(!o.stdout.contains(&0x1b), "{args:?}: {}", stdout(&o));
    }
    // Usage errors and help follow the choice too.
    let o = neomail(dir.path(), &["list", "--bogus", "--color", "always"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains('\x1b'), "{}", stderr(&o));
    let o = neomail(dir.path(), &["list", "--bogus"]);
    assert!(!stderr(&o).contains('\x1b'), "{}", stderr(&o));
    let o = command(dir.path(), &["--help", "--color", "always"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(stdout(&o).contains('\x1b'));
    let o = command(dir.path(), &["--help"])
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(!stdout(&o).contains('\x1b'));
}

#[test]
fn show_never_pages_off_a_terminal() {
    let dir = seeded();
    let id = &ids(dir.path())[2];
    let marker = dir.path().join("paged");
    let pager = format!("touch '{}'; cat", marker.display());
    let o = command(dir.path(), &["show", id])
        .env("NEOMAIL_PAGER", &pager)
        .env("PAGER", &pager)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("Subject: Oldest"));
    assert!(!marker.exists(), "the pager ran off a terminal");
}

/// A pager that catches Ctrl-C as less does, then reports whether neomail, its parent, was still
/// alive half a second after the Ctrl-C.
#[cfg(target_os = "linux")]
const PAGER_CATCHING_CTRL_C: &str = r#"trap 'touch "DIR/got-int"' INT
cat >/dev/null
touch "DIR/ready"
i=0
while [ ! -e "DIR/got-int" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i + 1)); done
sleep 0.5
read -r _ _ _ ppid _ < /proc/$$/stat
if [ "$ppid" = "$PPID" ]; then echo alive; else echo orphaned; fi > "DIR/result"
"#;

/// A pager that dies of Ctrl-C.
#[cfg(target_os = "linux")]
const PAGER_DYING_OF_CTRL_C: &str = r#"cat >/dev/null
touch "DIR/ready"
exec sleep 20
"#;

/// Runs `show` on a terminal (`script` gives it one) with `pager`, an sh script in which DIR is
/// the data directory, presses Ctrl-C once the pager has written DIR/ready, and returns how
/// neomail exited, with the data directory.
#[cfg(target_os = "linux")]
fn ctrl_c_while_paging(pager: &str) -> (std::process::ExitStatus, tempfile::TempDir) {
    let dir = seeded();
    let data = dir.path();
    let id = &ids(data)[2];
    let script = data.join("pager.sh");
    std::fs::write(&script, pager.replace("DIR", data.to_str().unwrap())).unwrap();
    // `exec` keeps neomail the terminal's foreground process group, with the pager in it.
    let run = format!("exec '{}' show {id}", env!("CARGO_BIN_EXE_neomail"));
    let mut c = Command::new("script");
    c.args(["-qec", &run, "/dev/null"]);
    isolate(&mut c, data);
    let mut child = c
        .env("SHELL", "/bin/sh")
        .env("NEOMAIL_PAGER", format!("sh '{}'", script.display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let ready = data.join("ready");
    let start = std::time::Instant::now();
    while !ready.exists() {
        if start.elapsed() > std::time::Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("the pager never started");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // ^C on the terminal: SIGINT to the foreground process group, neomail and the pager.
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"\x03").unwrap();
    stdin.flush().unwrap();
    let status = child.wait().unwrap();
    drop(stdin);
    (status, dir)
}

#[cfg(target_os = "linux")]
#[test]
fn ctrl_c_in_the_pager_waits_for_the_pager() {
    let (status, dir) = ctrl_c_while_paging(PAGER_CATCHING_CTRL_C);
    // The pager writes its result as it exits, so it is there once neomail has exited.
    let result = std::fs::read_to_string(dir.path().join("result"));
    assert!(result.is_ok(), "neomail exited before its pager");
    assert_eq!(result.unwrap(), "alive\n", "neomail died before its pager");
    assert!(
        dir.path().join("got-int").exists(),
        "the pager never saw the Ctrl-C"
    );
    // How neomail exits after waiting depends on the system's sh, which runs the pager. bash
    // execs the pager, so neomail sees the pager's own exit and exits 0. Ubuntu's dash keeps a
    // shell between them that dies of the Ctrl-C once the pager has exited (CI run 38274337495),
    // and neomail follows it with 130. Either way it waited.
    assert!(status.success() || status.code() == Some(130), "{status:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_pager_dying_of_ctrl_c_takes_neomail_with_it() {
    let (status, _dir) = ctrl_c_while_paging(PAGER_DYING_OF_CTRL_C);
    // script -e reports a child killed by a signal as 128 + its number.
    assert_eq!(status.code(), Some(130), "{status:?}");
}

#[test]
fn completions_for_every_shell() {
    let dir = tempfile::tempdir().unwrap();
    for (shell, needle) in [
        ("bash", "complete -F"),
        ("zsh", "#compdef neomail"),
        ("fish", "complete -c neomail"),
        ("elvish", "edit:completion:arg-completer[neomail]"),
        ("powershell", "Register-ArgumentCompleter"),
    ] {
        let o = neomail(dir.path(), &["completions", shell]);
        assert!(o.status.success(), "{shell}: {}", stderr(&o));
        let out = stdout(&o);
        assert!(out.contains(needle), "{shell}: {out}");
        assert!(out.contains("account"), "{shell}");
    }
    assert_eq!(
        neomail(dir.path(), &["completions", "tcsh"]).status.code(),
        Some(2)
    );
}

/// Runs the binary with its stdout's reader closed before it writes: it must exit 0 silently.
fn closed_reader(data: &Path, args: &[&str], input: &str) {
    let mut child = command(data, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
    assert_eq!(stderr(&o), "", "{args:?}");
}

#[test]
fn a_broken_pipe_exits_0_silently() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let work = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    // More than a pipe's buffer (64 KiB on Linux), so a write must fail.
    let messages: Vec<_> = (1..=1500)
        .map(|uid| message(uid, i64::from(uid), "a@mail.example", LONG_SUBJECT, false))
        .collect();
    cache.insert_messages(work.id, &messages).unwrap();
    drop(cache);
    let all: String = (1..=1500).map(|id| format!("{id}\n")).collect();
    for args in [
        &["list", "--limit", "all"][..],
        &["list", "--limit", "all", "--json"],
        &["list", "--limit", "all", "--jsonl"],
        &["show", "-"],
        &["show", "-", "--json"],
        &["completions", "bash"],
    ] {
        closed_reader(dir.path(), args, &all);
    }
}
