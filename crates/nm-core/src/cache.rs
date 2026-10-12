//! The local SQLite cache: one file holding every account's cached INBOX.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::parse::{self, EmailAddress};
use crate::types::{
    Cursor, FetchedMessage, Flags, IdListed, MessageId, RemoteId, Uid, UidFlags, UidValidity,
};

/// The schema this build writes. A cache with a newer version is refused, not guessed at; an
/// older one is migrated by [`Cache::open`].
pub const SCHEMA_VERSION: i64 = 4;

/// The tables of schema 4, named `mailbox{suffix}` and `message{suffix}`: the migration from 1
/// creates them under temporary names first. `message` refers to `mailbox` by its final name.
fn tables(suffix: &str) -> String {
    format!(
        "
CREATE TABLE mailbox{suffix} (
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
    -- By-id mailboxes only: where the source's changes stopped (Graph's delta link); NULL until
    -- a sync stores one, and whenever the next sync must list instead.
    cursor       TEXT CHECK (cursor <> ''),
    UNIQUE (account, name),
    CHECK (uid_validity IS NULL OR seen_until IS NULL)
);
CREATE TABLE message{suffix} (
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
"
    )
}

const INDEX: &str =
    "CREATE INDEX message_order ON message (COALESCE(received, date, 0) DESC, id DESC);";

/// Schema 1 to 4, after the tables of 4 exist as `mailbox_v2` and `message_v2`: copy every row
/// with its id (a mailbox as synced, as [`MIGRATE_V2`] does), drop the old tables, rename the new
/// ones and recreate the index (which went with the old `message`).
/// <https://www.sqlite.org/lang_altertable.html#otheralter>
const MIGRATE_V1_ROWS: &str = "
INSERT INTO mailbox_v2 (id, account, name, uid_validity, highest_uid, synced)
    SELECT id, account, name, uid_validity, highest_uid, 1 FROM mailbox;
INSERT INTO message_v2
    (id, mailbox_id, uid, flags, received, date, from_name, from_address, subject, raw)
    SELECT id, mailbox_id, uid, flags, received, date, from_name, from_address, subject, raw
    FROM message;
DROP TABLE message;
DROP TABLE mailbox;
ALTER TABLE mailbox_v2 RENAME TO mailbox;
ALTER TABLE message_v2 RENAME TO message;
";

/// Schema 2 to 3: `mailbox.synced`. A mailbox that already exists was synced by a build without
/// it and counts as synced; at worst, one whose first sync was interrupted before the upgrade has
/// the rest of that sync reported as new mail once.
const MIGRATE_V2: &str = "
ALTER TABLE mailbox ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;
UPDATE mailbox SET synced = 1;
";

/// Schema 3 to 4: `mailbox.cursor`. Every mailbox starts without one, so its next sync lists as
/// before and stores one.
const MIGRATE_V3: &str = "ALTER TABLE mailbox ADD COLUMN cursor TEXT CHECK (cursor <> '');";

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
    pub kind: MailboxKind,
    /// A sync of this mailbox has completed ([`Cache::mark_synced`]). `false` from
    /// [`Cache::reset_mailbox`] or [`Cache::reset_id_mailbox`] until then, so a first sync that
    /// is interrupted and finished by later syncs is a first sync throughout.
    pub synced: bool,
}

/// How a mailbox's messages are keyed, with the sync state that goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxKind {
    /// Messages keyed by UID, synced by [`sync_inbox`](crate::sync_inbox).
    Imap {
        uid_validity: UidValidity,
        /// The highest UID ever fetched; only UIDs above it are new.
        highest_uid: Option<Uid>,
    },
    /// Messages keyed by [`RemoteId`], synced by [`sync_inbox_by_id`](crate::sync_inbox_by_id).
    ById {
        /// Where the next sync's listing window must start at the latest; `None` until a sync
        /// lists something.
        seen_until: Option<DateTime<Utc>>,
    },
}

