//! `neomail classify`, `neomail key` and `neomail list --group` against a fake jev on 127.0.0.1
//! and a temporary data directory. Nothing reaches beyond this machine, no real keyring is
//! opened, and nothing sleeps: retries are told to wait 0 seconds.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use chrono::{Duration, Utc};
use nm_core::{Cache, FetchedMessage, Flags, Uid, UidValidity};
use serde_json::{Value, json};

#[path = "support/fake_jev.rs"]
mod fake_jev;
use fake_jev::{FakeJev, GROUPS, Reply, default_answer};

const KEY: &str = "fake-jev-key-0123";

/// A message `days` old, from `from`, with a Message-ID made from `uid`.
fn mail(uid: u32, days: i64, from: &str, subject: &str, body: &str) -> FetchedMessage {
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::new(Vec::<String>::new()),
        received: Some(Utc::now() - Duration::days(days)),
        raw: format!(
            "Message-ID: <m{uid}@acme.example>\r\nFrom: {from}\r\nTo: you@example.com\r\n\
             Subject: {subject}\r\n\r\n{body}\r\n"
        )
        .into_bytes(),
    }
}

/// A data directory whose config points classify.url at `jev`, with `messages` cached in the
/// account "work" (UIDVALIDITY 1).
fn data(jev: &FakeJev, messages: &[FetchedMessage]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &jev.url(), "");
    cache_mail(dir.path(), 1, messages);
    dir
}

fn write_config(dir: &Path, url: &str, extra: &str) {
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
             imap = {{ host = \"imap.example.com\" }}\n\n[classify]\nurl = \"{url}\"\n{extra}"
        ),
    )
    .unwrap();
}

/// (Re)fetches the INBOX of "work" under `validity`: a new value is a rebuild, which gives every
/// message a new id.
fn cache_mail(dir: &Path, validity: u32, messages: &[FetchedMessage]) {
    let mut cache = Cache::open(&dir.join("cache.sqlite3")).unwrap();
    let inbox = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(validity).unwrap())
        .unwrap();
    cache.insert_messages(inbox.id, messages).unwrap();
}

/// Caches `messages` again in the mailbox `mailbox` of "work": copies with new ids.
fn copy_to(dir: &Path, mailbox: &str, messages: &[FetchedMessage]) {
    let mut cache = Cache::open(&dir.join("cache.sqlite3")).unwrap();
    let copy = cache
        .reset_mailbox("work", mailbox, UidValidity::new(1).unwrap())
        .unwrap();
    cache.insert_messages(copy.id, messages).unwrap();
}

/// The binary with its data and config in `dir`, the fake key in JEV_API_KEY, and the debug
/// build's file keyring in `dir` (empty unless a test stores a key), so never the system's.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neomail"));
    c.args(args)
        .env("NEOMAIL_DATA_DIR", dir)
        .env("NEOMAIL_CONFIG", dir.join("config.toml"))
        .env("HOME", dir)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("JEV_API_KEY", KEY)
        .env("NEOMAIL_TEST_KEYRING", dir.join("test-keyring.json"))
        .env_remove("NEOMAIL_NO_INPUT")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    c
}

fn neomail(dir: &Path, args: &[&str]) -> Output {
    command(dir, args).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", stdout(o), stderr(o)))
}

/// Each listed message's subject and group, newest first.
fn groups(dir: &Path, args: &[&str]) -> Vec<(String, Value)> {
    let o = neomail(dir, &[&["list", "--json", "--limit", "all"], args].concat());
    assert!(o.status.success(), "{}", stderr(&o));
    json(&o)
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["subject"].as_str().unwrap().to_owned(),
                m["group"].clone(),
            )
        })
        .collect()
}

/// Three recent messages, newest first in list order.
fn three() -> Vec<FetchedMessage> {
    vec![
        mail(
            1,
            1,
            "orders@shop.example",
            "orders-shipping Your parcel",
            "On its way.",
        ),
        mail(
            2,
            2,
            "Bob <bob@friends.example>",
            "follow-up Lunch?",
            "Free on Friday?",
        ),
        mail(
            3,
            3,
            "news@paper.example",
            "newsletters Weekly",
            "This week.",
        ),
    ]
}

