//! One-shot, read-only sync of an account's INBOX into the cache.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use crate::cache::{Cache, CacheError, MailboxKind, MailboxRecord};
use crate::source::MailSource;
use crate::types::{Flags, ListedMessage, MessageId, Uid, UidFlags};

/// The only mailbox slice 1 syncs.
pub const INBOX: &str = "INBOX";

/// How many of the newest messages the first sync takes, unless told otherwise.
pub const DEFAULT_LIMIT: NonZeroU32 = NonZeroU32::MIN.saturating_add(499);

/// Messages fetched per request, and stored per transaction.
pub(crate) const FETCH_BATCH: usize = 50;

/// What one sync changed in the cache.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Messages fetched and added.
    pub new: usize,
    /// Cached messages whose flags changed on the server.
    pub updated: usize,
    /// Cached messages dropped: expunged on the server, or all of them on a UIDVALIDITY change.
    pub removed: usize,
    /// Messages asked for that came back without a body. They are not cached, and the next sync
    /// asks for them again.
    pub missing: usize,
    /// The server's UIDVALIDITY changed, so the cache was rebuilt.
    pub reset: bool,
    /// The sync is, or finishes, one that started from an empty mailbox it created: the account's
    /// first sync, or a rebuild after [`reset`](Self::reset). It stays set for every sync of that
    /// mailbox until one completes, so the rest of an interrupted first sync is not new mail
    /// either ([`MailboxRecord::synced`](crate::cache::MailboxRecord::synced)). Its new messages
    /// are not new mail.
    pub initial: bool,
    /// The cache ids of the messages this sync added, in the order they were stored. Ids are never
    /// reused, so a message that was already cached is never among them.
    pub new_ids: Vec<MessageId>,
}

/// How far a sync has got with fetching new messages, for a progress display: `done` of `total`
/// messages asked for so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError<E: std::error::Error + 'static> {
    #[error("the mail server request failed")]
    Source(#[source] E),
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// Brings the cached INBOX of `account` up to date with `source`.
///
/// - A changed UIDVALIDITY drops the mailbox's cache and starts again.
/// - The first sync lists every message and fetches the `limit` that arrived last (by
///   INTERNALDATE, not UID: some servers number old mail highest). Its watermark is the highest
///   UID on the server, set only once the first sync completes; an interrupted first sync is
///   simply run again, and keeps what it already cached.
/// - Later syncs fetch every UID above the watermark, with no cap, so no new mail is skipped.
/// - Cached messages get their flags refreshed, and the ones gone from the server are dropped.
/// - New messages are fetched oldest UID first, and each batch of a later sync raises the
///   watermark, so an interrupted sync resumes where it stopped.
/// - A message the source returns without a body is counted in [`SyncReport::missing`], and the
///   watermark stays below it, so the next sync asks for it again.
pub async fn sync_inbox<S: MailSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
) -> Result<SyncReport, SyncError<S::Error>> {
    let mut report = SyncReport::default();
    sync_inbox_with_progress(source, cache, account, limit, &mut report, |_| {}).await?;
    Ok(report)
}

