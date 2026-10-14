//! The local SQLite cache: one file holding every account's cached INBOX.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::group::{Choice, Group, GroupRules};
use crate::label::LabelPrefix;
use crate::parse::{self, EmailAddress};
use crate::search::{self, IndexText, Query, Scope};
use crate::types::{
    Cursor, FetchedMessage, Flags, IdListed, MessageId, RemoteId, Uid, UidFlags, UidValidity,
};

/// The schema this build writes. A cache with a newer version is refused, not guessed at; an
/// older one is migrated by [`Cache::open`].
pub const SCHEMA_VERSION: i64 = 9;

/// The `mailbox` and `message` tables of schema 5, named `mailbox{suffix}` and `message{suffix}`: the migration from 1
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
    -- What a classification result is stored under (parse::msg_key). Filled on insert; NULL for
    -- a row cached before schema 5 until `neomail classify` reaches it.
    msg_key      TEXT CHECK (msg_key <> ''),
    UNIQUE (mailbox_id, uid),
    UNIQUE (mailbox_id, remote_id),
    CHECK ((uid IS NULL) <> (remote_id IS NULL))
);
"
    )
}

/// Classification results (schema 5). Keyed by account and [`parse::msg_key`], not by
/// `message.id`: rebuilding a mailbox deletes its messages and gives them new ids, and a result
/// must outlive that so the same mail is never classified (and paid for) twice.
const CLASSIFICATION: &str = "
CREATE TABLE classification (
    account         TEXT    NOT NULL,
    msg_key         TEXT    NOT NULL CHECK (msg_key <> ''),
    -- The classifier's choice: one of group::Choice's names.
    choice          TEXT    NOT NULL,
    confidence      REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    -- Every option's probability, a JSON object.
    probabilities   TEXT    NOT NULL,
    -- The model id the answer named.
    model           TEXT    NOT NULL,
    request_version INTEGER NOT NULL,
    input_tokens    INTEGER,
    classified_at   INTEGER NOT NULL,
    PRIMARY KEY (account, msg_key)
);
";

/// The name neomail last wrote on a message on its server (schema 6): an Outlook category, a
/// Gmail label or a Proton label. On Outlook it is keyed by the server's id, not by `msg_key`:
/// two copies of one mail (the same Message-ID in two folders) are two items, and each is written
/// on its own. Graph's ids are immutable ids, which a rebuild of the mailbox lists again, so the
/// row outlives it as the result does. Over IMAP a UID changes with a rebuild or a move, so the
/// column holds the `msg_key` instead (an account is one or the other, never both). What is
/// still to write is worked out from it ([`Cache::labels_due`]): no queue.
const LABEL: &str = "
CREATE TABLE label (
    account    TEXT    NOT NULL,
    remote_id  TEXT    NOT NULL CHECK (remote_id <> ''),
    -- Graph's id, or over IMAP the message's msg_key (despite the column's name).
    -- The whole name as written, prefix included (label::LabelPrefix::name).
    name       TEXT    NOT NULL CHECK (name <> ''),
    written_at INTEGER NOT NULL,
    PRIMARY KEY (account, remote_id)
);
";

/// Moves of confident spam to Junk (schema 7; DEC-2026-10-14-labels-junk). A row is written
/// before the move is sent, so a message is moved at most once even when neomail stops between
/// the server's answer and the record: `moved_at` stays NULL until the server confirms, and a
/// row whose move was never confirmed counts as made once the message it was made for
/// (`message_id`, never reused) has left the cache. Keyed by `msg_key`, so the record outlives
/// the message's row: the user moving it back to the INBOX brings it back as a new row, which is
/// never moved again. Apart from the label table, because a failed or skipped move must stay due
/// after its label is written.
const JUNK: &str = "
CREATE TABLE junk (
    account     TEXT    NOT NULL,
    msg_key     TEXT    NOT NULL CHECK (msg_key <> ''),
    message_id  INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    moved_at    INTEGER,
    PRIMARY KEY (account, msg_key)
);
";

/// Writes and moves that failed for one message, and messages its server no longer has (schema
/// 9; DEC-2026-10-14-label-retries), so neither costs a request on every pass. Keyed by
/// `message.id`, never reused: the row goes with the message when a sync, a rebuild or a move
/// to Junk deletes it, and a message brought back is a new row, tried at once. `want` is the
/// name that was due when it failed: a message whose group or prefix changed is due at once.
/// `retry_at` is when it is due again, NULL for a message the server no longer has.
const LABEL_RETRY: &str = "
CREATE TABLE label_retry (
    message_id INTEGER PRIMARY KEY REFERENCES message (id) ON DELETE CASCADE,
    want       TEXT    NOT NULL,
    failures   INTEGER NOT NULL CHECK (failures >= 0),
    retry_at   INTEGER
);
";

/// The wait after a message's `failures`-th write or move in a row that failed: 15 minutes,
/// doubled with each failure, at most a day (DEC-2026-10-14-label-retries).
pub fn label_retry_wait(failures: u32) -> chrono::TimeDelta {
    const FIRST_MINUTES: i64 = 15;
    const CAP: chrono::TimeDelta = chrono::TimeDelta::days(1);
    let doublings = failures.saturating_sub(1).min(16);
    chrono::TimeDelta::minutes(FIRST_MINUTES << doublings).min(CAP)
}

/// How a message's write or move ended, for [`Cache::label_tried`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tried {
    /// Everything due on it was done (or it waits only for a later pass): due as usual.
    Done,
    /// A write or move failed for this message alone: due again after [`label_retry_wait`].
    Failed,
    /// Its server no longer has it: not due again while its row stays.
    Gone,
}

const INDEX: &str =
    "CREATE INDEX message_order ON message (COALESCE(received, date, 0) DESC, id DESC);";

/// Schema 1 to 5, after the tables of 5 exist as `mailbox_v2` and `message_v2`: copy every row
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

/// Schema 4 to 5: `message.msg_key` and the [`CLASSIFICATION`] table. No row is parsed here:
/// `neomail classify` fills the key of an older row when it reaches it.
fn migrate_v4() -> String {
    format!("ALTER TABLE message ADD COLUMN msg_key TEXT CHECK (msg_key <> '');\n{CLASSIFICATION}")
}

/// Schema 5 to 6: the [`LABEL`] table. Every existing row stays; no message has a label yet, so
/// every classified Outlook message is due once.
const MIGRATE_V5: &str = LABEL;

/// Schema 6 to 7: the [`JUNK`] table. Every existing row stays; no message has moved yet, so
/// every confident spam message is due a move once, 10c's included.
const MIGRATE_V6: &str = JUNK;

