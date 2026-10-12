//! The seam for providers that name messages with opaque ids instead of UIDs (Microsoft Graph),
//! and the sync engine that drives it. [`MailSource`](crate::MailSource) and
//! [`sync_inbox`](crate::sync_inbox) stay as they are for IMAP.

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, MailboxKind, MailboxRecord};
use crate::sync::{FETCH_BATCH, INBOX, Progress, SyncError, SyncReport};
use crate::types::{Flags, IdFetched, IdListed, RemoteId};

/// A read-only view of one account's INBOX at a provider whose messages have opaque string ids
/// and no UIDs or UIDVALIDITY (Microsoft Graph:
/// <https://learn.microsoft.com/en-us/graph/outlook-immutable-id>).
///
/// Contract, checked by `nm_core::conformance::id_source` (the `conformance` feature):
/// - Nothing a source does may change the server: no flags (fetches must not mark mail read), no
///   moves, no deletes.
/// - Every listed message carries its arrival time; the engine windows on it.
/// - Results may come in any order and may repeat; the sync engine filters and sorts.
///
/// neomail drives a source from one task, so the futures carry no `Send` bound yet.
pub trait IdSource {
    type Error: std::error::Error + Send + Sync + 'static;

    /// The `limit` messages that arrived last, without content, in any order.
    fn newest(
        &mut self,
        limit: NonZeroU32,
    ) -> impl Future<Output = Result<Vec<IdListed>, Self::Error>>;

    /// Every message that arrived at or after `since`, in any order; may repeat.
    fn arrived_since(
        &mut self,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<IdListed>, Self::Error>>;

    /// The RFC 822 source of `ids` without marking them read; ids that are gone are left out.
    fn fetch(
        &mut self,
        ids: &[RemoteId],
    ) -> impl Future<Output = Result<Vec<IdFetched>, Self::Error>>;
}

/// Brings the cached INBOX of `account` up to date with an [`IdSource`].
///
/// 1. A missing mailbox, or an IMAP one, is reset to a by-id mailbox; [`SyncReport::reset`] and
///    [`SyncReport::removed`] report it when the mailbox existed.
/// 2. With nothing cached and no `seen_until`, the source lists its `limit` newest messages.
///    Otherwise it lists every message that arrived since the oldest cached arrival or
///    `seen_until`, whichever is earlier. That window covers every cached message, so a cached id
///    that is not listed was removed and is dropped, and a listed one whose flags differ is
///    updated.
/// 3. Listed ids that are not cached are fetched oldest arrival first, 50 (`FETCH_BATCH`) per
///    request and per transaction, with no cap after the first sync. Flags and arrival come from
///    the listing; the source is parsed as the IMAP path parses it.
/// 4. With nothing missing, `seen_until` becomes the newest listed arrival; otherwise the oldest
///    arrival that came back without a body, so the next sync asks for it again. That arrival is
///    saved with the batch it was missing from, so it holds even when a later batch fails.
///    Because the window starts at the oldest cached message, an interrupted first sync is
///    finished by the next one.
///
/// Known limit: mail moved into the inbox with an arrival time older than the window is not
/// seen. Graph's delta query (docs/roadmap.md, row 7) lifts it.
pub async fn sync_inbox_by_id<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
) -> Result<SyncReport, SyncError<S::Error>> {
    sync_inbox_by_id_with_progress(source, cache, account, limit, |_| {}).await
}

/// [`sync_inbox_by_id`], calling `progress` once the new messages are known (`done` 0) and after
/// each batch is stored. It is not called when there is nothing new to fetch.
pub async fn sync_inbox_by_id_with_progress<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
    mut progress: impl FnMut(Progress),
) -> Result<SyncReport, SyncError<S::Error>> {
    let mut report = SyncReport::default();
    let (mailbox_id, seen_until) = match cache.mailbox(account, INBOX)? {
        Some(MailboxRecord {
            id,
            kind: MailboxKind::ById { seen_until },
        }) => (id, seen_until),
        previous => {
            if let Some(old) = previous {
                report.reset = true;
                report.removed = cache.message_count(old.id)?;
            }
            (cache.reset_id_mailbox(account, INBOX)?.id, None)
        }
    };

    let cached = cache.cached_ids(mailbox_id)?;
    let oldest_cached = cached.iter().map(|c| c.received).min();
    let since = match (oldest_cached, seen_until) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let listing = match since {
        None => newest(
            source.newest(limit).await.map_err(SyncError::Source)?,
            limit,
        ),
        Some(since) => source
            .arrived_since(since)
            .await
            .map_err(SyncError::Source)?
            .into_iter()
            .filter(|m| m.received >= since)
            .collect(),
    };
    let server: BTreeMap<RemoteId, IdListed> =
        listing.into_iter().map(|m| (m.id.clone(), m)).collect();

    let cached: BTreeMap<&RemoteId, &Flags> = cached.iter().map(|c| (&c.id, &c.flags)).collect();
    let mut updates = Vec::new();
    let mut removed = Vec::new();
    for (id, flags) in &cached {
        match server.get(*id) {
            Some(m) if m.flags != **flags => updates.push(((*id).clone(), m.flags.clone())),
            Some(_) => {}
            None => removed.push((*id).clone()),
        }
    }
    cache.apply_id_changes(mailbox_id, &updates, &removed)?;
    report.updated = updates.len();
    report.removed += removed.len();

    let mut new: Vec<&IdListed> = server
        .values()
        .filter(|m| !cached.contains_key(&m.id))
        .collect();
    new.sort_by(|a, b| (a.received, &a.id).cmp(&(b.received, &b.id)));
    // The oldest arrival asked for that came back without a body; `seen_until` stops there.
    let mut oldest_missing: Option<DateTime<Utc>> = None;
    let mut done = Progress {
        done: 0,
        total: new.len(),
    };
    if !new.is_empty() {
        progress(done);
    }
    for batch in new.chunks(FETCH_BATCH) {
        let ids: Vec<RemoteId> = batch.iter().map(|m| m.id.clone()).collect();
        let fetched = source.fetch(&ids).await.map_err(SyncError::Source)?;
        // Anything not asked for is ignored.
        let bodies: BTreeMap<&RemoteId, &[u8]> = fetched
            .iter()
            .map(|f: &IdFetched| (&f.id, f.raw.as_slice()))
            .collect();
        let mut store = Vec::with_capacity(batch.len());
        for m in batch {
            match bodies.get(&m.id) {
                Some(raw) => store.push((*m, *raw)),
                None => {
                    report.missing += 1;
                    // Batches go oldest first, so the first one missing is the oldest.
                    oldest_missing = oldest_missing.or(Some(m.received));
                }
            }
        }
        // Saved with the batch: a later batch that fails must not lose the hole.
        cache.store_id_messages(mailbox_id, &store, oldest_missing)?;
        report.new += store.len();
        done.done += batch.len();
        progress(done);
    }

    let next = oldest_missing.or_else(|| server.values().map(|m| m.received).max());
    if let Some(next) = next {
        cache.set_seen_until(mailbox_id, next)?;
    }
    Ok(report)
}

/// The `limit` messages of `listing` that arrived last, once each: a source may return more.
fn newest(mut listing: Vec<IdListed>, limit: NonZeroU32) -> Vec<IdListed> {
    listing.sort_by(|a, b| a.id.cmp(&b.id));
    listing.dedup_by(|a, b| a.id == b.id);
    listing.sort_by(|a, b| (b.received, &b.id).cmp(&(a.received, &a.id)));
    listing.truncate(usize::try_from(limit.get()).unwrap_or(usize::MAX));
    listing
}