/// One row of `neomail list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub id: MessageId,
    pub account: String,
    /// `None` for a message of a by-id mailbox.
    pub uid: Option<Uid>,
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
    /// `None` for a message of a by-id mailbox.
    pub uid: Option<Uid>,
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
        conn.pragma_update(None, "journal_mode", "WAL")?;
        if schema_version(&conn)? == 0 {
            // One write transaction that reads the version again: two first opens at once cannot
            // both create the schema, and a crash cannot leave half of it.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if schema_version(&tx)? == 0 {
                tx.execute_batch(&tables(""))?;
                tx.execute_batch(INDEX)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            tx.commit()?;
        }
        if schema_version(&conn)? == 1 {
            migrate_v1(&mut conn)?;
        }
        if schema_version(&conn)? == 2 {
            migrate_step(&mut conn, 2, MIGRATE_V2)?;
        }
        if schema_version(&conn)? == 3 {
            migrate_step(&mut conn, 3, MIGRATE_V3)?;
        }
        conn.pragma_update(None, "foreign_keys", true)?;
        match schema_version(&conn)? {
            SCHEMA_VERSION => Ok(Self { conn }),
            found if found > SCHEMA_VERSION => Err(CacheError::TooNew { found }),
            _ => Err(CacheError::Corrupt("user_version")),
        }
    }

    pub fn mailbox(&self, account: &str, name: &str) -> Result<Option<MailboxRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, uid_validity, highest_uid, seen_until, synced FROM mailbox
                 WHERE account = ?1 AND name = ?2",
                params![account, name],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, validity, highest, seen_until, synced)| {
            let kind = match validity {
                Some(validity) => MailboxKind::Imap {
                    uid_validity: u32::try_from(validity)
                        .ok()
                        .and_then(UidValidity::new)
                        .ok_or(CacheError::Corrupt("uid_validity"))?,
                    highest_uid: u32::try_from(highest).ok().and_then(Uid::new),
                },
                None => MailboxKind::ById {
                    seen_until: seen_until.map(time_from_db).transpose()?,
                },
            };
            Ok(MailboxRecord { id, kind, synced })
        })
        .transpose()
    }

    /// The number of cached messages in the mailbox, of either kind.
    pub fn message_count(&self, mailbox_id: i64) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM message WHERE mailbox_id = ?1",
            [mailbox_id],
            |r| r.get(0),
        )?;
        usize::try_from(count).map_err(|_| CacheError::Corrupt("message count"))
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
            kind: MailboxKind::Imap {
                uid_validity,
                highest_uid: None,
            },
            synced: false,
        })
    }

    /// The UIDs and flags of every cached message in the mailbox, ascending by UID.
    pub fn cached_flags(&self, mailbox_id: i64) -> Result<Vec<UidFlags>> {
        let mut stmt = self.conn.prepare(
            "SELECT uid, flags FROM message WHERE mailbox_id = ?1 AND uid IS NOT NULL ORDER BY uid",
        )?;
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
        self.store_messages(mailbox_id, messages, highest).map(drop)
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
    ///
    /// Returns the ids of the rows it added; a message that was already cached keeps its id and
    /// is not among them.
    pub fn store_messages(
        &mut self,
        mailbox_id: i64,
        messages: &[FetchedMessage],
        raise_to: Option<Uid>,
    ) -> Result<Vec<MessageId>> {
        // Immediate: the read of the last issued id comes before the first write, and a deferred
        // transaction whose read went stale (another connection committed in between) fails its
        // first write at once with "database is locked", without waiting the busy timeout.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let issued = last_issued_id(&tx)?;
        let mut added = Vec::new();
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
                     subject = excluded.subject, raw = excluded.raw
                 RETURNING id",
            )?;
            for m in messages {
                let s = parse::summarize(&m.raw);
                let (from_name, from_address) =
                    s.from.map(|a| (a.name, a.address)).unwrap_or_default();
                let id: i64 = insert.query_row(
                    params![
                        mailbox_id,
                        m.uid.get(),
                        m.flags.to_storage(),
                        m.received.map(|d| d.timestamp()),
                        s.date.map(|d| d.timestamp()),
                        from_name,
                        from_address,
                        s.subject,
                        m.raw,
                    ],
                    |r| r.get(0),
                )?;
                if id > issued {
                    added.push(MessageId::new(id));
                }
            }
            if let Some(max) = raise_to {
                tx.execute(
                    "UPDATE mailbox SET highest_uid = MAX(highest_uid, ?2) WHERE id = ?1",
                    params![mailbox_id, max.get()],
                )?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Empties the mailbox (creating it if needed) as a by-id mailbox with no `seen_until`.
    pub fn reset_id_mailbox(&mut self, account: &str, name: &str) -> Result<MailboxRecord> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM mailbox WHERE account = ?1 AND name = ?2",
            params![account, name],
        )?;
        tx.execute(
            "INSERT INTO mailbox (account, name) VALUES (?1, ?2)",
            params![account, name],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(MailboxRecord {
            id,
            kind: MailboxKind::ById { seen_until: None },
            synced: false,
        })
    }

    /// The remote ids, flags and arrival times of every cached message in a by-id mailbox,
    /// ascending by id.
    pub fn cached_ids(&self, mailbox_id: i64) -> Result<Vec<IdListed>> {
        let mut stmt = self.conn.prepare(
            "SELECT remote_id, flags, received FROM message
             WHERE mailbox_id = ?1 AND remote_id IS NOT NULL ORDER BY remote_id",
        )?;
        let rows = stmt.query_map([mailbox_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, flags, received) = row?;
            Ok(IdListed {
                id: RemoteId::new(id).ok_or(CacheError::Corrupt("remote_id"))?,
                flags: Flags::from_storage(&flags),
                received: time_from_db(received.ok_or(CacheError::Corrupt("received"))?)?,
            })
        })
        .collect()
    }

    /// Applies flag changes and removals to a by-id mailbox in one transaction. Returns the ids of
    /// the rows it removed.
    pub fn apply_id_changes(
        &mut self,
        mailbox_id: i64,
        flag_updates: &[(RemoteId, Flags)],
        removed: &[RemoteId],
    ) -> Result<Vec<MessageId>> {
        let tx = self.conn.transaction()?;
        let mut gone = Vec::new();
        {
            let mut update = tx.prepare(
                "UPDATE message SET flags = ?3 WHERE mailbox_id = ?1 AND remote_id = ?2",
            )?;
            for (id, flags) in flag_updates {
                update.execute(params![mailbox_id, id.as_str(), flags.to_storage()])?;
            }
            let mut delete = tx.prepare(
                "DELETE FROM message WHERE mailbox_id = ?1 AND remote_id = ?2 RETURNING id",
            )?;
            for id in removed {
                let mut rows = delete.query(params![mailbox_id, id.as_str()])?;
                while let Some(row) = rows.next()? {
                    gone.push(MessageId::new(row.get(0)?));
                }
            }
        }
        tx.commit()?;
        Ok(gone)
    }

    /// Stores fetched messages in a by-id mailbox in one transaction, each with its flags and
    /// arrival time from the listing. A message cached already keeps its id.
    ///
    /// `missing_since`, the oldest arrival that came back without a body, lowers `seen_until` to
    /// it in the same transaction (never raises it), so a sync that fails after this batch still
    /// starts its next window at that message.
    ///
    /// Returns the ids of the rows it added, as [`store_messages`](Self::store_messages) does.
    pub fn store_id_messages(
        &mut self,
        mailbox_id: i64,
        messages: &[(&IdListed, &[u8])],
        missing_since: Option<DateTime<Utc>>,
    ) -> Result<Vec<MessageId>> {
        // Immediate: the read of the last issued id comes before the first write, and a deferred
        // transaction whose read went stale (another connection committed in between) fails its
        // first write at once with "database is locked", without waiting the busy timeout.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let issued = last_issued_id(&tx)?;
        let mut added = Vec::new();
        if let Some(missing) = missing_since {
            tx.execute(
                "UPDATE mailbox SET seen_until = MIN(COALESCE(seen_until, ?2), ?2)
                 WHERE id = ?1 AND uid_validity IS NULL",
                params![mailbox_id, missing.timestamp()],
            )?;
        }
        {
            let mut insert = tx.prepare(
                "INSERT INTO message
                 (mailbox_id, remote_id, flags, received, date, from_name, from_address, subject,
                  raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (mailbox_id, remote_id) DO UPDATE SET
                     flags = excluded.flags, received = excluded.received, date = excluded.date,
                     from_name = excluded.from_name, from_address = excluded.from_address,
                     subject = excluded.subject, raw = excluded.raw
                 RETURNING id",
            )?;
            for (listed, raw) in messages {
                let s = parse::summarize(raw);
                let (from_name, from_address) =
                    s.from.map(|a| (a.name, a.address)).unwrap_or_default();
                let id: i64 = insert.query_row(
                    params![
                        mailbox_id,
                        listed.id.as_str(),
                        listed.flags.to_storage(),
                        listed.received.timestamp(),
                        s.date.map(|d| d.timestamp()),
                        from_name,
                        from_address,
                        s.subject,
                        raw,
                    ],
                    |r| r.get(0),
                )?;
                if id > issued {
                    added.push(MessageId::new(id));
                }
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Records that a sync of the mailbox has completed: [`MailboxRecord::synced`].
    pub fn mark_synced(&mut self, mailbox_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE mailbox SET synced = 1 WHERE id = ?1", [mailbox_id])?;
        Ok(())
    }

    /// A by-id mailbox's stored [`Cursor`], if any.
    pub fn cursor(&self, mailbox_id: i64) -> Result<Option<Cursor>> {
        let cursor: Option<String> = self
            .conn
            .query_row(
                "SELECT cursor FROM mailbox WHERE id = ?1 AND uid_validity IS NULL",
                [mailbox_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        cursor
            .map(|c| Cursor::new(c).ok_or(CacheError::Corrupt("cursor")))
            .transpose()
    }

    /// Stores a by-id mailbox's [`Cursor`], or clears it so the next sync lists instead.
    pub fn set_cursor(&mut self, mailbox_id: i64, cursor: Option<&Cursor>) -> Result<()> {
        self.conn.execute(
            "UPDATE mailbox SET cursor = ?2 WHERE id = ?1 AND uid_validity IS NULL",
            params![mailbox_id, cursor.map(Cursor::as_str)],
        )?;
        Ok(())
    }

    /// Sets a by-id mailbox's `seen_until`. Stored to the second, rounded down, so the window it
    /// starts never begins after the arrival it came from.
    pub fn set_seen_until(&mut self, mailbox_id: i64, seen_until: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE mailbox SET seen_until = ?2 WHERE id = ?1 AND uid_validity IS NULL",
            params![mailbox_id, seen_until.timestamp()],
        )?;
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
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| list_entry(row?)).collect()
    }

    /// The `limit` newest of the messages `ids`, newest first as [`list`](Self::list) orders
    /// them. Ids that are not cached are left out.
    pub fn newest_of(&self, ids: &[MessageId], limit: usize) -> Result<Vec<ListEntry>> {
        // Well below SQLite's limit on parameters (32766 since 3.32).
        const CHUNK: usize = 500;
        let mut found: Vec<(i64, ListRow)> = Vec::new();
        for chunk in ids.chunks(CHUNK) {
            let marks = vec!["?"; chunk.len()].join(", ");
            let mut stmt = self.conn.prepare(&format!(
                "SELECT m.id, b.account, m.uid, COALESCE(m.date, m.received), m.from_name,
                        m.from_address, m.subject, m.flags, COALESCE(m.received, m.date, 0)
                 FROM message m JOIN mailbox b ON b.id = m.mailbox_id
                 WHERE m.id IN ({marks})"
            ))?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(chunk.iter().map(|id| id.get())),
                |r| {
                    Ok((
                        r.get::<_, i64>(8)?,
                        (
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        ),
                    ))
                },
            )?;
            for row in rows {
                found.push(row?);
            }
        }
        // ORDER BY COALESCE(received, date, 0) DESC, id DESC, as `list`.
        found.sort_by(|(a, ra), (b, rb)| (b, rb.0).cmp(&(a, ra.0)));
        found.truncate(limit);
        found.into_iter().map(|(_, row)| list_entry(row)).collect()
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
                        r.get::<_, Option<i64>>(2)?,
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
                uid: uid.map(uid_from_db).transpose()?,
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

/// Schema 1 to the current one, in one `BEGIN IMMEDIATE` transaction: any failure rolls back
/// and leaves schema 1 as it was.
fn migrate_v1(conn: &mut Connection) -> Result<()> {
    // Outside the transaction, where it takes effect
    // (https://www.sqlite.org/pragma.html#pragma_foreign_keys): with foreign keys on, dropping
    // the old mailbox table would delete every message.
    conn.pragma_update(None, "foreign_keys", false)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another process may have migrated while this one waited for the lock.
    if schema_version(&tx)? != 1 {
        return Ok(());
    }
    // Copying the rows sets the counter to the highest id left, so it would hand out again the
    // id of a newest message that was deleted. Keep the old counter instead.
    let next_after: Option<i64> = tx
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'message'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    tx.execute_batch(&tables("_v2"))?;
    tx.execute_batch(MIGRATE_V1_ROWS)?;
    if let Some(seq) = next_after {
        // No row at all when every message was deleted before the migration.
        tx.execute(
            "INSERT INTO sqlite_sequence (name, seq) SELECT 'message', 0
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'message')",
            [],
        )?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = MAX(seq, ?1) WHERE name = 'message'",
            [seq],
        )?;
    }
    tx.execute_batch(INDEX)?;
    let dangling = tx
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some();
    if dangling {
        return Err(CacheError::Corrupt("a message without its mailbox"));
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// Schema `from` to `from + 1` with `sql`, in one `BEGIN IMMEDIATE` transaction that reads the
/// version again: any failure leaves schema `from` as it was.
fn migrate_step(conn: &mut Connection, from: i64, sql: &str) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if schema_version(&tx)? != from {
        return Ok(());
    }
    tx.execute_batch(sql)?;
    tx.pragma_update(None, "user_version", from + 1)?;
    tx.commit()?;
    Ok(())
}

/// A row of `list`: id, account, uid, date, from name, from address, subject, flags.
type ListRow = (
    i64,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

fn list_entry(row: ListRow) -> Result<ListEntry> {
    let (id, account, uid, date, name, address, subject, flags) = row;
    Ok(ListEntry {
        id: MessageId::new(id),
        account,
        uid: uid.map(uid_from_db).transpose()?,
        date: date.and_then(|t| DateTime::from_timestamp(t, 0)),
        from: (name.is_some() || address.is_some()).then_some(EmailAddress { name, address }),
        subject,
        flags: Flags::from_storage(&flags),
    })
}

/// The highest message id handed out so far (AUTOINCREMENT's counter), so a store can tell the
/// rows it adds (above it) from the ones it updates. No counter yet means none was handed out.
fn last_issued_id(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'message'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

fn time_from_db(seconds: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(seconds, 0).ok_or(CacheError::Corrupt("time"))
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
        assert_eq!(
            mb.kind,
            MailboxKind::Imap {
                uid_validity: validity,
                highest_uid: Uid::new(3)
            }
        );

        let list = cache.list(None, 10).unwrap();
        let subjects: Vec<_> = list.iter().map(|e| e.subject.clone().unwrap()).collect();
        assert_eq!(subjects, ["newest", "mid", "old"]);
        assert!(list[0].flags.is_seen());
        assert_eq!(list[0].from.as_ref().unwrap().short(), "Ada");
        assert_eq!(cache.list(None, 1).unwrap().len(), 1);
        assert!(cache.list(Some("other"), 10).unwrap().is_empty());

        let got = cache.get(list[0].id).unwrap().unwrap();
        assert_eq!(got.uid, Uid::new(2));
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
            cache.mailbox("work", "INBOX").unwrap().unwrap().kind,
            MailboxKind::Imap {
                uid_validity: UidValidity::new(2).unwrap(),
                highest_uid: None
            }
        );
    }

    #[test]
    fn newest_of_picks_the_newest_of_the_given_ids_in_list_order() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        let added = cache
            .store_messages(
                mb.id,
                &[
                    msg(1, 100, "a"),
                    msg(2, 400, "b"),
                    msg(3, 300, "c"),
                    msg(4, 200, "d"),
                ],
                None,
            )
            .unwrap();
        assert_eq!(added.len(), 4);
        let subjects = |ids: &[MessageId], limit| -> Vec<String> {
            cache
                .newest_of(ids, limit)
                .unwrap()
                .into_iter()
                .map(|e| e.subject.unwrap())
                .collect()
        };
        assert_eq!(subjects(&added, 3), ["b", "c", "d"]);
        // Only the ids asked for; one that is not cached is left out.
        let gone = MessageId::new(999);
        assert_eq!(subjects(&[added[0], added[3], gone], 3), ["d", "a"]);
        assert!(subjects(&[], 3).is_empty());
        let entry = &cache.newest_of(&added[1..2], 1).unwrap()[0];
        assert_eq!(
            entry,
            &cache.list(None, 1).unwrap()[0],
            "the same entry as list"
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
        let ids = |cache: &Cache| -> Vec<(Option<Uid>, MessageId)> {
            cache
                .list(None, 100)
                .unwrap()
                .into_iter()
                .map(|e| (e.uid, e.id))
                .collect()
        };
        let before = ids(&cache);

        // Storing a cached message again keeps its id, and is not reported as added.
        let added = cache
            .store_messages(mb.id, &[msg(11, 2, "b2"), msg(14, 5, "e")], None)
            .unwrap();
        assert_eq!(ids(&cache)[1..], before[..]);
        assert_eq!(added, [ids(&cache)[0].1], "only the new row");
        cache
            .apply_changes(mb.id, &[], &[Uid::new(14).unwrap()])
            .unwrap();

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
    fn a_by_id_message_needs_its_arrival_time() {
        let cache = Cache::open_in_memory().unwrap();
        cache
            .conn
            .execute(
                "INSERT INTO mailbox (account, name) VALUES ('work', 'INBOX')",
                [],
            )
            .unwrap();
        let insert = |remote_id: &str, received: Option<i64>| {
            cache.conn.execute(
                "INSERT INTO message (mailbox_id, remote_id, flags, received, raw)
                 VALUES (1, ?1, '', ?2, x'')",
                rusqlite::params![remote_id, received],
            )
        };
        assert!(insert("AAMkAG1", None).is_err());
        assert!(insert("AAMkAG2", Some(1_760_000_000)).is_ok());
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
