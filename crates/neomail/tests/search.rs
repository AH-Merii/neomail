//! `neomail search` against a temporary data directory whose cache was never indexed: the
//! command indexes it, then prints what `neomail list` prints for the matches. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::path::Path;
use std::process::{Command, Output};

use chrono::DateTime;
use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::Value;

fn message(uid: u32, from: &str, subject: &str, body: &str) -> FetchedMessage {
    let raw = format!(
        "From: {from}\r\nTo: me@mail.example\r\nSubject: {subject}\r\n\
         Message-ID: <m{uid}@mail.example>\r\nDate: Sat, 10 Oct 2026 09:30:00 +0000\r\n\r\n{body}\r\n"
    );
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::default(),
        received: DateTime::from_timestamp(i64::from(uid) * 100, 0),
        raw: raw.into_bytes(),
    }
}

/// Three messages in "work" and one in "home", not indexed.
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
                message(1, "Ada <ada@mail.example>", "Invoice 12", "Please pay."),
                message(2, "bob@mail.example", "Lunch", "The invoice can wait."),
                message(3, "carol@mail.example", "Holiday", "Back in May."),
            ],
        )
        .unwrap();
    let home = cache
        .reset_mailbox("home", "INBOX", UidValidity::new(9).unwrap())
        .unwrap();
    cache
        .insert_messages(home.id, &[message(5, "dan@mail.example", "Invoices", "")])
        .unwrap();
    dir
}

fn neomail(data: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .env("NEOMAIL_DATA_DIR", data)
        .env("NEOMAIL_CONFIG", data.join("config.toml"))
        .env("HOME", data)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent");
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "NEOMAIL_NO_INPUT",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
    ] {
        c.env_remove(var);
    }
    c.output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn json(o: &Output) -> Vec<Value> {
    assert!(o.status.success(), "{}", stderr(o));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn matches_print_as_lists_objects_newest_first() {
    let dir = seeded();
    let found = json(&neomail(dir.path(), &["search", "invoice", "--json"]));
    let subjects: Vec<_> = found
        .iter()
        .map(|m| m["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["Invoices", "Lunch", "Invoice 12"]);
    let listed = json(&neomail(dir.path(), &["list", "--json"]));
    for item in &found {
        assert!(listed.contains(item), "{item} is list's object");
    }

    let o = neomail(dir.path(), &["search", "invoice", "--jsonl"]);
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines, found);

    let o = neomail(dir.path(), &["search", "invoice", "--account", "work"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0][1..],
        [
            "*",
            "2026-10-10T09:30:00Z",
            "work",
            "bob@mail.example",
            "Lunch"
        ]
    );
}

#[test]
fn the_query_words_flags_and_thread_narrow_the_matches() {
    let dir = seeded();
    let subjects = |args: &[&str]| -> Vec<String> {
        json(&neomail(
            dir.path(),
            &[&["search", "--json"], args].concat(),
        ))
        .iter()
        .map(|m| m["subject"].as_str().unwrap().to_owned())
        .collect()
    };
    assert_eq!(subjects(&["invoice", "pay"]), ["Invoice 12"]);
    assert_eq!(subjects(&["from:ada"]), ["Invoice 12"]);
    assert_eq!(subjects(&["invoice", "--limit", "1"]), ["Invoices"]);
    assert_eq!(subjects(&["--group", "unclassified", "may"]), ["Holiday"]);
    let id = json(&neomail(dir.path(), &["search", "holiday", "--json"]))[0]["id"].to_string();
    assert_eq!(subjects(&["--thread", &id]), ["Holiday"]);
}

#[test]
fn no_match_succeeds_with_an_empty_listing_and_says_so() {
    let dir = seeded();
    assert!(json(&neomail(dir.path(), &["search", "zebra", "--json"])).is_empty());
    let o = neomail(dir.path(), &["search", "zebra", "--plain"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty());
    assert!(
        stderr(&o).contains("no cached messages match"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn bad_usage_exits_2_and_unknown_names_exit_1() {
    let dir = seeded();
    for args in [
        &["search"][..],
        &["search", "--account", "work"],
        &["search", "-", "*"],
        &["search", "from:"],
        &["search", "x", "--group", "nonsense"],
        &["search", "x", "--json", "--plain"],
    ] {
        let o = neomail(dir.path(), args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(o.stdout.is_empty(), "{args:?}");
    }
    for args in [
        &["search", "x", "--account", "nobody"][..],
        &["search", "--thread", "999"],
    ] {
        let o = neomail(dir.path(), args);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", stderr(&o));
    }
}

#[test]
fn without_a_cache_search_finds_nothing_but_a_named_account_fails() {
    let dir = tempfile::tempdir().unwrap();
    assert!(json(&neomail(dir.path(), &["search", "x", "--json"])).is_empty());
    let o = neomail(dir.path(), &["search", "x"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("`neomail account add`"),
        "names the next command: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let o = neomail(dir.path(), &["search", "x", "--account", "work"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        !dir.path().join("cache.sqlite3").exists(),
        "search creates no cache"
    );
}