#[test]
fn only_the_redacted_text_the_domain_and_the_signals_leave_with_the_bearer_key() {
    let jev = FakeJev::start();
    let dir = data(
        &jev,
        &[mail(
            1,
            1,
            "Ada Lovelace <ada.lovelace@acme.example>",
            "Hello from Ada Lovelace",
            "Write to ada.lovelace@acme.example or call Ada on +44 20 7946 0958.",
        )],
    );
    let o = neomail(dir.path(), &["classify", "--yes", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let captured = jev.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    // https://docs.typesafe.ai/api.md: POST {url}/v1/systemone with a JSON body.
    assert_eq!(captured[0].request_line, "POST /v1/systemone HTTP/1.1");
    assert_eq!(
        captured[0].content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    for planted in ["ada.lovelace", "Lovelace", "Ada", "7946"] {
        assert!(
            !captured[0].body.contains(planted),
            "{planted}: {}",
            captured[0].body
        );
    }
    let state = &captured[0].json()["state"];
    assert_eq!(
        state["sender"],
        json!({"domain": "acme.example", "kind": "personal"})
    );
    // What is sent is exactly what `neomail redact` previews.
    let id = json(&o)[0]["id"].to_string();
    let preview = json(&neomail(dir.path(), &["redact", "--json", &id]));
    assert_eq!(state["subject"], preview["subject"]);
    assert_eq!(state["body"], preview["body"]);
    assert_eq!(state["truncated"], preview["truncated"]);
    for signal in ["mailing_list", "automated", "reply"] {
        assert_eq!(state[signal], preview["signals"][signal], "{signal}");
    }
    assert_eq!(state["sender"]["domain"], preview["signals"]["domain"]);
    assert_eq!(state["sender"]["kind"], preview["signals"]["kind"]);
}

#[test]
fn consent_is_asked_once_per_host_and_without_it_nothing_is_sent() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    // No terminal, no --yes, nothing stored: a usage error, and no request.
    for extra in [&[][..], &["--no-input"][..]] {
        let o = neomail(dir.path(), &[&["classify"], extra].concat());
        assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
        assert!(stderr(&o).contains("--yes"), "{}", stderr(&o));
    }
    assert_eq!(jev.count(), 0);
    let o = neomail(dir.path(), &["classify", "--yes", "--limit", "1"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 1);
    // The yes is remembered for this host...
    let o = neomail(dir.path(), &["classify", "--no-input"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    // ...and only this one: localhost is another host.
    write_config(dir.path(), &format!("http://localhost:{}", jev.port), "");
    let o = neomail(dir.path(), &["classify", "--again"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
}

#[test]
fn a_classified_message_is_never_asked_again_even_after_a_rebuild() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let o = neomail(dir.path(), &["classify"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("nothing to classify"), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let before = groups(dir.path(), &[]);
    // A UIDVALIDITY change: the mailbox is rebuilt and every message gets a new id.
    cache_mail(dir.path(), 2, &three());
    assert_eq!(groups(dir.path(), &[]), before);
    assert_eq!(before[0].1, "orders-shipping");
    let o = neomail(dir.path(), &["classify"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    // A message named by id that has a result prints it without asking.
    let id = json(&neomail(dir.path(), &["list", "--json", "--limit", "1"]))[0]["id"].to_string();
    let o = neomail(dir.path(), &["classify", "--plain", &id]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), format!("{id}\twork\torders-shipping\t0.90\n"));
    assert_eq!(jev.count(), 3);
    // Only --again asks again.
    let o = neomail(dir.path(), &["classify", "--again"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 6);
}

#[test]
fn a_message_cached_before_schema_5_shows_its_group_once_classified() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    // Rows cached before schema 5 have no msg_key until classify reads them.
    rusqlite::Connection::open(dir.path().join("cache.sqlite3"))
        .unwrap()
        .execute("UPDATE message SET msg_key = NULL", [])
        .unwrap();
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let listed: Vec<Value> = groups(dir.path(), &[])
        .into_iter()
        .map(|(_, g)| g)
        .collect();
    assert_eq!(
        listed,
        [
            json!("orders-shipping"),
            json!("follow-up"),
            json!("newsletters")
        ]
    );
    let o = neomail(dir.path(), &["classify"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    // A row without a key whose message already has a result is not asked again.
    rusqlite::Connection::open(dir.path().join("cache.sqlite3"))
        .unwrap()
        .execute("UPDATE message SET msg_key = NULL", [])
        .unwrap();
    let o = neomail(dir.path(), &["classify"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let again: Vec<Value> = groups(dir.path(), &[])
        .into_iter()
        .map(|(_, g)| g)
        .collect();
    assert_eq!(again, listed);
}

#[test]
fn a_429_is_retried_after_the_wait_jev_names() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    jev.play([Reply {
        status: 429,
        headers: vec![("Retry-After", "0")],
        body: "{}".to_owned(),
    }]);
    let o = neomail(dir.path(), &["classify", "--yes", "--plain"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 2);
    assert!(
        stdout(&o).contains("\twork\torders-shipping\t0.90\n"),
        "{}",
        stdout(&o)
    );
}

/// `n` recent messages that the fake answers with main, more than the 4 requests in flight can
/// account for.
fn many(n: u32) -> Vec<FetchedMessage> {
    (1..=n)
        .map(|uid| {
            mail(
                uid,
                i64::from(uid),
                "news@paper.example",
                &format!("Note {uid}"),
                "Hello.",
            )
        })
        .collect()
}

/// At most the requests already in flight when the run stops, each possibly retried once more.
const AT_MOST_AFTER_A_STOP: usize = 2 * 4 + 2;

#[test]
fn a_refused_key_stops_the_run_with_the_key_hint() {
    let jev = FakeJev::start();
    let dir = data(&jev, &many(12));
    jev.play([Reply::status(401, r#"{"error":"bad key"}"#)]);
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("neomail key set"), "{}", stderr(&o));
    assert!(!stderr(&o).contains(KEY), "{}", stderr(&o));
    let sent = jev.count();
    assert!(sent <= AT_MOST_AFTER_A_STOP, "{sent} requests after a 401");
    // Every answer that arrived is kept; the rest wait for the next run.
    let left = groups(dir.path(), &["--group", "unclassified"]).len();
    assert_eq!(left, 12 - (sent - 1), "{sent} requests");
}

#[test]
fn an_unexpected_status_stops_the_run_and_keeps_what_was_stored() {
    let jev = FakeJev::start();
    let dir = data(&jev, &many(12));
    let first = default_answer(&json!({"state": {"subject": "receipts-invoices"}}).to_string());
    jev.play([first, Reply::status(402, "out of credit")]);
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("402: out of credit"), "{}", stderr(&o));
    let sent = jev.count();
    assert!(sent <= AT_MOST_AFTER_A_STOP, "{sent} requests after a 402");
    let listed = groups(dir.path(), &[]);
    let classified: Vec<_> = listed.iter().filter(|(_, g)| !g.is_null()).collect();
    // Every request but the 402 was answered and stored, the first among them.
    assert_eq!(classified.len(), sent - 1, "{classified:?}");
    assert_eq!(
        classified
            .iter()
            .filter(|(_, g)| *g == "receipts-invoices")
            .count(),
        1,
        "{classified:?}"
    );
    assert_eq!(
        groups(dir.path(), &["--group", "unclassified"]).len(),
        12 - classified.len()
    );
}

#[test]
fn a_message_is_tried_three_times_at_most() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    // A fourth try would get the fake's default answer and succeed.
    jev.play((0..3).map(|_| Reply {
        status: 429,
        headers: vec![("Retry-After", "0")],
        body: "{}".to_owned(),
    }));
    let started = std::time::Instant::now();
    let o = neomail(dir.path(), &["classify", "--yes", "--jsonl"]);
    // Retry-After: 0 is waited, not the backoff, which alone takes at least 1.5 s for 3 tries.
    assert!(started.elapsed() < std::time::Duration::from_millis(1400));
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{}", stdout(&o));
    assert!(lines[0]["error"].is_string(), "{}", lines[0]);
    assert!(lines[0]["group"].is_null(), "{}", lines[0]);
    assert_eq!(groups(dir.path(), &["--group", "unclassified"]).len(), 1);
}

#[test]
fn a_status_that_keeps_asking_for_a_retry_stops_the_run() {
    let jev = FakeJev::start();
    let dir = data(&jev, &many(12));
    // Enough 503s for every try of every message; the fake answers later requests with 200.
    jev.play((0..12 * 3).map(|_| Reply {
        status: 503,
        headers: vec![("Retry-After", "0")],
        body: "{}".to_owned(),
    }));
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("the run stopped"), "{}", stderr(&o));
    // At most 3 tries for each of the 4 messages in flight, not 3 for every message.
    let sent = jev.count();
    assert!(
        sent <= 3 * 4,
        "{sent} requests while jev kept answering 503"
    );
    assert_eq!(groups(dir.path(), &["--group", "unclassified"]).len(), 12);
}

#[test]
fn a_422_fails_one_message_and_the_others_are_classified() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    jev.play([Reply::status(422, r#"{"detail":"bad"}"#)]);
    let o = neomail(dir.path(), &["classify", "--yes", "--jsonl"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    let failed: Vec<_> = lines.iter().filter(|l| l.get("error").is_some()).collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["group"].is_null());
    assert_eq!(groups(dir.path(), &["--group", "unclassified"]).len(), 1);
}

#[test]
fn the_window_and_the_cap_choose_the_newest_and_the_rest_is_estimated() {
    let jev = FakeJev::start();
    let mut messages = three();
    messages.push(mail(
        4,
        400,
        "old@shop.example",
        "receipts-invoices Old",
        "Paid.",
    ));
    let dir = data(&jev, &messages);
    // A copy of the old message in a second mailbox is the same message: counted once.
    copy_to(dir.path(), "Archive", &messages[3..]);
    // --estimate sends nothing and needs no consent.
    let o = neomail(
        dir.path(),
        &["classify", "--estimate", "--limit", "2", "--json"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let e = json(&o);
    assert_eq!(
        (e["messages"].clone(), e["requests"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(
        e["left"]["messages"], 2,
        "one past the cap, one past the window"
    );
    assert_eq!(e["tokens_from"], "characters");
    assert!(e["input_tokens"].as_u64().unwrap() > 0);
    // stdout carries the estimate only; the note goes to stderr.
    let o = neomail(dir.path(), &["classify", "--estimate", "--plain"]);
    assert!(stdout(&o).starts_with("would send: "), "{}", stdout(&o));
    assert!(!stdout(&o).contains("nothing was sent"), "{}", stdout(&o));
    assert!(stderr(&o).contains("nothing was sent"), "{}", stderr(&o));
    // A wider window takes the old message too.
    let o = neomail(
        dir.path(),
        &["classify", "--estimate", "--days", "450", "--json"],
    );
    let e = json(&o);
    assert_eq!(
        (&e["messages"], &e["left"]["messages"]),
        (&json!(4), &json!(0))
    );
    assert!(
        e["cost_usd"].is_null(),
        "no price for a backend other than TypeSafe"
    );
    assert_eq!(jev.count(), 0);

    let o = neomail(dir.path(), &["classify", "--yes", "--limit", "2", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 2);
    let subjects: Vec<_> = jev
        .bodies()
        .iter()
        .map(|b| b["state"]["subject"].clone())
        .collect();
    assert!(
        subjects.contains(&json!("orders-shipping Your parcel")),
        "{subjects:?}"
    );
    assert!(
        subjects.contains(&json!("follow-up Lunch?")),
        "{subjects:?}"
    );
    let err = stderr(&o);
    assert!(err.contains("2 messages, 2 requests"), "{err}");
    assert!(err.contains("characters divided by 4"), "{err}");
    assert!(
        err.contains("`neomail classify --all` would send every message without a group"),
        "{err}"
    );

    // The default window keeps out the 400-day-old message; --all takes it.
    let o = neomail(dir.path(), &["classify"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 3);
    let o = neomail(dir.path(), &["classify", "--all"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 4);
    assert!(!stderr(&o).contains("`neomail classify"), "{}", stderr(&o));
}

#[test]
fn a_repeated_id_or_a_copy_is_asked_about_once() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    copy_to(dir.path(), "Archive", &three()[..1]);
    let ids: Vec<String> = json(&neomail(dir.path(), &["list", "--json"]))
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].to_string())
        .collect();
    assert_eq!(ids.len(), 2, "the message and its copy");
    for (n, args) in [
        ["classify", "--yes", ids[0].as_str(), ids[0].as_str()],
        ["classify", "--again", ids[0].as_str(), ids[1].as_str()],
    ]
    .into_iter()
    .enumerate()
    {
        let o = neomail(dir.path(), &args);
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert_eq!(jev.count(), n + 1, "{args:?}");
    }
}

#[test]
fn an_answer_that_is_not_jevs_stops_the_run() {
    let jev = FakeJev::start();
    let dir = data(&jev, &many(12));
    // A 2xx from some other service: every message would be sent to it for nothing.
    jev.play((0..12).map(|_| Reply::status(200, "{}")));
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("the run stopped"), "{}", stderr(&o));
    assert!(jev.count() <= AT_MOST_AFTER_A_STOP, "{}", jev.count());
}

#[test]
fn a_large_error_page_is_still_retried() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    // A proxy's 503 page, larger than any answer jev gives.
    jev.play([Reply {
        status: 503,
        headers: vec![("Retry-After", "0")],
        body: "<p>busy</p>".repeat(10_000),
    }]);
    let o = neomail(dir.path(), &["classify", "--yes", "--plain"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 2);
}

#[test]
fn account_takes_only_that_accounts_mail() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    write_config(
        dir.path(),
        &jev.url(),
        "\n[[accounts]]\nname = \"home\"\nusername = \"me@example.org\"\n\
         imap = { host = \"imap.example.org\" }\n",
    );
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let inbox = cache
        .reset_mailbox("home", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let sale = mail(9, 1, "a@shop.example", "promotions Sale", "Deals.");
    cache.insert_messages(inbox.id, &[sale]).unwrap();
    drop(cache);
    let o = neomail(
        dir.path(),
        &["classify", "--estimate", "--account", "work", "--json"],
    );
    assert_eq!(json(&o)["messages"], 3, "{}", stdout(&o));
    let o = neomail(
        dir.path(),
        &["classify", "--yes", "--account", "home", "--jsonl"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(jev.count(), 1);
    assert_eq!(jev.bodies()[0]["state"]["subject"], "promotions Sale");
    let printed: Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(printed["account"], "home");
}

#[test]
fn the_hint_for_what_was_left_keeps_the_account_and_again() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    let o = neomail(
        dir.path(),
        &[
            "classify",
            "--estimate",
            "--account",
            "work",
            "--limit",
            "1",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("`neomail classify --all --account work` would send"),
        "{}",
        stdout(&o)
    );
    let o = neomail(
        dir.path(),
        &["classify", "--yes", "--account", "work", "--limit", "1"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("`neomail classify --all --account work` would send"),
        "{}",
        stderr(&o)
    );
    let o = neomail(
        dir.path(),
        &[
            "classify",
            "--estimate",
            "--again",
            "--account",
            "work",
            "--limit",
            "1",
        ],
    );
    assert!(
        stdout(&o).contains(
            "`neomail classify --all --account work --again` would ask about every message again"
        ),
        "{}",
        stdout(&o)
    );
}

/// The `neomail classify --all ...` command a hint names, as arguments after `neomail`, and the
/// requests the hint says it sends: the first count after the command.
fn hint(text: &str) -> (Vec<String>, usize) {
    let start = 1 + text
        .find("`neomail classify --all")
        .unwrap_or_else(|| panic!("no command in: {text}"));
    let end = start + text[start..].find('`').unwrap();
    let command = text[start..end].split_whitespace().skip(1);
    let words: Vec<&str> = text[end..].split_whitespace().collect();
    let count = words
        .windows(2)
        .find(|w| w[1].starts_with("request"))
        .unwrap_or_else(|| panic!("no count in: {text}"))[0];
    (
        command.map(str::to_owned).collect(),
        count.replace(',', "").parse().unwrap(),
    )
}

/// A run whose hint is followed.
struct HintCase {
    case: &'static str,
    messages: Vec<FetchedMessage>,
    /// Every message is classified once before the run.
    classified: bool,
    /// The fake's replies for the run.
    replies: Vec<Reply>,
    args: &'static [&'static str],
    /// The count the hint must quote, where it does not depend on timing.
    expected: Option<usize>,
}

#[test]
fn the_command_a_hint_names_sends_as_many_requests_as_the_hint_says() {
    let cases = [
        HintCase {
            case: "--again",
            messages: three(),
            classified: true,
            replies: vec![],
            args: &["classify", "--again", "--limit", "1"],
            expected: Some(3),
        },
        HintCase {
            case: "--estimate --again",
            messages: three(),
            classified: true,
            replies: vec![],
            args: &["classify", "--estimate", "--again", "--limit", "1"],
            expected: Some(3),
        },
        HintCase {
            case: "--estimate",
            messages: three(),
            classified: false,
            replies: vec![],
            args: &["classify", "--estimate", "--limit", "1"],
            expected: Some(3),
        },
        HintCase {
            case: "a 422",
            messages: three(),
            classified: false,
            replies: vec![Reply::status(422, r#"{"detail":"bad"}"#)],
            args: &["classify", "--limit", "2"],
            expected: Some(2),
        },
        HintCase {
            case: "a stopped run",
            messages: many(12),
            classified: false,
            replies: vec![Reply::status(402, "out of credit")],
            args: &["classify", "--limit", "8"],
            expected: None,
        },
    ];
    for HintCase {
        case,
        messages,
        classified,
        replies,
        args,
        expected,
    } in cases
    {
        let jev = FakeJev::start();
        let dir = data(&jev, &messages);
        if classified {
            let o = neomail(dir.path(), &["classify", "--yes"]);
            assert!(o.status.success(), "{case}: {}", stderr(&o));
        }
        jev.play(replies);
        let o = neomail(dir.path(), &[args, &["--yes"]].concat());
        let text = if args.contains(&"--estimate") {
            stdout(&o)
        } else {
            stderr(&o)
        };
        let (command, quoted) = hint(&text);
        if let Some(expected) = expected {
            assert_eq!(quoted, expected, "{case}: {text}");
        }
        let before = jev.count();
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        // --yes only consents: it changes nothing that is sent.
        let o = neomail(dir.path(), &[&command[..], &["--yes"]].concat());
        assert!(o.status.success(), "{case}: {command:?}: {}", stderr(&o));
        assert_eq!(
            jev.count() - before,
            quoted,
            "{case}: {command:?} after: {text}"
        );
    }
}

#[test]
fn list_derives_other_and_follow_up_old_and_filters_by_group() {
    let jev = FakeJev::start();
    let dir = data(
        &jev,
        &[
            mail(1, 1, "a@shop.example", "promotions unsure Sale", "Deals."),
            mail(
                2,
                100,
                "bob@friends.example",
                "follow-up Did you see this?",
                "Reply.",
            ),
            mail(3, 5, "x@scam.example", "spam unsure Win", "Prize."),
            mail(4, 6, "bob@friends.example", "Unsorted", "Hi."),
        ],
    );
    let ids: Vec<String> = json(&neomail(dir.path(), &["list", "--json"]))
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["subject"] != "Unsorted")
        .map(|m| m["id"].to_string())
        .collect();
    assert_eq!(ids.len(), 3);
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let o = neomail(
        dir.path(),
        &[&["classify", "--yes", "--json"], &ids[..]].concat(),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    // classify's own output shows the effective group apart from jev's choice.
    let printed = json(&o);
    let sale = printed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == serde_json::from_str::<Value>(ids[0]).unwrap())
        .unwrap();
    assert_eq!(
        (
            &sale["group"],
            &sale["choice"],
            &sale["confidence"],
            &sale["model"]
        ),
        (
            &json!("other"),
            &json!("promotions"),
            &json!(0.2),
            &json!("jev-1.13.0")
        ),
        "{sale}"
    );
    // Keys in the documented order: id first, the probabilities in the request's order (the
    // fake sends them sorted by name).
    let text = stdout(&o);
    assert!(text.find("\"id\"") < text.find("\"account\""), "{text}");
    let start = text.find("\"probabilities\": {").unwrap();
    let block = &text[start..start + text[start..].find('}').unwrap()];
    let keys: Vec<&str> = block
        .lines()
        .skip(1)
        .filter_map(|l| l.trim().split('"').nth(1))
        .collect();
    assert_eq!(keys, GROUPS, "{block}");
    let expected = [
        ("promotions unsure Sale", json!("other")),
        ("spam unsure Win", json!("spam")),
        ("Unsorted", Value::Null),
        ("follow-up Did you see this?", json!("follow-up-old")),
    ];
    let expected: Vec<_> = expected
        .iter()
        .map(|(s, g)| (s.to_string(), g.clone()))
        .collect();
    assert_eq!(groups(dir.path(), &[]), expected);
    let only = |group: &str| -> Vec<String> {
        groups(dir.path(), &["--group", group])
            .into_iter()
            .map(|(s, _)| s)
            .collect()
    };
    assert_eq!(only("follow-up-old"), ["follow-up Did you see this?"]);
    assert_eq!(only("unclassified"), ["Unsorted"]);
    assert!(only("main").is_empty());
    let o = neomail(dir.path(), &["list", "--group", "receipts"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    // New thresholds change the groups without asking jev again.
    write_config(
        dir.path(),
        &jev.url(),
        "other_below = 0.1\nfollow_up_old_days = 200\n",
    );
    let after: Vec<_> = groups(dir.path(), &[])
        .into_iter()
        .map(|(_, g)| g)
        .collect();
    assert_eq!(
        after,
        [
            json!("promotions"),
            json!("spam"),
            Value::Null,
            json!("follow-up")
        ]
    );
    assert_eq!(jev.count(), 3);
}

#[test]
fn the_key_is_stored_from_standard_input_and_its_status_never_shows_it() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    let keyring = dir.path().join("test-keyring.json");
    let stored = "keyring-key-4567";
    // Every run, whatever the key's source, is checked for both keys.
    let run = |args: &[&str], input: Option<&str>, env_key: bool| {
        let mut c = command(dir.path(), args);
        c.env("NEOMAIL_TEST_KEYRING", &keyring);
        if !env_key {
            c.env_remove("JEV_API_KEY");
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(input.unwrap_or("").as_bytes());
        drop(stdin);
        let o = child.wait_with_output().unwrap();
        for text in [stdout(&o), stderr(&o)] {
            assert!(
                !text.contains(KEY) && !text.contains(stored),
                "{args:?}: {text}"
            );
        }
        o
    };
    let o = run(&["key", "status"], None, false);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("no jev key"), "{}", stdout(&o));
    let o = run(&["key", "status"], None, true);
    assert!(o.status.success());
    assert!(stdout(&o).contains("JEV_API_KEY"), "{}", stdout(&o));
    // The hint for another backend names that backend, or following it would not help.
    let o = run(
        &["key", "status", "--backend", "http://127.0.0.1:9"],
        None,
        false,
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("run `neomail key set --backend http://127.0.0.1:9`"),
        "{}",
        stdout(&o)
    );
    // --no-input with nothing piped is a usage error.
    let o = run(&["--no-input", "key", "set"], None, false);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    // A named config that is missing is an error, not the default backend.
    let missing = dir.path().join("missing.toml");
    let missing = missing.to_str().unwrap();
    let o = run(&["--config", missing, "key", "set"], Some("k\n"), false);
    assert!(
        !o.status.success() && !stdout(&o).contains("Stored"),
        "{}",
        stdout(&o)
    );
    let o = run(&["key", "set"], Some(&format!("{stored}\n")), false);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("127.0.0.1"), "{}", stdout(&o));
    // The keyring comes before JEV_API_KEY, and nothing prints the key.
    let o = run(&["key", "status"], None, true);
    assert!(o.status.success());
    assert!(stdout(&o).contains("keyring"), "{}", stdout(&o));
    assert!(!stdout(&o).contains(stored) && !stderr(&o).contains(stored));
    let o = run(&["classify", "--yes"], None, true);
    assert!(o.status.success(), "{}", stderr(&o));
    let captured = jev.captured.lock().unwrap();
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(format!("Bearer {stored}").as_str())
    );
    drop(captured);
    let o = run(&["key", "delete"], None, false);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = run(&["key", "status"], None, false);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn without_a_key_nothing_is_sent() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three());
    let o = command(dir.path(), &["classify", "--yes"])
        .env_remove("JEV_API_KEY")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("no jev key: run `neomail key set` or set JEV_API_KEY"),
        "{}",
        stderr(&o)
    );
    assert_eq!(jev.count(), 0);
}

/// `neomail ARGS` with the debug build's file keyring in `dir`, `input` on standard input, and no
/// JEV_API_KEY.
fn with_keyring(dir: &Path, args: &[&str], input: &str) -> Output {
    let mut child = command(dir, args)
        .env("NEOMAIL_TEST_KEYRING", dir.join("test-keyring.json"))
        .env_remove("JEV_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input.as_bytes());
    drop(stdin);
    child.wait_with_output().unwrap()
}

#[test]
fn a_blank_key_is_refused_and_jev_api_key_still_counts() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    for blank in ["   \n", "\t\n", " "] {
        let o = with_keyring(dir.path(), &["key", "set"], blank);
        assert_eq!(o.status.code(), Some(1), "{blank:?}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("the key is empty; nothing was stored"),
            "{blank:?}: {}",
            stderr(&o)
        );
        assert!(!stdout(&o).contains("Stored"), "{blank:?}: {}", stdout(&o));
    }
    let o = with_keyring(dir.path(), &["key", "status"], "");
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    // With nothing stored, JEV_API_KEY's key is the one sent.
    let o = command(dir.path(), &["classify", "--yes"])
        .env("NEOMAIL_TEST_KEYRING", dir.path().join("test-keyring.json"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let captured = jev.captured.lock().unwrap();
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[test]
fn an_invalid_backend_is_bad_usage_for_every_key_command() {
    let jev = FakeJev::start();
    let dir = data(&jev, &three()[..1]);
    for command in ["set", "delete", "status"] {
        for backend in ["ftp://jev.example", "http://jev.example", "not a url"] {
            let o = with_keyring(dir.path(), &["key", command, "--backend", backend], "k\n");
            assert_eq!(
                o.status.code(),
                Some(2),
                "{command} {backend}: {}",
                stderr(&o)
            );
            assert!(
                stderr(&o).contains("--backend"),
                "{command} {backend}: {}",
                stderr(&o)
            );
            assert!(!stdout(&o).contains("Stored"), "{command} {backend}");
        }
    }
}

#[test]
fn a_closed_output_ends_quietly_and_every_answer_that_arrived_is_stored() {
    let jev = FakeJev::start();
    let dir = data(&jev, &many(12));
    let mut child = command(dir.path(), &["classify", "--yes", "--plain"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The reader is gone before the first result (`neomail classify --plain | head -0`), so
    // printing it fails with EPIPE while other answers are still on their way.
    drop(child.stdout.take());
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stderr(&o), "");
    let sent = jev.count();
    assert_eq!(
        groups(dir.path(), &["--group", "unclassified"]).len(),
        12 - sent,
        "{sent} requests answered"
    );
}

#[test]
fn the_help_says_yes_covers_the_daemon_only_without_an_earlier_yes() {
    let dir = tempfile::tempdir().unwrap();
    let o = neomail(dir.path(), &["classify", "--help"]);
    assert!(o.status.success());
    let flat = stdout(&o).split_whitespace().collect::<Vec<_>>().join(" ");
    for part in [
        "With no earlier yes for the host it is remembered, as a typed yes is, and `neomail \
         daemon` then also classifies new mail",
        "covers the daemon only after a yes to its question on a terminal",
        "--yes gives it only when there is no earlier yes for the host: a yes from before the \
         daemon classified needs the daemon's question, asked on a terminal even when there is \
         nothing to send",
    ] {
        assert!(flat.contains(part), "{part}: {flat}");
    }
}

/// `work` and an Outlook account `outlook` whose cache holds one message, with `extra` in the
/// `[classify]` table.
fn with_outlook(jev: &FakeJev, extra: &str) -> tempfile::TempDir {
    let dir = data(jev, &three()[..1]);
    write_config(
        dir.path(),
        &jev.url(),
        &format!(
            "{extra}\n[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
             oauth = {{ provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }}\n"
        ),
    );
    let mut cache = Cache::open(&dir.path().join("cache.sqlite3")).unwrap();
    let inbox = cache.reset_id_mailbox("outlook", "INBOX").unwrap();
    let listed = nm_core::IdListed {
        id: nm_core::RemoteId::new("AAA").unwrap(),
        flags: Flags::default(),
        received: Utc::now() - Duration::days(1),
    };
    let raw = &mail(
        7,
        1,
        "news@paper.example",
        "newsletters Weekly",
        "This week.",
    )
    .raw;
    cache
        .store_id_messages(inbox.id, &[(&listed, raw.as_slice())], None)
        .unwrap();
    dir
}

/// The lines about Outlook categories on standard error.
fn category_lines(o: &Output) -> Vec<String> {
    stderr(o)
        .lines()
        .filter(|l| l.contains("categor") || l.contains("Outlook"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn categories_need_the_outlook_sign_in_and_a_failed_one_is_one_line_and_exit_1() {
    let jev = FakeJev::start();
    let dir = with_outlook(&jev, "");
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(jev.count(), 2, "both accounts' mail is classified first");
    // The tests have no keyring, so the sign-in cannot be read and Graph is never reached.
    let lines = category_lines(&o);
    assert_eq!(lines.len(), 1, "{}", stderr(&o));
    assert!(
        lines[0].starts_with("neomail: outlook: categories not written: ")
            && lines[0].contains("keyring"),
        "{}",
        lines[0]
    );
    // Neither the subject nor the sender is named.
    assert!(!stderr(&o).contains("Weekly") && !stderr(&o).contains("paper.example"));
    // The results are stored, and listed without the prefix.
    let listed = groups(dir.path(), &["--account", "outlook"]);
    assert_eq!(listed[0].1, "newsletters");

    // Still due, so the next run tries again without asking jev.
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(category_lines(&o).len(), 1, "{}", stderr(&o));
    assert_eq!(jev.count(), 2);
}

#[test]
fn categories_are_not_written_when_turned_off_out_of_scope_or_estimating() {
    let jev = FakeJev::start();
    let dir = with_outlook(&jev, "");
    // Classified, and its category still due: with no keyring, every run that reaches it says so.
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert_eq!(category_lines(&o).len(), 1, "{}", stderr(&o));
    let work: Vec<String> = json(&neomail(
        dir.path(),
        &["list", "--json", "--account", "work"],
    ))
    .as_array()
    .unwrap()
    .iter()
    .map(|m| m["id"].to_string())
    .collect();
    assert_eq!(work.len(), 1);
    for args in [
        &["classify", "--estimate"][..],
        &["classify", "--yes", "--account", "work"],
        &["classify", "--yes", "--again", work[0].as_str()],
    ] {
        let o = neomail(dir.path(), args);
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert_eq!(category_lines(&o), Vec::<String>::new(), "{args:?}");
    }
    let dir = with_outlook(&jev, "labels = false\n");
    let o = neomail(dir.path(), &["classify", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(category_lines(&o), Vec::<String>::new());
}