/// [`sync_inbox`], filling `report` as it goes and calling `progress` once the new messages are
/// known (`done` 0) and after each batch is stored. `progress` is not called when there is nothing
/// new to fetch.
///
/// Each batch is stored as it comes, so when the sync fails, `report` still holds what it changed
/// before the failure: [`SyncReport::new_ids`] has the messages already stored, which the next
/// sync finds cached and does not report again.
pub async fn sync_inbox_with_progress<S: MailSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
    report: &mut SyncReport,
    mut progress: impl FnMut(Progress),
) -> Result<(), SyncError<S::Error>> {
    let status = source.examine_inbox().await.map_err(SyncError::Source)?;

    // A by-id mailbox is reset too: its messages have no UIDs.
    let (mailbox_id, highest_uid) = match cache.mailbox(account, INBOX)? {
        Some(MailboxRecord {
            id,
            kind:
                MailboxKind::Imap {
                    uid_validity,
                    highest_uid,
                },
            synced,
        }) if uid_validity == status.uid_validity => {
            report.initial = !synced;
            (id, highest_uid)
        }
        previous => {
            if let Some(old) = previous {
                report.reset = true;
                report.removed = cache.message_count(old.id)?;
            }
            report.initial = true;
            let reset = cache.reset_mailbox(account, INBOX, status.uid_validity)?;
            (reset.id, None)
        }
    };

    let cached = cache.cached_flags(mailbox_id)?;
    let (server, new, watermark): (BTreeMap<Uid, Flags>, Vec<Uid>, Option<Uid>) = match highest_uid
    {
        None => {
            let listing = if status.exists == 0 {
                Vec::new()
            } else {
                source.list_all().await.map_err(SyncError::Source)?
            };
            let (chosen, watermark) = newest_arrivals(&listing, limit);
            let new = chosen
                .into_iter()
                .filter(|uid| cached.binary_search_by_key(uid, |c| c.uid).is_err())
                .collect();
            // The whole listing, so a first sync that runs again keeps the messages it
            // cached before, even once newer mail pushes them out of the chosen set.
            let server = listing.into_iter().map(|m| (m.uid, m.flags)).collect();
            (server, new, watermark)
        }
        Some(highest) => {
            // The listing must cover the oldest cached message, else the first UID after the
            // highest one seen.
            let from = match cached.first() {
                Some(first) => first.uid,
                None => match highest.get().checked_add(1).and_then(Uid::new) {
                    Some(next) => next,
                    None => return Ok(()), // UID space exhausted: nothing can be new.
                },
            };
            let listing = if status.exists == 0 {
                Vec::new()
            } else {
                source.flags_from(from).await.map_err(SyncError::Source)?
            };
            let server: BTreeMap<Uid, Flags> = listing
                .into_iter()
                .filter(|m| m.uid >= from)
                .map(|m| (m.uid, m.flags))
                .collect();
            let new = server
                .keys()
                .copied()
                .filter(|uid| *uid > highest)
                // Already cached: stored above a message that came back without a body.
                .filter(|uid| cached.binary_search_by_key(uid, |c| c.uid).is_err())
                .collect();
            (server, new, None)
        }
    };

    let mut updates = Vec::new();
    let mut removed = Vec::new();
    for c in &cached {
        match server.get(&c.uid) {
            Some(flags) if *flags != c.flags => updates.push(UidFlags {
                uid: c.uid,
                flags: flags.clone(),
            }),
            Some(_) => {}
            None => removed.push(c.uid),
        }
    }
    cache.apply_changes(mailbox_id, &updates, &removed)?;
    report.updated = updates.len();
    report.removed += removed.len();

    let first_sync = highest_uid.is_none();
    // The lowest UID that was asked for but came back without a body. The watermark never passes
    // it, so the next sync asks for it again.
    let mut lowest_missing: Option<Uid> = None;
    let mut done = Progress {
        done: 0,
        total: new.len(),
    };
    if !new.is_empty() {
        progress(done);
    }
    for batch in new.chunks(FETCH_BATCH) {
        let mut fetched = source.fetch(batch).await.map_err(SyncError::Source)?;
        fetched.retain(|m| batch.binary_search(&m.uid).is_ok());
        fetched.sort_by_key(|m| m.uid);
        fetched.dedup_by_key(|m| m.uid);
        let missing = batch
            .iter()
            .filter(|uid| fetched.binary_search_by_key(*uid, |m| m.uid).is_err());
        report.missing += missing.clone().count();
        lowest_missing = lowest_missing.or_else(|| missing.min().copied());
        // A later sync raises the watermark to the highest UID of this batch below the lowest
        // missing one; a first sync sets it only at the end.
        let raise_to = if first_sync {
            None
        } else {
            batch
                .iter()
                .copied()
                .filter(|uid| lowest_missing.is_none_or(|lowest| *uid < lowest))
                .max()
        };
        let stored = cache.store_messages(mailbox_id, &fetched, raise_to)?;
        report.new += fetched.len();
        report.new_ids.extend(stored);
        done.done += batch.len();
        progress(done);
    }
    if let Some(watermark) = watermark.filter(|_| lowest_missing.is_none()) {
        cache.raise_highest_uid(mailbox_id, watermark)?;
    }
    // A first sync still owed a body is not done: the next sync stays initial, so the old mail it
    // fetches is not announced as new.
    if !first_sync || lowest_missing.is_none() {
        cache.mark_synced(mailbox_id)?;
    }
    Ok(())
}

/// Picks the UIDs of the `limit` messages that arrived last (ties and unknown dates by UID,
/// highest first), in UID order, and the highest UID in the whole listing.
fn newest_arrivals(listing: &[ListedMessage], limit: NonZeroU32) -> (Vec<Uid>, Option<Uid>) {
    let watermark = listing.iter().map(|m| m.uid).max();
    let mut arrivals: Vec<&ListedMessage> = listing.iter().collect();
    arrivals.sort_by_key(|m| m.uid);
    arrivals.dedup_by_key(|m| m.uid);
    // `None` sorts before any date, so messages without one count as the oldest.
    arrivals.sort_by_key(|m| std::cmp::Reverse((m.received, m.uid)));
    let cap = usize::try_from(limit.get()).unwrap_or(usize::MAX);
    let mut chosen: Vec<Uid> = arrivals.into_iter().take(cap).map(|m| m.uid).collect();
    chosen.sort_unstable();
    (chosen, watermark)
}