/// The full-text index (schema 8): `message_fts`, a contentless FTS5 table whose rowid is
/// `message.id`, so it keeps no second copy of the text (the raw message is in `message`); and
/// per message `indexed`, the [`search::TEXT_VERSION`] it was indexed under (NULL until then),
/// and `thread`, its thread's key. Deleting a message deletes its index row, a cascade from its
/// mailbox included. Search always joins `message`, so a row that is gone is never found.
/// <https://www.sqlite.org/fts5.html#contentless_delete_tables>
const SEARCH: &str = "
ALTER TABLE message ADD COLUMN indexed INTEGER;
ALTER TABLE message ADD COLUMN thread TEXT;
CREATE INDEX message_thread ON message (thread);
CREATE VIRTUAL TABLE message_fts USING fts5 (
    subject, sender, recipients, body,
    content = '', contentless_delete = 1, tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER message_fts_delete AFTER DELETE ON message BEGIN
    DELETE FROM message_fts WHERE rowid = old.id;
END;
";

/// Schema 7 to 8: [`SEARCH`]. No row is parsed here: every row starts unindexed, and
/// [`Cache::index_pending`] indexes them.
const MIGRATE_V7: &str = SEARCH;

/// Schema 8 to 9: the [`LABEL_RETRY`] table. Every existing row stays; nothing has failed yet.
const MIGRATE_V8: &str = LABEL_RETRY;

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

/// Which messages [`Cache::list_grouped`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupFilter {
    Any,
    /// Messages whose effective group is this one.
    Is(Group),
    /// Messages with no stored classification.
    Unclassified,
}

impl GroupFilter {
    fn admits(self, group: Option<Group>) -> bool {
        match self {
            GroupFilter::Any => true,
            GroupFilter::Is(g) => group == Some(g),
            GroupFilter::Unclassified => group.is_none(),
        }
    }
}

/// One row of `neomail list` with its group: `None` until the message is classified.
#[derive(Debug, Clone, PartialEq)]
pub struct Grouped {
    pub entry: ListEntry,
    pub group: Option<Group>,
}

/// A message `neomail classify` may ask about: [`Cache::pending`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub id: MessageId,
    pub account: String,
    /// `None` for a row cached before schema 5; [`Cache::set_msg_key`] records it.
    pub msg_key: Option<String>,
    /// When the message arrived (else its Date header), for the window.
    pub received: Option<DateTime<Utc>>,
}

/// A stored classification result: the classifier's answer for one message, as it gave it. The
/// group it lists under is derived from it on reading ([`GroupRules::effective`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub choice: Choice,
    /// The classifier's confidence, 0 to 1.
    pub confidence: f64,
    /// Each option's probability, in [`Choice::ALL`] order.
    pub probabilities: Vec<(Choice, f64)>,
    /// The model id the answer named.
    pub model: String,
    /// The request's version (`REQUEST_VERSION` in `crates/neomail/src/jev.rs`).
    pub request_version: u32,
    pub input_tokens: Option<u64>,
    pub classified_at: DateTime<Utc>,
}

/// What a pass asks [`Cache::labels_due`] for: labels (or categories) whose name is not the
/// wanted one, Junk moves of confident spam, or both. An account whose server takes no labels or
/// has no Junk folder asks only for the other, so what it cannot do never fills its cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueFor {
    pub labels: bool,
    pub junk: bool,
}

/// What [`Cache::labels_due`] found for a pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelsDue {
    /// The due messages, newest first: at most the limit.
    pub due: Vec<LabelDue>,
    /// How many more are due past the limit.
    pub more: usize,
    /// How many would be due but are held back after a failed write or move, until their
    /// `retry_at` ([`Cache::label_tried`]). A message its server no longer has is not counted.
    pub held: usize,
}

/// A classified message with something to write on its server: [`Cache::labels_due`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDue {
    pub id: MessageId,
    pub account: String,
    pub key: MessageKey,
    /// What its result is stored under ([`parse::msg_key`]): over IMAP, also what its label row
    /// is stored under, and the Message-ID a Proton label's removal searches for.
    pub msg_key: String,
    /// The effective group's name with the prefix: what the message should carry.
    pub want: String,
    /// What neomail wrote last, `None` before the first write.
    pub last: Option<String>,
    /// Whether `want` is still to write.
    pub label: bool,
    /// Whether the message is due its move to Junk.
    pub junk: bool,
}

impl LabelDue {
    /// What the message's label row is stored under ([`Cache::store_label`]): Graph's id, or the
    /// `msg_key` over IMAP, whose UIDs change with a move or a rebuild.
    pub fn item(&self) -> &str {
        match &self.key {
            MessageKey::Remote(id) => id.as_str(),
            MessageKey::Uid { .. } => &self.msg_key,
        }
    }
}

/// Where a cached message lives, for an action to reach it on its server: [`Cache::locate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub id: MessageId,
    pub account: String,
    pub flags: Flags,
    pub key: MessageKey,
}

