//! Caches written by the v1 and v2 schemas, pinned here as they shipped, are migrated.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::path::Path;

use nm_core::cache::{MailboxKind, SCHEMA_VERSION};
use nm_core::{Cache, FetchedMessage, Flags, GroupRules, LabelPrefix, RemoteId, Uid, UidValidity};
use rusqlite::Connection;

/// The schema 1 DDL, exactly as nm-core wrote it. Never edit: it stands for caches on disk.
const V1_SCHEMA: &str = "
CREATE TABLE mailbox (
    id           INTEGER PRIMARY KEY,
    account      TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    uid_validity INTEGER NOT NULL,
    highest_uid  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account, name)
);
CREATE TABLE message (
    -- AUTOINCREMENT: a message id is never reused, even after the newest row is deleted, so an
    -- id a script saved can never point at a different message (docs/json.md).
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    mailbox_id   INTEGER NOT NULL REFERENCES mailbox (id) ON DELETE CASCADE,
    uid          INTEGER NOT NULL,
    flags        TEXT    NOT NULL,
    received     INTEGER,
    date         INTEGER,
    from_name    TEXT,
    from_address TEXT,
    subject      TEXT,
    raw          BLOB    NOT NULL,
    UNIQUE (mailbox_id, uid)
);
CREATE INDEX message_order ON message (COALESCE(received, date, 0) DESC, id DESC);
";

/// A v1 cache: messages 1 to 3 in work's INBOX, 4 in home's, and 5, the newest id, deleted.
fn v1_cache(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute_batch(V1_SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute_batch(
        "INSERT INTO mailbox (id, account, name, uid_validity, highest_uid)
             VALUES (1, 'work', 'INBOX', 7, 3), (2, 'home', 'INBOX', 9, 1);
         INSERT INTO message (mailbox_id, uid, flags, received, subject, raw) VALUES
             (1, 1, '', 100, 'one', x'00'), (1, 2, '\\Seen', 200, 'two', x'00'),
             (1, 3, '', 300, 'three', x'00'), (2, 1, '', 400, 'four', x'00'),
             (1, 4, '', 500, 'five', x'00');
         DELETE FROM message WHERE id = 5;",
    )
    .unwrap();
    conn
}

fn msg(uid: u32) -> FetchedMessage {
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::default(),
        received: None,
        raw: b"Subject: new\r\n\r\nhi\r\n".to_vec(),
    }
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn rows_and_ids_are_kept_and_the_next_id_is_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    drop(v1_cache(&path));

    let mut cache = Cache::open(&path).unwrap();
    let rows: Vec<(i64, String, Option<u32>, bool)> = cache
        .list(None, 10)
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.id.get(),
                e.account,
                e.uid.map(Uid::get),
                e.flags.is_seen(),
            )
        })
        .collect();
    let expected = [
        (4, "home", 1, false),
        (3, "work", 3, false),
        (2, "work", 2, true),
        (1, "work", 1, false),
    ];
    let expected: Vec<_> = expected
        .iter()
        .map(|(i, a, u, s)| (*i, a.to_string(), Some(*u), *s))
        .collect();
    assert_eq!(rows, expected);
    let work = cache.mailbox("work", "INBOX").unwrap().unwrap();
    assert_eq!(
        work.kind,
        MailboxKind::Imap {
            uid_validity: UidValidity::new(7).unwrap(),
            highest_uid: Uid::new(3)
        }
    );
    assert!(
        work.synced,
        "a mailbox from before `synced` counts as synced"
    );
    // Straight to the current schema, the label table included: it reads and stores.
    let now = chrono::DateTime::from_timestamp(2000, 0).unwrap();
    let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
    assert_eq!(
        cache
            .labels_due(None, None, &rules, &prefix, now, 10)
            .unwrap(),
        (vec![], 0)
    );
    let id = RemoteId::new("id-1").unwrap();
    cache.store_label("work", &id, "nm-main", now).unwrap();

    // Id 5 was deleted before the migration; it is not handed out again.
    cache.insert_messages(work.id, &[msg(4)]).unwrap();
    let newest_id = cache
        .list(None, 10)
        .unwrap()
        .iter()
        .map(|e| e.id.get())
        .max();
    assert_eq!(newest_id, Some(6));

    // Foreign keys are on: a mailbox's messages go with it, and a message needs its mailbox.
    cache
        .reset_mailbox("work", "INBOX", UidValidity::new(8).unwrap())
        .unwrap();
    assert!(cache.list(Some("work"), 10).unwrap().is_empty());
    assert_eq!(cache.list(Some("home"), 10).unwrap().len(), 1);
    assert!(cache.insert_messages(999, &[msg(1)]).is_err());
    drop(cache);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(user_version(&conn), SCHEMA_VERSION);
    let index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'message_order'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index, 1);
    // Reopening a migrated cache changes nothing.
    assert!(Cache::open(&path).is_ok());
}

