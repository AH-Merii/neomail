//! The action registry: the typed actions that change mail, each with a stable name, and the one
//! way to apply an action to cached messages ([`apply`]). The CLI calls it today; the TUI's keys,
//! scripts and agents will call the same function.
//!
//! An action reaches a server through a [`MailWriter`], one per run, which owns the logins and
//! the account locks. The writer gets UIDs (with their UIDVALIDITY) or [`RemoteId`]s through
//! separate methods, so a UID never reaches Microsoft Graph and a Graph id never reaches IMAP.
//! A move names its destination by [`Folder`] role or by the [`FolderName`] the user typed
//! ([`Destination`]); only the account's server resolves it to one of its folders. The cache
//! changes only for what the server confirmed.

use std::fmt;

use crate::cache::{Cache, CacheError, Located, MessageKey};
use crate::folder::FolderName;
use crate::types::{Flags, MessageId, RemoteId, Uid, UidValidity};

/// What can be done to a message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    MarkRead,
    MarkUnread,
    /// Moves a message from the INBOX to its account's archive.
    Archive,
    /// Moves a message from the INBOX to its account's trash. Never deletes it for good.
    Delete,
    /// Moves a message from the INBOX to its account's folder of this name. Never creates one.
    Move(FolderName),
}

/// A folder named by its role. The account's server resolves the role to one of its folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Folder {
    Archive,
    Trash,
    /// Where confident spam goes (DEC-2026-10-14-labels-junk). No user command moves here: only
    /// the label pass does.
    Junk,
}

impl fmt::Display for Folder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Archive => "archive",
            Self::Trash => "trash",
            Self::Junk => "junk",
        })
    }
}

/// Where a move puts a message: the account's folder with a role, or the one with the name the
/// user gave. It stays this until the account's server resolves it to one of its folders.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Destination {
    Role(Folder),
    Name(FolderName),
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Role(role) => role.fmt(f),
            Self::Name(name) => name.fmt(f),
        }
    }
}

impl From<Folder> for Destination {
    fn from(role: Folder) -> Self {
        Self::Role(role)
    }
}

/// What an action does to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Sets (`true`) or clears `\Seen`.
    Seen(bool),
    /// Moves it out of the INBOX to this destination.
    Move(Destination),
}

impl Action {
    /// Every action's [`name`](Self::name).
    pub const NAMES: [&'static str; 5] = ["mark-read", "mark-unread", "archive", "delete", "move"];

    /// The action's stable name, for keys, scripts and agents: it never changes once released.
    pub fn name(&self) -> &'static str {
        match self {
            Self::MarkRead => "mark-read",
            Self::MarkUnread => "mark-unread",
            Self::Archive => "archive",
            Self::Delete => "delete",
            Self::Move(_) => "move",
        }
    }

    pub fn change(&self) -> Change {
        match self {
            Self::MarkRead => Change::Seen(true),
            Self::MarkUnread => Change::Seen(false),
            Self::Archive => Change::Move(Destination::Role(Folder::Archive)),
            Self::Delete => Change::Move(Destination::Role(Folder::Trash)),
            Self::Move(name) => Change::Move(Destination::Name(name.clone())),
        }
    }
}

/// `flags` with `\Seen` set or cleared, for a server that confirms without sending flags.
fn seen_to(seen: bool, flags: &Flags) -> Flags {
    let others = flags.iter().filter(|f| *f != Flags::SEEN);
    if seen {
        Flags::new(others.chain([Flags::SEEN]))
    } else {
        Flags::new(others)
    }
}

/// What a server confirmed: the messages now in the asked state, with their flags when the
/// server sent them, and the messages it no longer has. A key in neither was not confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written<K> {
    pub done: Vec<(K, Option<Flags>)>,
    pub gone: Vec<K>,
}

impl<K> Default for Written<K> {
    fn default() -> Self {
        Self {
            done: Vec::new(),
            gone: Vec::new(),
        }
    }
}

/// What a server confirmed of a move: the folder it moved to, the messages it moved there, and
/// the ones its INBOX no longer has. A key in neither was not confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved<K> {
    /// The folder as the user sees it: an IMAP mailbox name decoded from modified UTF-7, such as
    /// `Archive` or `[Gmail]/Trash`, or a Graph folder name.
    pub folder: String,
    pub moved: Vec<K>,
    pub gone: Vec<K>,
}

/// The seam between [`apply`] and the servers. neomail implements it with the logins `neomail
/// sync` uses; the tests use fakes.
///
/// Contract:
/// - [`lock`](Self::lock) is taken before an account's messages are written, and held until its
///   cache rows are updated: a sync of the same account cannot run in between.
/// - `Err` means none of the call's messages is confirmed. The server may still have changed some
///   of them (Graph writes one message per request, and an IMAP MOVE can fail partway, RFC 6851
///   section 3.3); the cache is left alone for all of them, and the next sync reads their state.
/// - Nothing a writer does may delete or expunge a message for good, or create, rename or delete a
///   folder. A [`Destination::Name`] the account has no folder for is an `Err`, and so is one
///   naming the INBOX or a folder that cannot hold messages: nothing moves. Only the move methods
///   move a message, and only to the folder asked: by UID, only from the INBOX; by id, from
///   wherever the server holds it now (Graph moves a message by its id), which is the INBOX unless
///   it moved since the last sync.
pub trait MailWriter {
    /// Held while an account is written and its cache rows are updated.
    type Lock;
    type Error: fmt::Display;

