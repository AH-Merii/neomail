//! `nmail list`, `show` and the offline failure paths of `sync`, against a temporary data
//! directory. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use chrono::DateTime;
use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

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

fn nmail(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nmail"))
        .args(args)
        .env("NMAIL_DATA_DIR", data)
        .env("NMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap()
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
fn list_prints_newest_first_within_80_columns() {
    let dir = seeded();
    let o = nmail(dir.path(), &["list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert!(
        lines[0].contains("bob@mail.example") && lines[0].contains('…'),
        "{out}"
    );
    assert!(
        lines[1].contains("Carol") && !lines[1].contains('\x1b'),
        "{out}"
    );
    assert!(
        lines[2].contains("Ada Lovelace") && lines[2].contains("Oldest"),
        "{out}"
    );
    assert!(lines[3].contains("Home"), "{out}");
    for line in &lines {
        assert!(line.width() <= 80, "too wide: {line:?}");
    }
    // Unread mark after the id.
    assert_eq!(lines[0].split_whitespace().nth(1), Some("*"));
    assert_ne!(lines[2].split_whitespace().nth(1), Some("*"));

    let o = nmail(dir.path(), &["list", "--account", "work", "--limit", "2"]);
    assert_eq!(stdout(&o).lines().count(), 2);
}

#[test]
fn list_json_shape() {
    let dir = seeded();
    let v = json(&nmail(dir.path(), &["list", "--json", "--account", "work"]));
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
    let id = json(&nmail(dir.path(), &["list", "--json"]))[2]["id"]
        .as_i64()
        .unwrap()
        .to_string();

    let o = nmail(dir.path(), &["show", &id]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("From:    Ada Lovelace <ada@mail.example>"),
        "{out}"
    );
    assert!(out.contains("To:      me@mail.example"), "{out}");
    assert!(out.contains("Subject: Oldest"), "{out}");
    assert!(out.contains("Hello from 1."), "{out}");

    let v = json(&nmail(dir.path(), &["show", &id, "--json"]));
    assert_eq!(v["body_type"], "text/plain");
    assert_eq!(v["from"][0]["name"], "Ada Lovelace");
    assert_eq!(v["attachments"], serde_json::json!([]));
    assert_eq!(v["unread"], false);

    let o = nmail(dir.path(), &["show", &id, "--raw"]);
    assert!(o.stdout.starts_with(b"From: Ada Lovelace"));
}

#[test]
fn list_counts_emoji_sequences_as_two_columns() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let work = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    // U+2764 U+FE0F is one character wide on its own, two as an emoji sequence.
    let hearts = "\u{2764}\u{FE0F}".repeat(40);
    cache
        .insert_messages(
            work.id,
            &[message(
                1,
                100,
                "\u{2764}\u{FE0F} <a@mail.example>",
                &hearts,
                false,
            )],
        )
        .unwrap();
    let o = nmail(dir.path(), &["list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let line = out.lines().next().unwrap();
    assert!(line.width() <= 80, "too wide ({}): {line:?}", line.width());
    assert!(line.contains('…'), "{line:?}");
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
    // file's mode, from being removed when nmail exits.
    let _open = Cache::open(&cache).unwrap();
    let mode = |file: &str| {
        let meta = std::fs::metadata(dir.path().join(file)).unwrap();
        meta.permissions().mode() & 0o777
    };
    assert_eq!(mode("cache.sqlite3-wal"), 0o644);
    assert_eq!(mode("cache.sqlite3-shm"), 0o644);

    assert!(nmail(dir.path(), &["list"]).status.success());
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
    let o = nmail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(all.contains("line 3, column"), "{all}");
    assert!(!all.contains("s3cret"), "{all}");
    assert_eq!(all.lines().count(), 1, "{all}");
}

#[test]
fn empty_nmail_config_counts_as_unset() {
    let dir = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_nmail"))
        .arg("sync")
        .env("NMAIL_DATA_DIR", dir.path())
        .env("NMAIL_CONFIG", "")
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

#[test]
fn failures_are_clear_and_non_zero() {
    let dir = seeded();
    let o = nmail(dir.path(), &["show", "999"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("no cached message with id 999"),
        "{}",
        stderr(&o)
    );

    assert_eq!(nmail(dir.path(), &["show", "abc"]).status.code(), Some(2));
    assert_eq!(
        nmail(dir.path(), &["list", "--limit", "0"]).status.code(),
        Some(2)
    );

    let o = nmail(dir.path(), &["list", "--account", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("home, work"), "{}", stderr(&o));

    let o = nmail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no config file"), "{}", stderr(&o));
}

#[test]
fn empty_data_dir_lists_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let o = nmail(dir.path(), &["list"]);
    assert!(o.status.success());
    assert!(stdout(&o).is_empty());
    assert!(stderr(&o).contains("nmail sync"));
    assert_eq!(
        json(&nmail(dir.path(), &["list", "--json"])),
        serde_json::json!([])
    );
    assert_eq!(nmail(dir.path(), &["show", "1"]).status.code(), Some(1));
    assert!(
        !dir.path().join("cache.sqlite3").exists(),
        "list must not create the cache"
    );
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
    let o = nmail(dir.path(), &["sync"]);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(all.contains("password_command failed"), "{all}");
    assert!(!all.contains("hunter2"), "{all}");
}

const KEYRING_CONFIG: &str = "[[accounts]]\nname = \"work\"\nusername = \"user@mail.example\"\n\
                              imap = { host = \"imap.mail.example\" }\n";
const FAKE_PASSWORD: &str = "s3cret-fake-password";

/// Runs `nmail` with `input` on standard input and a D-Bus session bus that does not exist, so
/// nothing reaches a real keyring.
fn nmail_stdin(data: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nmail"))
        .args(args)
        .env("NMAIL_DATA_DIR", data)
        .env("NMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // nmail may exit before it reads, closing the pipe.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn set_password_checks_the_account_and_refuses_an_empty_password() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), KEYRING_CONFIG).unwrap();

    let input = format!("{FAKE_PASSWORD}\n");
    let o = nmail_stdin(dir.path(), &["account", "set-password", "nope"], &input);
    assert_eq!(o.status.code(), Some(1));
    let all = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        all.contains("no account named \"nope\"; configured: work"),
        "{all}"
    );
    assert!(!all.contains(FAKE_PASSWORD), "{all}");

    // The config has no password_command and loads: the empty password is what fails.
    for input in ["", "\n", "\r\n"] {
        let o = nmail_stdin(dir.path(), &["account", "set-password", "work"], input);
        assert_eq!(o.status.code(), Some(1), "{input:?}");
        assert!(
            stderr(&o).contains("the password is empty"),
            "{}",
            stderr(&o)
        );
    }

    let o = nmail_stdin(dir.path(), &["account", "delete-password", "nope"], "");
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
        let o = nmail_stdin(dir.path(), args, &input);
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
}