#[test]
fn a_cache_whose_messages_were_all_deleted_keeps_its_counter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    v1_cache(&path).execute("DELETE FROM message", []).unwrap();

    let mut cache = Cache::open(&path).unwrap();
    let work = cache.mailbox("work", "INBOX").unwrap().unwrap();
    cache.insert_messages(work.id, &[msg(9)]).unwrap();
    assert_eq!(cache.list(None, 1).unwrap()[0].id.get(), 6);
}

#[test]
fn a_failed_migration_leaves_a_working_v1_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let conn = v1_cache(&path);
    // A message whose mailbox is gone fails the foreign key check, after every table has been
    // copied, dropped and renamed.
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "INSERT INTO message (mailbox_id, uid, flags, raw) VALUES (99, 1, '', x'00')",
        [],
    )
    .unwrap();
    drop(conn);

    assert!(Cache::open(&path).is_err());
    assert!(
        Cache::open(&path).is_err(),
        "the second try finds the same v1 cache"
    );

    let conn = Connection::open(&path).unwrap();
    assert_eq!(user_version(&conn), 1);
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tables, ["mailbox", "message", "sqlite_sequence"]);
    let ddl: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'message'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ddl.contains("uid          INTEGER NOT NULL"), "{ddl}");
    let (count, seq): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM message),
                    (SELECT seq FROM sqlite_sequence WHERE name = 'message')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, seq), (5, 6));
    // An old neomail can still write to it.
    conn.execute(
        "INSERT INTO message (mailbox_id, uid, flags, raw) VALUES (1, 9, '', x'00')",
        [],
    )
    .unwrap();
}

/// The schema 2 tables, exactly as nm-core wrote them. Never edit: they stand for caches on disk.
const V2_SCHEMA: &str = "
CREATE TABLE mailbox (
    id           INTEGER PRIMARY KEY,
    account      TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    -- NULL for a by-id mailbox, whose messages are keyed by remote_id instead of uid.
    uid_validity INTEGER,
    highest_uid  INTEGER NOT NULL DEFAULT 0,
    -- By-id mailboxes only: the arrival time the next sync's window must reach back to.
    seen_until   INTEGER,
    UNIQUE (account, name),
    CHECK (uid_validity IS NULL OR seen_until IS NULL)
);
CREATE TABLE message (
    -- AUTOINCREMENT: a message id is never reused, even after the newest row is deleted, so an
    -- id a script saved can never point at a different message (docs/json.md).
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    mailbox_id   INTEGER NOT NULL REFERENCES mailbox (id) ON DELETE CASCADE,
    -- Exactly one of uid (IMAP) and remote_id (by-id sources such as Microsoft Graph).
    uid          INTEGER,
    remote_id    TEXT CHECK (remote_id <> ''),
    flags        TEXT    NOT NULL,
    -- Required for by-id rows: their engine windows on the arrival time.
    received     INTEGER CHECK (remote_id IS NULL OR received IS NOT NULL),
    date         INTEGER,
    from_name    TEXT,
    from_address TEXT,
    subject      TEXT,
    raw          BLOB    NOT NULL,
    UNIQUE (mailbox_id, uid),
    UNIQUE (mailbox_id, remote_id),
    CHECK ((uid IS NULL) <> (remote_id IS NULL))
);
CREATE INDEX message_order ON message (COALESCE(received, date, 0) DESC, id DESC);
";

#[test]
fn a_v2_cache_is_migrated_with_its_mailboxes_synced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(V2_SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 2).unwrap();
    conn.execute_batch(
        "INSERT INTO mailbox (id, account, name, uid_validity, highest_uid, seen_until)
             VALUES (1, 'work', 'INBOX', 7, 3, NULL), (2, 'home', 'INBOX', NULL, 0, 1000);
         INSERT INTO message (mailbox_id, uid, flags, received, subject, raw)
             VALUES (1, 1, '', 100, 'one', x'00');",
    )
    .unwrap();
    drop(conn);

    let mut cache = Cache::open(&path).unwrap();
    let work = cache.mailbox("work", "INBOX").unwrap().unwrap();
    let home = cache.mailbox("home", "INBOX").unwrap().unwrap();
    assert!(work.synced && home.synced);
    assert_eq!(cache.list(Some("work"), 10).unwrap().len(), 1);
    // A mailbox created after the migration is not synced until a sync completes.
    let reset = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(8).unwrap())
        .unwrap();
    assert!(!reset.synced);
    assert!(!cache.mailbox("work", "INBOX").unwrap().unwrap().synced);
    cache.mark_synced(reset.id).unwrap();
    assert!(cache.mailbox("work", "INBOX").unwrap().unwrap().synced);
    drop(cache);

    assert_eq!(
        user_version(&Connection::open(&path).unwrap()),
        SCHEMA_VERSION
    );
    // Reopening a migrated cache changes nothing.
    assert!(Cache::open(&path).is_ok());
}