    fn lock(&mut self, account: &str) -> Result<Self::Lock, Self::Error>;

    /// Sets or clears `\Seen` on `uids` in the account's INBOX, refusing when the server's
    /// UIDVALIDITY is not `uid_validity`.
    fn set_seen_by_uid(
        &mut self,
        account: &str,
        uid_validity: UidValidity,
        uids: &[Uid],
        seen: bool,
    ) -> Result<Written<Uid>, Self::Error>;

    /// Sets or clears the read state of messages named by their server ids.
    fn set_seen_by_id(
        &mut self,
        account: &str,
        ids: &[RemoteId],
        seen: bool,
    ) -> Result<Written<RemoteId>, Self::Error>;

    /// Moves `uids` from the account's INBOX to its folder `to`, refusing when the server's
    /// UIDVALIDITY is not `uid_validity` or it has no such folder.
    fn move_by_uid(
        &mut self,
        account: &str,
        uid_validity: UidValidity,
        uids: &[Uid],
        to: &Destination,
    ) -> Result<Moved<Uid>, Self::Error>;

    /// Moves messages named by their server ids to the account's folder `to`, refusing when it
    /// has no such folder.
    fn move_by_id(
        &mut self,
        account: &str,
        ids: &[RemoteId],
        to: &Destination,
    ) -> Result<Moved<RemoteId>, Self::Error>;
}

/// A message now in the asked state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub id: MessageId,
    pub account: String,
    pub state: State,
}

/// Where a done message stands: its flags as cached after a mark, or the folder a move put it in
/// (its cache row is gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Flags(Flags),
    /// The destination as the server names it.
    MovedTo(String),
}

/// Why a message is not confirmed in the asked state. The cache is unchanged for it, except for
/// flags the server sent ([`Failure::NotConfirmed`]).
#[derive(Debug)]
pub enum Failure<E> {
    /// No cached message has this id.
    Unknown(MessageId),
    /// The server no longer has it; the next sync removes it from the cache.
    Gone { account: String, id: MessageId },
    /// The server answered but did not confirm the change. Flags it sent for the message, in the
    /// other state, are cached all the same.
    NotConfirmed { account: String, id: MessageId },
    /// The account could not be locked or written; none of `ids` is confirmed. The server may
    /// still have changed some of them; the next sync reads their state.
    Account {
        account: String,
        ids: Vec<MessageId>,
        error: E,
    },
}

/// What [`apply`] did: the messages now in the asked state, in the order asked, and the
/// failures.
#[derive(Debug)]
pub struct Report<E> {
    pub done: Vec<Done>,
    pub failures: Vec<Failure<E>>,
}

/// Applies `action` to the cached messages `ids`: per account, takes its lock, writes to its
/// server, then updates the cache with what the server confirmed: new flags, or a moved message's
/// row removed, as a sync does for a message gone from the INBOX. One account failing never stops
/// the others. Repeated ids count once. Only a cache error ends the run early.
pub fn apply<W: MailWriter>(
    action: Action,
    ids: &[MessageId],
    cache: &mut Cache,
    writer: &mut W,
) -> Result<Report<W::Error>, CacheError> {
    let mut asked: Vec<MessageId> = Vec::new();
    for &id in ids {
        if !asked.contains(&id) {
            asked.push(id);
        }
    }
    let mut report = Report {
        done: Vec::new(),
        failures: Vec::new(),
    };
    let located = cache.locate(&asked)?;
    for &id in &asked {
        if !located.iter().any(|l| l.id == id) {
            report.failures.push(Failure::Unknown(id));
        }
    }
    let mut accounts: Vec<(&str, Vec<MessageId>)> = Vec::new();
    for l in &located {
        match accounts.iter_mut().find(|(a, _)| *a == l.account) {
            Some((_, ids)) => ids.push(l.id),
            None => accounts.push((&l.account, vec![l.id])),
        }
    }
    for (account, ids) in accounts {
        let lock = match writer.lock(account) {
            Ok(lock) => lock,
            Err(error) => {
                report.failures.push(Failure::Account {
                    account: account.to_owned(),
                    ids,
                    error,
                });
                continue;
            }
        };
        // Read again under the lock: a sync that ran while we waited may have changed them.
        let now = cache.locate(&ids)?;
        for &id in &ids {
            if !now.iter().any(|l| l.id == id) {
                report.failures.push(Failure::Unknown(id));
            }
        }
        let updates = write_account(&action, account, &now, writer, &mut report);
        cache.set_flags(&updates.flags)?;
        cache.remove(&updates.moved)?;
        drop(lock);
    }
    report
        .done
        .sort_by_key(|d| asked.iter().position(|&id| id == d.id));
    Ok(report)
}

