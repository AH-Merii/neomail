//! The local SQLite cache: one file holding every account's cached INBOX.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::parse::{self, EmailAddress};
use crate::types::{FetchedMessage, Flags, MessageId, Uid, UidFlags, UidValidity};

/// The schema this build writes. A cache with a newer version is refused, not guessed at.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
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

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "the cache uses schema version {found}, newer than this neomail ({SCHEMA_VERSION}); upgrade neomail or remove the cache"
    )]
    TooNew { found: i64 },
    #[error("the cache holds an invalid value: {0}")]
    Corrupt(&'static str),
}

pub type Result<T, E = CacheError> = std::result::Result<T, E>;

/// A cached mailbox's sync state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxRecord {
    pub id: i64,
    pub uid_validity: UidValidity,
    /// The highest UID ever fetched; only UIDs above it are new.
    pub highest_uid: Option<Uid>,
}

/// One row of `neomail list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub id: MessageId,
    pub account: String,
    pub uid: Uid,
    pub date: Option<DateTime<Utc>>,
    pub from: Option<EmailAddress>,
    pub subject: Option<String>,
    pub flags: Flags,
}

/// A cached message with its source, for `neomail show`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub id: MessageId,
    pub account: String,
    pub uid: Uid,
    pub flags: Flags,
    pub received: Option<DateTime<Utc>>,
    pub raw: Vec<u8>,
}

pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Opens (creating if needed) the cache file at `path`. The parent directory must exist.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A throwaway cache, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        if schema_version(&conn)? == 0 {
            // One write transaction that reads the version again: two first opens at once cannot
            // both create the schema, and a crash cannot leave half of it.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if schema_version(&tx)? == 0 {
                tx.execute_batch(SCHEMA)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            tx.commit()?;
        }
        match schema_version(&conn)? {
            SCHEMA_VERSION => Ok(Self { conn }),
            found => Err(CacheError::TooNew { found }),
        }
    }

    pub fn mailbox(&self, account: &str, name: &str) -> Result<Option<MailboxRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, uid_validity, highest_uid FROM mailbox WHERE account = ?1 AND name = ?2",
                params![account, name],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
            )
            .optional()?;
        row.map(|(id, validity, highest)| {
            Ok(MailboxRecord {
                id,
                uid_validity: u32::try_from(validity)
                    .ok()
                    .and_then(UidValidity::new)
                    .ok_or(CacheError::Corrupt("uid_validity"))?,
                highest_uid: u32::try_from(highest).ok().and_then(Uid::new),
            })
        })
        .transpose()
    }

    /// Empties the mailbox (creating it if needed) and records a new UIDVALIDITY.
    pub fn reset_mailbox(
        &mut self,
        account: &str,
        name: &str,
        uid_validity: UidValidity,
    ) -> Result<MailboxRecord> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM mailbox WHERE account = ?1 AND name = ?2",
            params![account, name],
        )?;
        tx.execute(
            "INSERT INTO mailbox (account, name, uid_validity) VALUES (?1, ?2, ?3)",
            params![account, name, uid_validity.get()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(MailboxRecord {
            id,
            uid_validity,
            highest_uid: None,
        })
    }

    /// The UIDs and flags of every cached message in the mailbox, ascending by UID.
    pub fn cached_flags(&self, mailbox_id: i64) -> Result<Vec<UidFlags>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uid, flags FROM message WHERE mailbox_id = ?1 ORDER BY uid")?;
        let rows = stmt.query_map([mailbox_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (uid, flags) = row?;
            Ok(UidFlags {
                uid: uid_from_db(uid)?,
                flags: Flags::from_storage(&flags),
            })
        })
        .collect()
    }

    /// Applies flag changes and removals in one transaction.
    pub fn apply_changes(
        &mut self,
        mailbox_id: i64,
        flag_updates: &[UidFlags],
        removed: &[Uid],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut update =
                tx.prepare("UPDATE message SET flags = ?3 WHERE mailbox_id = ?1 AND uid = ?2")?;
            for u in flag_updates {
                update.execute(params![mailbox_id, u.uid.get(), u.flags.to_storage()])?;
            }
            let mut delete =
                tx.prepare("DELETE FROM message WHERE mailbox_id = ?1 AND uid = ?2")?;
            for uid in removed {
                delete.execute(params![mailbox_id, uid.get()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Stores fetched messages and raises the mailbox's highest UID to theirs, in one transaction.
    pub fn insert_messages(&mut self, mailbox_id: i64, messages: &[FetchedMessage]) -> Result<()> {
        let highest = messages.iter().map(|m| m.uid).max();
        self.store_messages(mailbox_id, messages, highest)
    }

    /// Raises the mailbox's highest UID to `uid` (never lowers it): UIDs up to it are not new.
    pub fn raise_highest_uid(&mut self, mailbox_id: i64, uid: Uid) -> Result<()> {
        self.conn.execute(
            "UPDATE mailbox SET highest_uid = MAX(highest_uid, ?2) WHERE id = ?1",
            params![mailbox_id, uid.get()],
        )?;
        Ok(())
    }

    /// Stores fetched messages and, when `raise_to` is given, raises the mailbox's highest UID to
    /// it (never lowers it), in one transaction. A first sync passes `None`, because the messages
    /// it picks need not be the highest UIDs on the server.
    pub fn store_messages(
        &mut self,
        mailbox_id: i64,
        messages: &[FetchedMessage],
        raise_to: Option<Uid>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                // An upsert, not INSERT OR REPLACE: REPLACE deletes the row and inserts a new
                // one, which would give an already cached message a new id.
                "INSERT INTO message
                 (mailbox_id, uid, flags, received, date, from_name, from_address, subject, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (mailbox_id, uid) DO UPDATE SET
                     flags = excluded.flags, received = excluded.received, date = excluded.date,
                     from_name = excluded.from_name, from_address = excluded.from_address,
                     subject = excluded.subject, raw = excluded.raw",
            )?;
            for m in messages {
                let s = parse::summarize(&m.raw);
                let (from_name, from_address) =
                    s.from.map(|a| (a.name, a.address)).unwrap_or_default();
                insert.execute(params![
                    mailbox_id,
                    m.uid.get(),
                    m.flags.to_storage(),
                    m.received.map(|d| d.timestamp()),
                    s.date.map(|d| d.timestamp()),
                    from_name,
                    from_address,
                    s.subject,
                    m.raw,
                ])?;
            }
            if let Some(max) = raise_to {
                tx.execute(
                    "UPDATE mailbox SET highest_uid = MAX(highest_uid, ?2) WHERE id = ?1",
                    params![mailbox_id, max.get()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cached messages, newest first, from one account or all of them.
    pub fn list(&self, account: Option<&str>, limit: u32) -> Result<Vec<ListEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, b.account, m.uid, COALESCE(m.date, m.received), m.from_name,
                    m.from_address, m.subject, m.flags
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id
             WHERE ?1 IS NULL OR b.account = ?1
             ORDER BY COALESCE(m.received, m.date, 0) DESC, m.id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![account, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (id, account, uid, date, name, address, subject, flags) = row?;
            Ok(ListEntry {
                id: MessageId::new(id),
                account,
                uid: uid_from_db(uid)?,
                date: date.and_then(|t| DateTime::from_timestamp(t, 0)),
                from: (name.is_some() || address.is_some())
                    .then_some(EmailAddress { name, address }),
                subject,
                flags: Flags::from_storage(&flags),
            })
        })
        .collect()
    }

    /// The accounts that have a cached mailbox, sorted.
    pub fn accounts(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT account FROM mailbox ORDER BY account")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get(&self, id: MessageId) -> Result<Option<StoredMessage>> {
        let row = self
            .conn
            .query_row(
                "SELECT m.id, b.account, m.uid, m.flags, m.received, m.raw
                 FROM message m JOIN mailbox b ON b.id = m.mailbox_id WHERE m.id = ?1",
                [id.get()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, account, uid, flags, received, raw)| {
            Ok(StoredMessage {
                id: MessageId::new(id),
                account,
                uid: uid_from_db(uid)?,
                flags: Flags::from_storage(&flags),
                received: received.and_then(|t| DateTime::from_timestamp(t, 0)),
                raw,
            })
        })
        .transpose()
    }
}

fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn uid_from_db(value: i64) -> Result<Uid> {
    u32::try_from(value)
        .ok()
        .and_then(Uid::new)
        .ok_or(CacheError::Corrupt("uid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(uid: u32, received: i64, subject: &str) -> FetchedMessage {
        FetchedMessage {
            uid: Uid::new(uid).unwrap(),
            flags: Flags::new(if uid.is_multiple_of(2) {
                vec![Flags::SEEN]
            } else {
                vec![]
            }),
            received: DateTime::from_timestamp(received, 0),
            raw: format!("From: Ada <ada@mail.example>\r\nSubject: {subject}\r\n\r\nhi\r\n")
                .into_bytes(),
        }
    }

    #[test]
    fn stores_lists_newest_first_and_gets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite3");
        let validity = UidValidity::new(42).unwrap();
        {
            let mut cache = Cache::open(&path).unwrap();
            let mb = cache.reset_mailbox("work", "INBOX", validity).unwrap();
            cache
                .insert_messages(
                    mb.id,
                    &[
                        msg(1, 100, "old"),
                        msg(2, 300, "newest"),
                        msg(3, 200, "mid"),
                    ],
                )
                .unwrap();
        }
        // Reopening keeps everything.
        let cache = Cache::open(&path).unwrap();
        let mb = cache.mailbox("work", "INBOX").unwrap().unwrap();
        assert_eq!(mb.uid_validity, validity);
        assert_eq!(mb.highest_uid, Uid::new(3));

        let list = cache.list(None, 10).unwrap();
        let subjects: Vec<_> = list.iter().map(|e| e.subject.clone().unwrap()).collect();
        assert_eq!(subjects, ["newest", "mid", "old"]);
        assert!(list[0].flags.is_seen());
        assert_eq!(list[0].from.as_ref().unwrap().short(), "Ada");
        assert_eq!(cache.list(None, 1).unwrap().len(), 1);
        assert!(cache.list(Some("other"), 10).unwrap().is_empty());

        let got = cache.get(list[0].id).unwrap().unwrap();
        assert_eq!(got.uid, Uid::new(2).unwrap());
        assert!(got.raw.starts_with(b"From: Ada"));
        assert!(cache.get(MessageId::new(999)).unwrap().is_none());
    }

    #[test]
    fn reset_drops_messages_and_changes_apply() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(1, 1, "a"), msg(3, 3, "b")])
            .unwrap();
        let seen = UidFlags {
            uid: Uid::new(1).unwrap(),
            flags: Flags::new([Flags::SEEN]),
        };
        cache
            .apply_changes(mb.id, std::slice::from_ref(&seen), &[Uid::new(3).unwrap()])
            .unwrap();
        assert_eq!(cache.cached_flags(mb.id).unwrap(), vec![seen]);

        let mb2 = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(2).unwrap())
            .unwrap();
        assert!(cache.cached_flags(mb2.id).unwrap().is_empty());
        assert_eq!(
            cache.mailbox("work", "INBOX").unwrap().unwrap().highest_uid,
            None
        );
    }

    #[test]
    fn ids_are_stable_and_never_reused() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(10, 1, "a"), msg(11, 2, "b"), msg(12, 3, "c")])
            .unwrap();
        let ids = |cache: &Cache| -> Vec<(Uid, MessageId)> {
            cache
                .list(None, 100)
                .unwrap()
                .into_iter()
                .map(|e| (e.uid, e.id))
                .collect()
        };
        let before = ids(&cache);

        // Storing a cached message again keeps its id.
        cache
            .store_messages(mb.id, &[msg(11, 2, "b2")], None)
            .unwrap();
        assert_eq!(ids(&cache), before);

        // Expunge the newest-inserted row, then add new mail: the new id is a fresh one.
        cache
            .apply_changes(mb.id, &[], &[Uid::new(12).unwrap()])
            .unwrap();
        cache.insert_messages(mb.id, &[msg(13, 4, "d")]).unwrap();
        let new_id = cache.list(None, 1).unwrap()[0].id;
        assert!(before.iter().all(|(_, id)| *id != new_id));

        // The same holds across a UIDVALIDITY reset.
        let mb2 = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(2).unwrap())
            .unwrap();
        cache.insert_messages(mb2.id, &[msg(10, 5, "e")]).unwrap();
        let reset_id = cache.list(None, 1).unwrap()[0].id;
        assert!(reset_id != new_id && before.iter().all(|(_, id)| *id != reset_id));
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite3");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(Cache::open(&path), Err(CacheError::TooNew { .. })));
    }
}