/// The schema 3 tables, exactly as nm-core wrote them. Never edit: they stand for caches on disk.
const V3_MAILBOX: &str = "
CREATE TABLE mailbox (
    id           INTEGER PRIMARY KEY,
    account      TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    -- NULL for a by-id mailbox, whose messages are keyed by remote_id instead of uid.
    uid_validity INTEGER,
    highest_uid  INTEGER NOT NULL DEFAULT 0,
    -- By-id mailboxes only: the arrival time the next sync's window must reach back to.
    seen_until   INTEGER,
    -- 1 once a sync of this mailbox has completed. Until then its new messages are old mail of a
    -- first sync (or a rebuild) that has not finished, not new mail.
    synced       INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account, name),
    CHECK (uid_validity IS NULL OR seen_until IS NULL)
);
";

#[test]
fn a_v3_cache_is_migrated_without_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let conn = Connection::open(&path).unwrap();
    // Schema 3's message table and index are schema 2's.
    let message = &V2_SCHEMA[V2_SCHEMA.find("CREATE TABLE message").unwrap()..];
    conn.execute_batch(V3_MAILBOX).unwrap();
    conn.execute_batch(message).unwrap();
    conn.pragma_update(None, "user_version", 3).unwrap();
    conn.execute_batch(
        "INSERT INTO mailbox (id, account, name, uid_validity, highest_uid, seen_until, synced)
             VALUES (1, 'work', 'INBOX', 7, 3, NULL, 1), (2, 'home', 'INBOX', NULL, 0, 1000, 1);
         INSERT INTO message (mailbox_id, remote_id, flags, received, subject, raw)
             VALUES (2, 'AAMk1=', '', 1000, 'one', x'00');",
    )
    .unwrap();
    drop(conn);

    let mut cache = Cache::open(&path).unwrap();
    let home = cache.mailbox("home", "INBOX").unwrap().unwrap();
    assert!(home.synced);
    assert_eq!(cache.cursor(home.id).unwrap(), None, "the next sync lists");
    assert_eq!(cache.list(Some("home"), 10).unwrap().len(), 1);
    let cursor = nm_core::Cursor::new("https://graph.microsoft.com/v1.0/x?$deltatoken=a").unwrap();
    cache.set_cursor(home.id, Some(&cursor)).unwrap();
    assert_eq!(cache.cursor(home.id).unwrap(), Some(cursor));
    drop(cache);

    assert_eq!(
        user_version(&Connection::open(&path).unwrap()),
        SCHEMA_VERSION
    );
    // Reopening keeps the cursor.
    let cache = Cache::open(&path).unwrap();
    assert!(cache.cursor(home.id).unwrap().is_some());
}

#[test]
fn a_v4_cache_gains_msg_key_and_the_classification_table_without_parsing_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let conn = Connection::open(&path).unwrap();
    // Schema 4 is schema 3 plus mailbox.cursor.
    let message = &V2_SCHEMA[V2_SCHEMA.find("CREATE TABLE message").unwrap()..];
    conn.execute_batch(V3_MAILBOX).unwrap();
    conn.execute_batch(message).unwrap();
    conn.execute_batch("ALTER TABLE mailbox ADD COLUMN cursor TEXT CHECK (cursor <> '');")
        .unwrap();
    conn.pragma_update(None, "user_version", 4).unwrap();
    conn.execute_batch(
        "INSERT INTO mailbox (id, account, name, uid_validity, highest_uid, synced)
             VALUES (1, 'work', 'INBOX', 7, 1, 1);
         INSERT INTO message (mailbox_id, uid, flags, received, subject, raw)
             VALUES (1, 1, '', 1000, 'one', CAST('Message-ID: <one@mail.example>' AS BLOB));",
    )
    .unwrap();
    drop(conn);

    let mut cache = Cache::open(&path).unwrap();
    let pending = cache.pending(None, false).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].msg_key, None, "the migration parses no row");
    cache
        .set_msg_key(pending[0].id, "one@mail.example")
        .unwrap();
    let result = nm_core::Classification {
        choice: nm_core::Choice::Main,
        confidence: 0.8,
        probabilities: vec![(nm_core::Choice::Main, 0.8)],
        model: "jev-1.13.0".into(),
        request_version: 1,
        input_tokens: None,
        classified_at: chrono::DateTime::from_timestamp(2000, 0).unwrap(),
    };
    cache
        .store_classification("work", "one@mail.example", &result)
        .unwrap();
    assert!(cache.pending(None, false).unwrap().is_empty());
    drop(cache);

    assert_eq!(user_version(&Connection::open(&path).unwrap()), 6);
    assert_eq!(SCHEMA_VERSION, 6);
    let cache = Cache::open(&path).unwrap();
    assert_eq!(
        cache.classification("work", "one@mail.example").unwrap(),
        Some(result)
    );
}