/// The cache updates the server confirmed for one account.
#[derive(Default)]
struct Updates {
    flags: Vec<(MessageId, Flags)>,
    moved: Vec<MessageId>,
}

/// Writes one account's messages, the UID ones per UIDVALIDITY and the by-id ones together, and
/// returns the cache updates the server confirmed.
fn write_account<W: MailWriter>(
    action: &Action,
    account: &str,
    located: &[Located],
    writer: &mut W,
    report: &mut Report<W::Error>,
) -> Updates {
    let mut updates = Updates::default();
    let mut validities: Vec<UidValidity> = Vec::new();
    for l in located {
        if let MessageKey::Uid { uid_validity, .. } = l.key
            && !validities.contains(&uid_validity)
        {
            validities.push(uid_validity);
        }
    }
    for validity in validities {
        let batch: Vec<(&Located, Uid)> = located
            .iter()
            .filter_map(|l| match l.key {
                MessageKey::Uid { uid_validity, uid } if uid_validity == validity => Some((l, uid)),
                _ => None,
            })
            .collect();
        let uids: Vec<Uid> = batch.iter().map(|(_, uid)| *uid).collect();
        match action.change() {
            Change::Seen(seen) => {
                let written = writer.set_seen_by_uid(account, validity, &uids, seen);
                settle(seen, account, &batch, written, report, &mut updates);
            }
            Change::Move(to) => {
                let moved = writer.move_by_uid(account, validity, &uids, &to);
                settle_move(account, &batch, moved, report, &mut updates);
            }
        }
    }
    let batch: Vec<(&Located, RemoteId)> = located
        .iter()
        .filter_map(|l| match &l.key {
            MessageKey::Remote(id) => Some((l, id.clone())),
            MessageKey::Uid { .. } => None,
        })
        .collect();
    if !batch.is_empty() {
        let ids: Vec<RemoteId> = batch.iter().map(|(_, id)| id.clone()).collect();
        match action.change() {
            Change::Seen(seen) => {
                let written = writer.set_seen_by_id(account, &ids, seen);
                settle(seen, account, &batch, written, report, &mut updates);
            }
            Change::Move(to) => {
                let moved = writer.move_by_id(account, &ids, &to);
                settle_move(account, &batch, moved, report, &mut updates);
            }
        }
    }
    updates
}

/// The failure for every message of `batch` when its account could not be written.
fn account_failure<K, E>(account: &str, batch: &[(&Located, K)], error: E) -> Failure<E> {
    Failure::Account {
        account: account.to_owned(),
        ids: batch.iter().map(|(l, _)| l.id).collect(),
        error,
    }
}

/// Records one read-state write's outcome for `batch`, pairing the server's keys back with
/// message ids.
fn settle<K: PartialEq, E>(
    seen: bool,
    account: &str,
    batch: &[(&Located, K)],
    written: Result<Written<K>, E>,
    report: &mut Report<E>,
    updates: &mut Updates,
) {
    let written = match written {
        Ok(written) => written,
        Err(error) => {
            report.failures.push(account_failure(account, batch, error));
            return;
        }
    };
    for (l, key) in batch {
        let done = written.done.iter().find(|(k, _)| k == key);
        match done {
            Some((_, flags)) => {
                let flags = flags.clone().unwrap_or_else(|| seen_to(seen, &l.flags));
                if flags != l.flags {
                    updates.flags.push((l.id, flags.clone()));
                }
                if flags.is_seen() == seen {
                    report.done.push(Done {
                        id: l.id,
                        account: account.to_owned(),
                        state: State::Flags(flags),
                    });
                } else {
                    report.failures.push(Failure::NotConfirmed {
                        account: account.to_owned(),
                        id: l.id,
                    });
                }
            }
            None if written.gone.contains(key) => report.failures.push(Failure::Gone {
                account: account.to_owned(),
                id: l.id,
            }),
            None => report.failures.push(Failure::NotConfirmed {
                account: account.to_owned(),
                id: l.id,
            }),
        }
    }
}

/// Records one move's outcome for `batch`: a moved message's row leaves the cache; a gone one
/// stays for the next sync, as for a mark.
fn settle_move<K: PartialEq, E>(
    account: &str,
    batch: &[(&Located, K)],
    moved: Result<Moved<K>, E>,
    report: &mut Report<E>,
    updates: &mut Updates,
) {
    let moved = match moved {
        Ok(moved) => moved,
        Err(error) => {
            report.failures.push(account_failure(account, batch, error));
            return;
        }
    };
    for (l, key) in batch {
        let failure = if moved.moved.contains(key) {
            updates.moved.push(l.id);
            report.done.push(Done {
                id: l.id,
                account: account.to_owned(),
                state: State::MovedTo(moved.folder.clone()),
            });
            continue;
        } else if moved.gone.contains(key) {
            Failure::Gone {
                account: account.to_owned(),
                id: l.id,
            }
        } else {
            Failure::NotConfirmed {
                account: account.to_owned(),
                id: l.id,
            }
        };
        report.failures.push(failure);
    }
}