/// How a message is named on its server. A UID is only meaningful with its mailbox's
/// UIDVALIDITY, so the two travel together; a by-id message has neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageKey {
    Uid { uid_validity: UidValidity, uid: Uid },
    Remote(RemoteId),
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
                tx.execute_batch(CLASSIFICATION)?;
                tx.execute_batch(LABEL)?;
                tx.execute_batch(JUNK)?;
                tx.execute_batch(LABEL_RETRY)?;
                tx.execute_batch(INDEX)?;
                tx.execute_batch(SEARCH)?;
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
        if schema_version(&conn)? == 4 {
            migrate_step(&mut conn, 4, &migrate_v4())?;
        }
        if schema_version(&conn)? == 5 {
            migrate_step(&mut conn, 5, MIGRATE_V5)?;
        }
        if schema_version(&conn)? == 6 {
            migrate_step(&mut conn, 6, MIGRATE_V6)?;
        }
        if schema_version(&conn)? == 7 {
            migrate_step(&mut conn, 7, MIGRATE_V7)?;
        }
        if schema_version(&conn)? == 8 {
            migrate_step(&mut conn, 8, MIGRATE_V8)?;
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
                 (mailbox_id, uid, flags, received, date, from_name, from_address, subject, raw,
                  msg_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (mailbox_id, uid) DO UPDATE SET
                     flags = excluded.flags, received = excluded.received, date = excluded.date,
                     from_name = excluded.from_name, from_address = excluded.from_address,
                     subject = excluded.subject, raw = excluded.raw, msg_key = excluded.msg_key,
                     indexed = NULL
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
                        parse::msg_key(s.message_id.as_deref(), &m.raw),
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
                  raw, msg_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (mailbox_id, remote_id) DO UPDATE SET
                     flags = excluded.flags, received = excluded.received, date = excluded.date,
                     from_name = excluded.from_name, from_address = excluded.from_address,
                     subject = excluded.subject, raw = excluded.raw, msg_key = excluded.msg_key,
                     indexed = NULL
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
                        parse::msg_key(s.message_id.as_deref(), raw),
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

    /// Where each of `ids` lives, for the cached ones, in the order asked. Ids that are not
    /// cached are left out.
    pub fn locate(&self, ids: &[MessageId]) -> Result<Vec<Located>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.account, b.uid_validity, m.uid, m.remote_id, m.flags
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id WHERE m.id = ?1",
        )?;
        let mut found = Vec::new();
        for &id in ids {
            let row = stmt
                .query_row([id.get()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .optional()?;
            let Some((account, validity, uid, remote_id, flags)) = row else {
                continue;
            };
            let key = match (validity, uid, remote_id) {
                (Some(validity), Some(uid), None) => MessageKey::Uid {
                    uid_validity: u32::try_from(validity)
                        .ok()
                        .and_then(UidValidity::new)
                        .ok_or(CacheError::Corrupt("uid_validity"))?,
                    uid: uid_from_db(uid)?,
                },
                (None, None, Some(remote_id)) => MessageKey::Remote(
                    RemoteId::new(remote_id).ok_or(CacheError::Corrupt("remote_id"))?,
                ),
                _ => return Err(CacheError::Corrupt("a message's key")),
            };
            found.push(Located {
                id,
                account,
                flags: Flags::from_storage(&flags),
                key,
            });
        }
        Ok(found)
    }

    /// Sets the flags of cached messages by id, in one transaction. Ids that are not cached are
    /// skipped.
    pub fn set_flags(&mut self, updates: &[(MessageId, Flags)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut update = tx.prepare("UPDATE message SET flags = ?2 WHERE id = ?1")?;
            for (id, flags) in updates {
                update.execute(params![id.get(), flags.to_storage()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes cached messages by id, in one transaction, as a sync does for a message gone from
    /// the INBOX. Ids that are not cached are skipped. A removed id is never handed out again
    /// (AUTOINCREMENT).
    pub fn remove(&mut self, ids: &[MessageId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut delete = tx.prepare("DELETE FROM message WHERE id = ?1")?;
            for id in ids {
                delete.execute([id.get()])?;
            }
        }
        tx.commit()?;
        Ok(())
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

    /// Cached messages with their [`Group`], newest first, from one account or all of them,
    /// keeping only those `filter` admits. The group is worked out here, by
    /// [`GroupRules::effective`], so thresholds can change without asking the classifier again.
    pub fn list_grouped(
        &self,
        account: Option<&str>,
        filter: GroupFilter,
        limit: u32,
        rules: &GroupRules,
        now: DateTime<Utc>,
    ) -> Result<Vec<Grouped>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, b.account, m.uid, COALESCE(m.date, m.received), m.from_name,
                    m.from_address, m.subject, m.flags, COALESCE(m.received, m.date),
                    c.choice, c.confidence
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id
             LEFT JOIN classification c ON c.account = b.account AND c.msg_key = m.msg_key
             WHERE ?1 IS NULL OR b.account = ?1
             ORDER BY COALESCE(m.received, m.date, 0) DESC, m.id DESC",
        )?;
        let rows = stmt.query(params![account])?;
        collect_grouped(rows, filter, limit, rules, now)
    }

    /// Indexes up to `batch` messages that are not indexed under the current
    /// [`search::TEXT_VERSION`] (new, upserted, or cached before schema 7), in one transaction.
    /// Returns how many it indexed: 0 once nothing is left. A stopped run keeps the batches it
    /// committed.
    pub fn index_pending(&mut self, batch: usize) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows: Vec<(i64, Vec<u8>)> = tx
            .prepare("SELECT id, raw FROM message WHERE indexed IS NOT ?1 LIMIT ?2")?
            .query_map(
                params![
                    search::TEXT_VERSION,
                    i64::try_from(batch).unwrap_or(i64::MAX)
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
            .collect::<Result<_, _>>()?;
        {
            let mut forget = tx.prepare("DELETE FROM message_fts WHERE rowid = ?1")?;
            let mut add = tx.prepare(
                "INSERT INTO message_fts (rowid, subject, sender, recipients, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            let mut done =
                tx.prepare("UPDATE message SET indexed = ?2, thread = ?3 WHERE id = ?1")?;
            for (id, raw) in &rows {
                let text = IndexText::of(raw);
                forget.execute([id])?;
                add.execute(params![
                    id,
                    text.subject,
                    text.sender,
                    text.recipients,
                    text.body
                ])?;
                done.execute(params![id, search::TEXT_VERSION, text.thread])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// How many messages [`Self::index_pending`] has still to index.
    pub fn unindexed_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM message WHERE indexed IS NOT ?1",
            [search::TEXT_VERSION],
            |r| r.get(0),
        )?;
        usize::try_from(count).map_err(|_| CacheError::Corrupt("message count"))
    }

    /// The indexed messages in `scope` that match `query`, with their [`Group`], newest first as
    /// [`Self::list_grouped`] orders and groups them: at most `limit`. An empty query matches
    /// every message in scope. A message not yet indexed ([`Self::index_pending`]) is not found;
    /// nor is any message when the scope's thread message is not cached.
    pub fn search(
        &self,
        query: &Query,
        scope: &Scope,
        limit: u32,
        rules: &GroupRules,
        now: DateTime<Utc>,
    ) -> Result<Vec<Grouped>> {
        // The text condition only when there is text: FTS5 refuses an empty MATCH.
        let text = query.fts();
        let matching = if text.is_some() {
            "AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?3)"
        } else {
            "AND ?3 IS NULL"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT m.id, b.account, m.uid, COALESCE(m.date, m.received), m.from_name,
                    m.from_address, m.subject, m.flags, COALESCE(m.received, m.date),
                    c.choice, c.confidence
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id
             LEFT JOIN classification c ON c.account = b.account AND c.msg_key = m.msg_key
             WHERE m.indexed IS NOT NULL
               AND (?1 IS NULL OR b.account = ?1)
               AND (?2 IS NULL OR (b.account, m.thread) = (
                   SELECT tb.account, tm.thread FROM message tm
                   JOIN mailbox tb ON tb.id = tm.mailbox_id WHERE tm.id = ?2))
               {matching}
             ORDER BY COALESCE(m.received, m.date, 0) DESC, m.id DESC"
        ))?;
        let rows = stmt.query(params![
            scope.account,
            scope.thread.map(MessageId::get),
            text
        ])?;
        collect_grouped(rows, scope.group, limit, rules, now)
    }

    /// Every cached message with no stored classification (with `again`, every message), from
    /// one account or all of them, newest first. A message cached before schema 5 has no
    /// [`msg_key`](Pending::msg_key) yet and counts as unclassified: no result can name it.
    pub fn pending(&self, account: Option<&str>, again: bool) -> Result<Vec<Pending>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, b.account, m.msg_key, COALESCE(m.received, m.date)
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id
             LEFT JOIN classification c ON c.account = b.account AND c.msg_key = m.msg_key
             WHERE (?1 IS NULL OR b.account = ?1) AND (?2 OR c.msg_key IS NULL)
             ORDER BY COALESCE(m.received, m.date, 0) DESC, m.id DESC",
        )?;
        let rows = stmt.query_map(params![account, again], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (id, account, msg_key, received) = row?;
            Ok(Pending {
                id: MessageId::new(id),
                account,
                msg_key,
                received: received.and_then(|t| DateTime::from_timestamp(t, 0)),
            })
        })
        .collect()
    }

    /// Records a message's [`msg_key`](parse::msg_key), for a row cached before schema 5.
    pub fn set_msg_key(&mut self, id: MessageId, msg_key: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE message SET msg_key = ?2 WHERE id = ?1",
            params![id.get(), msg_key],
        )?;
        Ok(())
    }

    /// The stored result for `account`'s message `msg_key`, if any.
    pub fn classification(&self, account: &str, msg_key: &str) -> Result<Option<Classification>> {
        let row = self
            .conn
            .query_row(
                "SELECT choice, confidence, probabilities, model, request_version, input_tokens,
                        classified_at
                 FROM classification WHERE account = ?1 AND msg_key = ?2",
                params![account, msg_key],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(choice, confidence, probabilities, model, version, tokens, at)| {
                let probabilities: BTreeMap<String, f64> = serde_json::from_str(&probabilities)
                    .map_err(|_| CacheError::Corrupt("probabilities"))?;
                let mut probabilities = probabilities
                    .into_iter()
                    .map(|(name, p)| Choice::from_name(&name).map(|c| (c, p)))
                    .collect::<Option<Vec<_>>>()
                    .ok_or(CacheError::Corrupt("probabilities"))?;
                probabilities.sort_by_key(|(c, _)| *c);
                Ok(Classification {
                    choice: Choice::from_name(&choice).ok_or(CacheError::Corrupt("choice"))?,
                    confidence,
                    probabilities,
                    model,
                    request_version: u32::try_from(version)
                        .map_err(|_| CacheError::Corrupt("request_version"))?,
                    input_tokens: tokens
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| CacheError::Corrupt("input_tokens"))?,
                    classified_at: time_from_db(at)?,
                })
            },
        )
        .transpose()
    }

    /// Stores a result for `account`'s message `msg_key`, replacing one stored before (only
    /// `neomail classify --again` asks again).
    pub fn store_classification(
        &mut self,
        account: &str,
        msg_key: &str,
        result: &Classification,
    ) -> Result<()> {
        let probabilities: BTreeMap<&str, f64> = result
            .probabilities
            .iter()
            .map(|(c, p)| (c.name(), *p))
            .collect();
        let probabilities = serde_json::to_string(&probabilities)
            .map_err(|_| CacheError::Corrupt("probabilities"))?;
        self.conn.execute(
            "INSERT OR REPLACE INTO classification
             (account, msg_key, choice, confidence, probabilities, model, request_version,
              input_tokens, classified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account,
                msg_key,
                result.choice.name(),
                result.confidence,
                probabilities,
                result.model,
                result.request_version,
                result
                    .input_tokens
                    .map(i64::try_from)
                    .transpose()
                    .ok()
                    .flatten(),
                result.classified_at.timestamp(),
            ],
        )?;
        Ok(())
    }

    /// The classified messages with something to write on their server, newest first: at most
    /// `limit`, how many more are due, and how many are held back after a failure. From one account or all of them, and only the
    /// messages `ids` when given.
    ///
    /// A label is due (`for_.labels`) when the wanted name, `prefix` and the effective group
    /// worked out now ([`GroupRules::effective`]), differs from the one stored by
    /// [`Self::store_label`]. A Junk move is due (`for_.junk`) apart from that, for spam
    /// [`GroupRules::junk`] moves: when no move is recorded for its `msg_key`, or one was
    /// recorded for this very row and never confirmed ([`Self::junk_attempt`]). So a move that
    /// failed after its label was written is still due, and a message the user brought back is
    /// not.
    ///
    /// Neither is due while [`Self::label_tried`] holds it back: a message its server no longer
    /// has, or one whose last write or move failed, until its `retry_at` passes or the name it
    /// should carry changes.
    #[allow(clippy::too_many_arguments)] // The pass's scope, rules and cap, each used once.
    pub fn labels_due(
        &self,
        account: Option<&str>,
        ids: Option<&[MessageId]>,
        rules: &GroupRules,
        prefix: &LabelPrefix,
        for_: DueFor,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<LabelsDue> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, b.account, m.remote_id, m.uid, b.uid_validity,
                    COALESCE(m.received, m.date), m.msg_key, c.choice, c.confidence, l.name,
                    j.msg_key IS NOT NULL, j.moved_at IS NULL AND j.message_id = m.id,
                    r.message_id IS NOT NULL, r.want, r.retry_at
             FROM message m JOIN mailbox b ON b.id = m.mailbox_id
             JOIN classification c ON c.account = b.account AND c.msg_key = m.msg_key
             LEFT JOIN label l ON l.account = b.account
                 AND l.remote_id = COALESCE(m.remote_id, m.msg_key)
             LEFT JOIN junk j ON j.account = b.account AND j.msg_key = m.msg_key
             LEFT JOIN label_retry r ON r.message_id = m.id
             WHERE ?1 IS NULL OR b.account = ?1
             ORDER BY COALESCE(m.received, m.date, 0) DESC, m.id DESC",
        )?;
        let mut rows = stmt.query(params![account])?;
        let mut found = LabelsDue::default();
        while let Some(r) = rows.next()? {
            let id = MessageId::new(r.get(0)?);
            if ids.is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            let received: Option<i64> = r.get(5)?;
            let choice =
                Choice::from_name(&r.get::<_, String>(7)?).ok_or(CacheError::Corrupt("choice"))?;
            let confidence: f64 = r.get(8)?;
            let group = rules.effective(
                choice,
                confidence,
                received.and_then(|t| DateTime::from_timestamp(t, 0)),
                now,
            );
            let want = prefix.name(group);
            let last: Option<String> = r.get(9)?;
            let label = for_.labels && last.as_deref() != Some(want.as_str());
            let (recorded, unconfirmed_here): (bool, Option<bool>) = (r.get(10)?, r.get(11)?);
            let junk = for_.junk
                && rules.junk(choice, confidence)
                && (!recorded || unconfirmed_here == Some(true));
            if !label && !junk {
                continue;
            }
            let (held, failed_want, retry_at): (bool, Option<String>, Option<i64>) =
                (r.get(12)?, r.get(13)?, r.get(14)?);
            match retry_at {
                // Gone from its server.
                None if held => continue,
                Some(at)
                    if failed_want.as_deref() == Some(want.as_str()) && at > now.timestamp() =>
                {
                    found.held += 1;
                    continue;
                }
                _ => {}
            }
            if found.due.len() == limit {
                found.more += 1;
                continue;
            }
            let key = match (r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?) {
                (Some(remote), _) => MessageKey::Remote(
                    RemoteId::new(remote).ok_or(CacheError::Corrupt("remote_id"))?,
                ),
                (None, Some(uid)) => MessageKey::Uid {
                    uid_validity: r
                        .get::<_, Option<i64>>(4)?
                        .and_then(|v| u32::try_from(v).ok())
                        .and_then(UidValidity::new)
                        .ok_or(CacheError::Corrupt("uid_validity"))?,
                    uid: uid_from_db(uid)?,
                },
                (None, None) => return Err(CacheError::Corrupt("a message without a key")),
            };
            found.due.push(LabelDue {
                id,
                account: r.get(1)?,
                key,
                msg_key: r.get(6)?,
                want,
                last,
                label,
                junk,
            });
        }
        Ok(found)
    }

    /// Records that `account`'s item `item` ([`LabelDue::item`]) carries `name` on its server,
    /// as written at `at`.
    pub fn store_label(
        &mut self,
        account: &str,
        item: &str,
        name: &str,
        at: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO label (account, remote_id, name, written_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![account, item, name, at.timestamp()],
        )?;
        Ok(())
    }

    /// Records how a write or move for the message `id`, due to carry `want`, ended at `at`:
    /// [`Tried::Done`] clears what held it back, [`Tried::Failed`] holds it back for
    /// [`label_retry_wait`] of its failures in a row (counted afresh when `want` changed), and
    /// [`Tried::Gone`] for as long as its row stays.
    pub fn label_tried(
        &mut self,
        id: MessageId,
        want: &str,
        tried: Tried,
        at: DateTime<Utc>,
    ) -> Result<()> {
        match tried {
            Tried::Done => {
                self.conn
                    .execute("DELETE FROM label_retry WHERE message_id = ?1", [id.get()])?;
            }
            Tried::Gone => {
                self.conn.execute(
                    "INSERT OR REPLACE INTO label_retry (message_id, want, failures, retry_at)
                     VALUES (?1, ?2, 0, NULL)",
                    params![id.get(), want],
                )?;
            }
            Tried::Failed => {
                let before: Option<u32> = self
                    .conn
                    .query_row(
                        "SELECT failures FROM label_retry WHERE message_id = ?1 AND want = ?2",
                        params![id.get(), want],
                        |r| r.get(0),
                    )
                    .optional()?;
                let failures = before.unwrap_or(0).saturating_add(1);
                let retry_at = at + label_retry_wait(failures);
                self.conn.execute(
                    "INSERT OR REPLACE INTO label_retry (message_id, want, failures, retry_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id.get(), want, failures, retry_at.timestamp()],
                )?;
            }
        }
        Ok(())
    }

    /// Records, before the move is sent, that the message `id`, stored under `msg_key`, is
    /// moving to Junk. A move already confirmed stays as it is.
    pub fn junk_attempt(
        &mut self,
        account: &str,
        msg_key: &str,
        id: MessageId,
        at: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO junk (account, msg_key, message_id, recorded_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (account, msg_key) DO UPDATE
                 SET message_id = excluded.message_id, recorded_at = excluded.recorded_at
                 WHERE moved_at IS NULL",
            params![account, msg_key, id.get(), at.timestamp()],
        )?;
        Ok(())
    }

    /// Records that the server confirmed the move [`Self::junk_attempt`] recorded, or that the
    /// message had left the INBOX already: either way it is never moved again.
    pub fn junk_moved(&mut self, account: &str, msg_key: &str, at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE junk SET moved_at = ?3
             WHERE account = ?1 AND msg_key = ?2 AND moved_at IS NULL",
            params![account, msg_key, at.timestamp()],
        )?;
        Ok(())
    }

    /// Drops a recorded move that was never sent (the account has no Junk folder or no MOVE),
    /// so it is due again once the account can move.
    pub fn junk_not_sent(&mut self, account: &str, msg_key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM junk WHERE account = ?1 AND msg_key = ?2 AND moved_at IS NULL",
            params![account, msg_key],
        )?;
        Ok(())
    }

    /// The average `input_tokens` of the stored results that report it, for the estimate.
    pub fn average_input_tokens(&self) -> Result<Option<f64>> {
        Ok(self.conn.query_row(
            "SELECT AVG(input_tokens) FROM classification WHERE input_tokens IS NOT NULL",
            [],
            |r| r.get(0),
        )?)
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
    tx.execute_batch(CLASSIFICATION)?;
    tx.execute_batch(LABEL)?;
    tx.execute_batch(JUNK)?;
    tx.execute_batch(LABEL_RETRY)?;
    tx.execute_batch(INDEX)?;
    tx.execute_batch(SEARCH)?;
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

/// The first `limit` of `rows` that `filter` admits, each with its effective group. `rows` are
/// list's eight columns, then the arrival time, the stored choice and its confidence.
fn collect_grouped(
    mut rows: rusqlite::Rows<'_>,
    filter: GroupFilter,
    limit: u32,
    rules: &GroupRules,
    now: DateTime<Utc>,
) -> Result<Vec<Grouped>> {
    let mut found = Vec::new();
    while found.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
        let Some(r) = rows.next()? else { break };
        let received: Option<i64> = r.get(8)?;
        let group = match (r.get::<_, Option<String>>(9)?, r.get::<_, Option<f64>>(10)?) {
            (Some(choice), Some(confidence)) => Some(rules.effective(
                Choice::from_name(&choice).ok_or(CacheError::Corrupt("choice"))?,
                confidence,
                received.and_then(|t| DateTime::from_timestamp(t, 0)),
                now,
            )),
            _ => None,
        };
        if !filter.admits(group) {
            continue;
        }
        let entry = list_entry((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ))?;
        found.push(Grouped { entry, group });
    }
    Ok(found)
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

    /// Drops what search added (schema 8); the caller drops the rest and sets the version.
    const UNDO_SEARCH: &str = "
        DROP TRIGGER message_fts_delete;
        DROP TABLE message_fts;
        DROP INDEX message_thread;
        ALTER TABLE message DROP COLUMN thread;
        ALTER TABLE message DROP COLUMN indexed;
    ";

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

    fn result(choice: Choice, confidence: f64) -> Classification {
        Classification {
            choice,
            confidence,
            probabilities: vec![(Choice::Main, 1.0 - confidence), (choice, confidence)],
            model: "jev-1.13.0".into(),
            request_version: 1,
            input_tokens: Some(300),
            classified_at: DateTime::from_timestamp(1_000, 0).unwrap(),
        }
    }

    fn groups(
        cache: &Cache,
        filter: GroupFilter,
        rules: &GroupRules,
    ) -> Vec<(String, Option<Group>)> {
        let now = DateTime::from_timestamp(100 * 86_400, 0).unwrap();
        cache
            .list_grouped(None, filter, 100, rules, now)
            .unwrap()
            .into_iter()
            .map(|g| (g.entry.subject.unwrap(), g.group))
            .collect()
    }

    fn key_of(cache: &Cache, subject: &str) -> (String, String) {
        let p = cache
            .pending(None, true)
            .unwrap()
            .into_iter()
            .find(|p| {
                cache
                    .get(p.id)
                    .unwrap()
                    .unwrap()
                    .raw
                    .ends_with(format!("{subject}\r\n\r\nhi\r\n").as_bytes())
            })
            .unwrap();
        (p.account, p.msg_key.unwrap())
    }

    #[test]
    fn a_result_survives_a_rebuild_of_the_mailbox() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(1, 1, "a"), msg(2, 2, "b")])
            .unwrap();
        let (account, key) = key_of(&cache, "a");
        cache
            .store_classification(&account, &key, &result(Choice::Newsletters, 0.9))
            .unwrap();
        let rules = GroupRules::default();
        assert_eq!(
            groups(&cache, GroupFilter::Any, &rules),
            [("b".into(), None), ("a".into(), Some(Group::Newsletters))]
        );

        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(2).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(7, 1, "a"), msg(8, 2, "b")])
            .unwrap();
        assert_eq!(
            groups(&cache, GroupFilter::Any, &rules),
            [("b".into(), None), ("a".into(), Some(Group::Newsletters))]
        );
        let pending = cache.pending(None, false).unwrap();
        assert_eq!(pending.len(), 1, "only b is left to classify");
        assert_eq!(
            cache.pending(None, true).unwrap().len(),
            2,
            "--again takes both"
        );
        assert_eq!(
            cache.classification("work", &key).unwrap(),
            Some(result(Choice::Newsletters, 0.9))
        );
        assert_eq!(
            cache.classification("home", &key).unwrap(),
            None,
            "results are per account"
        );
    }

    #[test]
    fn list_grouped_filters_on_the_effective_group() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        let day = 86_400;
        cache
            .insert_messages(
                mb.id,
                &[
                    msg(1, day, "old"),
                    msg(2, 50 * day, "unsure"),
                    msg(3, 99 * day, "new"),
                    msg(4, 99 * day, "none"),
                ],
            )
            .unwrap();
        for (subject, choice, confidence) in [
            ("old", Choice::FollowUp, 0.9),
            ("unsure", Choice::Promotions, 0.3),
            ("new", Choice::FollowUp, 0.9),
        ] {
            let (account, key) = key_of(&cache, subject);
            cache
                .store_classification(&account, &key, &result(choice, confidence))
                .unwrap();
        }
        let rules = GroupRules::default();
        let subjects = |filter| -> Vec<String> {
            groups(&cache, filter, &rules)
                .into_iter()
                .map(|(s, _)| s)
                .collect()
        };
        assert_eq!(subjects(GroupFilter::Is(Group::FollowUpOld)), ["old"]);
        assert_eq!(subjects(GroupFilter::Is(Group::FollowUp)), ["new"]);
        assert_eq!(subjects(GroupFilter::Is(Group::Other)), ["unsure"]);
        assert_eq!(
            subjects(GroupFilter::Is(Group::Promotions)),
            Vec::<String>::new()
        );
        assert_eq!(subjects(GroupFilter::Unclassified), ["none"]);
        let loose = GroupRules {
            other_below: 0.2,
            follow_up_old_days: 365,
            ..GroupRules::default()
        };
        assert_eq!(
            groups(&cache, GroupFilter::Any, &loose)
                .into_iter()
                .filter_map(|(_, g)| g)
                .collect::<Vec<_>>(),
            [Group::FollowUp, Group::Promotions, Group::FollowUp],
            "new thresholds apply without a new answer"
        );
        let now = DateTime::from_timestamp(100 * day, 0).unwrap();
        let limited = cache
            .list_grouped(None, GroupFilter::Is(Group::FollowUpOld), 1, &rules, now)
            .unwrap();
        assert_eq!(limited.len(), 1, "the limit counts kept rows only");
    }

    #[test]
    fn a_row_without_a_msg_key_is_pending_until_classify_records_its_key() {
        let mut cache = Cache::open_in_memory().unwrap();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache.insert_messages(mb.id, &[msg(1, 1, "a")]).unwrap();
        let (_, key) = key_of(&cache, "a");
        cache
            .store_classification("work", &key, &result(Choice::Main, 0.9))
            .unwrap();
        cache
            .conn
            .execute("UPDATE message SET msg_key = NULL", [])
            .unwrap();
        let pending = cache.pending(Some("work"), false).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].msg_key, None);
        cache.set_msg_key(pending[0].id, &key).unwrap();
        assert!(cache.pending(Some("work"), false).unwrap().is_empty());
        assert_eq!(cache.average_input_tokens().unwrap(), Some(300.0));
    }

    const DAY: i64 = 86_400;

    fn day(n: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(n * DAY, 0).unwrap()
    }

    /// An Outlook (by-id) message `subject` received on day `received`, classified as `choice`
    /// unless `None`.
    fn outlook(
        cache: &mut Cache,
        account: &str,
        subject: &str,
        received: i64,
        choice: Option<(Choice, f64)>,
    ) -> MessageId {
        let mb = match cache.mailbox(account, "INBOX").unwrap() {
            Some(mb) => mb,
            None => cache.reset_id_mailbox(account, "INBOX").unwrap(),
        };
        let listed = IdListed {
            id: RemoteId::new(format!("id-{subject}")).unwrap(),
            flags: Flags::default(),
            received: day(received),
        };
        let raw = format!("From: Ada <ada@mail.example>\r\nSubject: {subject}\r\n\r\nhi\r\n");
        let ids = cache
            .store_id_messages(mb.id, &[(&listed, raw.as_bytes())], None)
            .unwrap();
        if let Some((choice, confidence)) = choice {
            let (account, key) = key_of(cache, subject);
            cache
                .store_classification(&account, &key, &result(choice, confidence))
                .unwrap();
        }
        ids[0]
    }

    const BOTH: DueFor = DueFor {
        labels: true,
        junk: true,
    };

    fn due(
        cache: &Cache,
        rules: &GroupRules,
        prefix: &LabelPrefix,
        now: i64,
        limit: usize,
    ) -> (Vec<(String, String, Option<String>)>, usize) {
        let LabelsDue { due, more, .. } = cache
            .labels_due(None, None, rules, prefix, BOTH, day(now), limit)
            .unwrap();
        let due = due
            .into_iter()
            .map(|d| (d.item().to_owned(), d.want, d.last))
            .collect();
        (due, more)
    }

    fn written(cache: &mut Cache, subject: &str, name: &str) {
        let (account, _) = key_of(cache, subject);
        let id = RemoteId::new(format!("id-{subject}")).unwrap();
        cache
            .store_label(&account, id.as_str(), name, day(1))
            .unwrap();
    }

    #[test]
    fn classified_outlook_mail_is_due_newest_first_until_its_label_is_written() {
        let mut cache = Cache::open_in_memory().unwrap();
        let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
        outlook(&mut cache, "out", "a", 1, Some((Choice::Main, 0.9)));
        let b = outlook(&mut cache, "out", "b", 2, Some((Choice::Spam, 0.1)));
        outlook(&mut cache, "out", "c", 3, Some((Choice::Newsletters, 0.9)));
        outlook(&mut cache, "out", "d", 4, None);

        let entry = |id: &str, want: &str| (id.to_owned(), want.to_owned(), None);
        assert_eq!(
            due(&cache, &rules, &prefix, 10, 10),
            (
                vec![
                    entry("id-c", "nm-newsletters"),
                    entry("id-b", "nm-spam"),
                    entry("id-a", "nm-main"),
                ],
                0
            )
        );
        assert_eq!(
            due(&cache, &rules, &prefix, 10, 2),
            (
                vec![entry("id-c", "nm-newsletters"), entry("id-b", "nm-spam")],
                1
            ),
            "the limit leaves the rest counted"
        );
        written(&mut cache, "c", "nm-newsletters");
        assert_eq!(
            due(&cache, &rules, &prefix, 10, 10).0,
            [entry("id-b", "nm-spam"), entry("id-a", "nm-main")]
        );
        let LabelsDue {
            due: only, more, ..
        } = cache
            .labels_due(Some("out"), Some(&[b]), &rules, &prefix, BOTH, day(10), 10)
            .unwrap();
        assert_eq!((only.len(), only[0].item(), more), (1, "id-b", 0));
    }

    #[test]
    fn a_label_is_due_again_when_the_effective_group_changes_and_only_then() {
        let mut cache = Cache::open_in_memory().unwrap();
        let rules = GroupRules {
            other_below: 0.4,
            follow_up_old_days: 30,
            ..GroupRules::default()
        };
        let prefix = LabelPrefix::default();
        outlook(&mut cache, "out", "a", 1, Some((Choice::FollowUp, 0.6)));
        written(&mut cache, "a", "nm-follow-up");
        assert_eq!(due(&cache, &rules, &prefix, 31, 10).0, []);
        let again = |want: &str| {
            vec![(
                "id-a".to_owned(),
                want.to_owned(),
                Some("nm-follow-up".to_owned()),
            )]
        };
        // By age.
        assert_eq!(
            due(&cache, &rules, &prefix, 32, 10).0,
            again("nm-follow-up-old")
        );
        // By a new threshold.
        let strict = GroupRules {
            other_below: 0.7,
            ..rules
        };
        assert_eq!(due(&cache, &strict, &prefix, 31, 10).0, again("nm-other"));
        // By a new prefix.
        let neo = LabelPrefix::new("neo-").unwrap();
        assert_eq!(due(&cache, &rules, &neo, 31, 10).0, again("neo-follow-up"));
    }

    #[test]
    fn a_written_label_survives_a_rebuild_of_the_mailbox() {
        let mut cache = Cache::open_in_memory().unwrap();
        let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
        outlook(&mut cache, "out", "a", 1, Some((Choice::Main, 0.9)));
        written(&mut cache, "a", "nm-main");
        cache.reset_id_mailbox("out", "INBOX").unwrap();
        // The same mail as a new row: its result is found by msg_key, its label by the
        // immutable id Graph lists again.
        outlook(&mut cache, "out", "a", 1, None);
        assert_eq!(
            cache
                .labels_due(None, None, &rules, &prefix, BOTH, day(2), 10)
                .unwrap(),
            LabelsDue::default()
        );
        assert_eq!(cache.list(None, 10).unwrap().len(), 1);
    }

    #[test]
    fn two_copies_of_one_mail_are_each_due_until_each_is_written() {
        let mut cache = Cache::open_in_memory().unwrap();
        let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
        // The same Message-ID in two folders: one msg_key, so one result, but two items.
        let raw = b"Message-ID: <same@acme.example>\r\nSubject: copy\r\n\r\nhi\r\n";
        for (folder, id) in [("INBOX", "id-x"), ("Archive", "id-y")] {
            let mb = cache.reset_id_mailbox("out", folder).unwrap();
            let listed = IdListed {
                id: RemoteId::new(id).unwrap(),
                flags: Flags::default(),
                received: day(1),
            };
            cache
                .store_id_messages(mb.id, &[(&listed, raw.as_slice())], None)
                .unwrap();
        }
        let (account, key) = key_of(&cache, "copy");
        cache
            .store_classification(&account, &key, &result(Choice::Main, 0.9))
            .unwrap();
        let ids = |cache: &Cache| -> Vec<String> {
            let mut ids: Vec<String> = due(cache, &rules, &prefix, 2, 10)
                .0
                .into_iter()
                .map(|d| d.0)
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(&cache), ["id-x", "id-y"]);
        // The write to id-y failed: only id-x is recorded, and id-y stays due.
        let x = RemoteId::new("id-x").unwrap();
        cache
            .store_label("out", x.as_str(), "nm-main", day(2))
            .unwrap();
        assert_eq!(ids(&cache), ["id-y"]);
    }

    #[test]
    fn a_schema_5_cache_gains_the_label_table_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite3");
        {
            let mut cache = Cache::open(&path).unwrap();
            outlook(&mut cache, "out", "a", 1, Some((Choice::Newsletters, 0.9)));
            outlook(&mut cache, "out", "b", 2, None);
        }
        // Schema 5 is this schema without the label, junk and label_retry tables.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(UNDO_SEARCH).unwrap();
        conn.execute_batch(
            "DROP TABLE label_retry; DROP TABLE label; DROP TABLE junk; PRAGMA user_version = 5;",
        )
        .unwrap();
        drop(conn);

        let mut cache = Cache::open(&path).unwrap();
        assert_eq!(schema_version(&cache.conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(cache.list(None, 10).unwrap().len(), 2);
        let (account, key) = key_of(&cache, "a");
        assert_eq!(
            cache.classification(&account, &key).unwrap(),
            Some(result(Choice::Newsletters, 0.9))
        );
        let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
        assert_eq!(
            due(&cache, &rules, &prefix, 3, 10).0,
            [("id-a".to_owned(), "nm-newsletters".to_owned(), None)],
            "mail classified before 10c is due once"
        );
        written(&mut cache, "a", "nm-newsletters");
        assert_eq!(due(&cache, &rules, &prefix, 3, 10).0, []);
    }

    /// The due messages as (item, label due, move due), asking for both.
    fn due_for(cache: &Cache, rules: &GroupRules, for_: DueFor) -> Vec<(String, bool, bool)> {
        let prefix = LabelPrefix::default();
        let due = cache
            .labels_due(None, None, rules, &prefix, for_, day(10), 10)
            .unwrap()
            .due;
        due.into_iter()
            .map(|d| (d.item().to_owned(), d.label, d.junk))
            .collect()
    }

    #[test]
    fn imap_mail_is_due_by_its_msg_key_and_a_rebuild_keeps_what_was_written() {
        let mut cache = Cache::open_in_memory().unwrap();
        let (rules, prefix) = (GroupRules::default(), LabelPrefix::default());
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(1, 5 * DAY, "e")])
            .unwrap();
        let (account, key) = key_of(&cache, "e");
        cache
            .store_classification(&account, &key, &result(Choice::Main, 0.9))
            .unwrap();
        let LabelsDue { due, more, .. } = cache
            .labels_due(None, None, &rules, &prefix, BOTH, day(10), 10)
            .unwrap();
        assert_eq!((due.len(), more), (1, 0));
        assert_eq!(
            due[0].key,
            MessageKey::Uid {
                uid_validity: UidValidity::new(1).unwrap(),
                uid: Uid::new(1).unwrap()
            }
        );
        assert_eq!((due[0].item(), due[0].msg_key.as_str()), (&*key, &*key));
        assert!(due[0].label && !due[0].junk);
        cache
            .store_label(&account, due[0].item(), "nm-main", day(10))
            .unwrap();
        // A rebuild gives the message a new UID: what was written still stands.
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(2).unwrap())
            .unwrap();
        cache
            .insert_messages(mb.id, &[msg(7, 5 * DAY, "e")])
            .unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), []);
    }

    #[test]
    fn confident_spam_is_due_a_move_apart_from_its_label_until_the_move_is_confirmed() {
        let mut cache = Cache::open_in_memory().unwrap();
        let rules = GroupRules::default();
        let a = outlook(&mut cache, "out", "a", 1, Some((Choice::Spam, 0.95)));
        outlook(&mut cache, "out", "b", 2, Some((Choice::Spam, 0.6)));
        outlook(&mut cache, "out", "c", 3, Some((Choice::Main, 0.99)));
        let entry = |id: &str, label, junk| (id.to_owned(), label, junk);
        assert_eq!(
            due_for(&cache, &rules, BOTH),
            [
                entry("id-c", true, false),
                entry("id-b", true, false),
                entry("id-a", true, true)
            ],
            "only spam at or above junk_at moves"
        );
        let off = GroupRules {
            junk_at: None,
            ..rules
        };
        assert!(
            due_for(&cache, &off, BOTH).iter().all(|d| !d.2),
            "junk = false"
        );
        // Its label written, the move is still due: one that failed is tried again.
        for (subject, name) in [("a", "nm-spam"), ("b", "nm-spam"), ("c", "nm-main")] {
            written(&mut cache, subject, name);
        }
        assert_eq!(due_for(&cache, &rules, BOTH), [entry("id-a", false, true)]);
        let labels = DueFor {
            labels: true,
            junk: false,
        };
        assert_eq!(due_for(&cache, &rules, labels), [], "a server without Junk");
        // Recorded before it is sent: still due until the server confirms it.
        let (account, key) = key_of(&cache, "a");
        cache.junk_attempt(&account, &key, a, day(3)).unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), [entry("id-a", false, true)]);
        cache.junk_moved(&account, &key, day(3)).unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), []);
        // A confirmed move stays confirmed.
        cache.junk_attempt(&account, &key, a, day(4)).unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), []);
    }

    #[test]
    fn a_message_brought_back_is_never_moved_again_unless_its_move_was_never_sent() {
        let mut cache = Cache::open_in_memory().unwrap();
        let rules = GroupRules::default();
        let mb = cache
            .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
            .unwrap();
        cache.insert_messages(mb.id, &[msg(1, DAY, "s")]).unwrap();
        let (account, key) = key_of(&cache, "s");
        cache
            .store_classification(&account, &key, &result(Choice::Spam, 0.99))
            .unwrap();
        cache
            .store_label(&account, &key, "nm-spam", day(2))
            .unwrap();
        let LabelsDue { due, .. } = cache
            .labels_due(
                None,
                None,
                &rules,
                &LabelPrefix::default(),
                BOTH,
                day(2),
                10,
            )
            .unwrap();
        cache
            .junk_attempt(&account, &key, due[0].id, day(2))
            .unwrap();
        // neomail stopped before the answer, and the move went through: the next sync drops the
        // row. Then the user moves it back: a new row, never moved again.
        cache.remove(&[due[0].id]).unwrap();
        cache.insert_messages(mb.id, &[msg(2, DAY, "s")]).unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), []);
        // A move known never to have been sent is dropped, and due again.
        cache.junk_not_sent(&account, &key).unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), [(key.clone(), false, true)]);
    }

    #[test]
    fn a_schema_6_cache_gains_the_junk_table_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite3");
        {
            let mut cache = Cache::open(&path).unwrap();
            outlook(&mut cache, "out", "a", 1, Some((Choice::Spam, 0.95)));
            outlook(&mut cache, "out", "b", 2, None);
            written(&mut cache, "a", "nm-spam");
        }
        // Schema 6 is this schema without search's index and the junk and label_retry tables.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(UNDO_SEARCH).unwrap();
        conn.execute_batch("DROP TABLE label_retry; DROP TABLE junk; PRAGMA user_version = 6;")
            .unwrap();
        drop(conn);

        let cache = Cache::open(&path).unwrap();
        assert_eq!(schema_version(&cache.conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(cache.list(None, 10).unwrap().len(), 2);
        assert_eq!(
            due_for(&cache, &GroupRules::default(), BOTH),
            [("id-a".to_owned(), false, true)],
            "its label kept, and spam classified before 10d due a move once"
        );
    }
    #[test]
    fn a_failed_message_waits_longer_after_each_failure_and_at_once_for_a_new_group() {
        let mut cache = Cache::open_in_memory().unwrap();
        let rules = GroupRules::default();
        let a = outlook(&mut cache, "out", "a", 1, Some((Choice::Main, 0.9)));
        let at = day(10);
        let due_at = |cache: &Cache, prefix: &LabelPrefix, now: DateTime<Utc>| {
            let found = cache
                .labels_due(None, None, &rules, prefix, BOTH, now, 10)
                .unwrap();
            (found.due.len(), found.more, found.held)
        };
        let prefix = LabelPrefix::default();
        cache.label_tried(a, "nm-main", Tried::Failed, at).unwrap();
        let minute = chrono::TimeDelta::minutes(1);
        assert_eq!(
            due_at(&cache, &prefix, at),
            (0, 0, 1),
            "not due, not counted as more, counted as held back"
        );
        assert_eq!(due_at(&cache, &prefix, at + minute * 14), (0, 0, 1));
        assert_eq!(due_at(&cache, &prefix, at + minute * 15), (1, 0, 0));
        // A second failure in a row waits 30 minutes, a third an hour.
        cache.label_tried(a, "nm-main", Tried::Failed, at).unwrap();
        assert_eq!(due_at(&cache, &prefix, at + minute * 29), (0, 0, 1));
        assert_eq!(due_at(&cache, &prefix, at + minute * 30), (1, 0, 0));
        cache.label_tried(a, "nm-main", Tried::Failed, at).unwrap();
        assert_eq!(due_at(&cache, &prefix, at + minute * 59), (0, 0, 1));
        // A new prefix is a new name to write: due at once.
        let other = LabelPrefix::new("x-").unwrap();
        assert_eq!(due_at(&cache, &other, at), (1, 0, 0));
        // A success clears the waits.
        cache.label_tried(a, "nm-main", Tried::Done, at).unwrap();
        assert_eq!(due_at(&cache, &prefix, at), (1, 0, 0));
    }

    #[test]
    fn the_wait_doubles_from_15_minutes_up_to_a_day() {
        let waits: Vec<i64> = (1..=9).map(|n| label_retry_wait(n).num_minutes()).collect();
        assert_eq!(waits, [15, 30, 60, 120, 240, 480, 960, 1440, 1440]);
        assert_eq!(label_retry_wait(u32::MAX).num_minutes(), 1440);
    }

    #[test]
    fn a_gone_message_is_never_due_while_its_row_stays_and_a_new_row_is_due_at_once() {
        let mut cache = Cache::open_in_memory().unwrap();
        let rules = GroupRules::default();
        let a = outlook(&mut cache, "out", "a", 1, Some((Choice::Spam, 0.95)));
        cache
            .label_tried(a, "nm-spam", Tried::Gone, day(10))
            .unwrap();
        assert_eq!(due_for(&cache, &rules, BOTH), []);
        // Not even for a new group, and it is not held back after a failure either.
        let other = LabelPrefix::new("x-").unwrap();
        assert_eq!(
            cache
                .labels_due(None, None, &rules, &other, BOTH, day(400), 10)
                .unwrap(),
            LabelsDue::default()
        );
        // The sync drops it; brought back, it is a new row, due at once.
        cache.remove(&[a]).unwrap();
        outlook(&mut cache, "out", "a", 1, None);
        assert_eq!(
            due_for(&cache, &rules, BOTH),
            [("id-a".to_owned(), true, true)]
        );
    }

    #[test]
    fn a_schema_8_cache_gains_the_label_retry_table_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite3");
        {
            let mut cache = Cache::open(&path).unwrap();
            outlook(&mut cache, "out", "a", 1, Some((Choice::Main, 0.9)));
            written(&mut cache, "a", "nm-main");
            outlook(&mut cache, "out", "b", 2, Some((Choice::Main, 0.9)));
        }
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE label_retry; PRAGMA user_version = 8;")
            .unwrap();
        drop(conn);

        let mut cache = Cache::open(&path).unwrap();
        assert_eq!(schema_version(&cache.conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(cache.list(None, 10).unwrap().len(), 2);
        assert_eq!(
            due_for(&cache, &GroupRules::default(), BOTH),
            [("id-b".to_owned(), true, false)]
        );
        let prefix = LabelPrefix::default();
        let rules = GroupRules::default();
        let LabelsDue { due, .. } = cache
            .labels_due(None, None, &rules, &prefix, BOTH, day(10), 10)
            .unwrap();
        cache
            .label_tried(due[0].id, "nm-main", Tried::Gone, day(10))
            .unwrap();
        assert_eq!(due_for(&cache, &GroupRules::default(), BOTH), []);
    }
}
