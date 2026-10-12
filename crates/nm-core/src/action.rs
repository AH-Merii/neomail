//! The action registry: the typed actions that change mail, each with a stable name, and the one
//! way to apply an action to cached messages ([`apply`]). The CLI calls it today; the TUI's keys,
//! scripts and agents will call the same function.
//!
//! An action reaches a server through a [`MailWriter`], one per run, which owns the logins and
//! the account locks. The writer gets UIDs (with their UIDVALIDITY) or [`RemoteId`]s through two
//! separate methods, so a UID never reaches Microsoft Graph and a Graph id never reaches IMAP.
//! The cache changes only for what the server confirmed.

use std::fmt;

use crate::cache::{Cache, CacheError, Located, MessageKey};
use crate::types::{Flags, MessageId, RemoteId, Uid, UidValidity};

/// What can be done to a message. Archive, delete and move come later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MarkRead,
    MarkUnread,
}

impl Action {
    pub const ALL: [Self; 2] = [Self::MarkRead, Self::MarkUnread];

    /// The action's stable name, for keys, scripts and agents: it never changes once released.
    pub fn name(self) -> &'static str {
        match self {
            Self::MarkRead => "mark-read",
            Self::MarkUnread => "mark-unread",
        }
    }

    /// Whether a message is read (`\Seen`) once the action is done.
    pub fn seen(self) -> bool {
        self == Self::MarkRead
    }

    /// Whether `flags` are in the state the action asks for.
    pub fn is_done(self, flags: &Flags) -> bool {
        flags.is_seen() == self.seen()
    }

    /// `flags` with the action done to them, for a server that confirms without sending flags.
    fn done_to(self, flags: &Flags) -> Flags {
        let others = flags.iter().filter(|f| *f != Flags::SEEN);
        if self.seen() {
            Flags::new(others.chain([Flags::SEEN]))
        } else {
            Flags::new(others)
        }
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

/// The seam between [`apply`] and the servers. neomail implements it with the logins `neomail
/// sync` uses; the tests use fakes.
///
/// Contract:
/// - [`lock`](Self::lock) is taken before an account's messages are written, and held until its
///   cache rows are updated: a sync of the same account cannot run in between.
/// - `Err` means none of the call's messages is confirmed. The server may still have changed some
///   of them (Graph writes one message per request); the cache is left alone for all of them, and
///   the next sync reads their state.
/// - Nothing a writer does may delete, expunge or move a message.
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
}

/// A message now in the asked state, with its flags as cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub id: MessageId,
    pub account: String,
    pub flags: Flags,
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
/// server, then updates the cache with what the server confirmed. One account failing never
/// stops the others. Repeated ids count once. Only a cache error ends the run early.
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
        let updates = write_account(action, account, &now, writer, &mut report);
        cache.set_flags(&updates)?;
        drop(lock);
    }
    report
        .done
        .sort_by_key(|d| asked.iter().position(|&id| id == d.id));
    Ok(report)
}

/// Writes one account's messages, the UID ones per UIDVALIDITY and the by-id ones together, and
/// returns the cache updates the server confirmed.
fn write_account<W: MailWriter>(
    action: Action,
    account: &str,
    located: &[Located],
    writer: &mut W,
    report: &mut Report<W::Error>,
) -> Vec<(MessageId, Flags)> {
    let mut updates = Vec::new();
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
        let written = writer.set_seen_by_uid(account, validity, &uids, action.seen());
        settle(action, account, &batch, written, report, &mut updates);
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
        let written = writer.set_seen_by_id(account, &ids, action.seen());
        settle(action, account, &batch, written, report, &mut updates);
    }
    updates
}

/// Records one write's outcome for `batch`, pairing the server's keys back with message ids.
fn settle<K: PartialEq, E>(
    action: Action,
    account: &str,
    batch: &[(&Located, K)],
    written: Result<Written<K>, E>,
    report: &mut Report<E>,
    updates: &mut Vec<(MessageId, Flags)>,
) {
    let written = match written {
        Ok(written) => written,
        Err(error) => {
            report.failures.push(Failure::Account {
                account: account.to_owned(),
                ids: batch.iter().map(|(l, _)| l.id).collect(),
                error,
            });
            return;
        }
    };
    for (l, key) in batch {
        let done = written.done.iter().find(|(k, _)| k == key);
        match done {
            Some((_, flags)) => {
                let flags = flags.clone().unwrap_or_else(|| action.done_to(&l.flags));
                if flags != l.flags {
                    updates.push((l.id, flags.clone()));
                }
                if action.is_done(&flags) {
                    report.done.push(Done {
                        id: l.id,
                        account: account.to_owned(),
                        flags,
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
