//! The seam for providers that name messages with opaque ids instead of UIDs (Microsoft Graph),
//! and the sync engine that drives it. [`MailSource`](crate::MailSource) and
//! [`sync_inbox`](crate::sync_inbox) stay as they are for IMAP.

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU32;

use chrono::{DateTime, TimeDelta, Utc};

use crate::cache::{Cache, MailboxKind, MailboxRecord};
use crate::sync::{FETCH_BATCH, INBOX, Progress, SyncError, SyncReport};
use crate::types::{Cursor, Flags, IdChange, IdChanges, IdFetched, IdListed, RemoteId};

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

    /// The most messages a starting round can hold, when the provider has such a limit: a
    /// listing that alone holds this many does not ask for a round, which [`Self::start_changes`]
    /// would refuse. `None`, the default, for no limit.
    const START_LIMIT: Option<usize> = None;

    /// Starts tracking changes: a whole round that holds, as [`IdChange::Present`], every message
    /// there now that arrived at or after `from`, and the cursor the next round starts from. The
    /// round is complete: the engine drops a cached message from `from` on that it does not name,
    /// since it was deleted or moved out after the listing, and no later round would report that
    /// removal. Later rounds report changes after the round's state was taken: every message that
    /// arrives, every read change to a message that arrived at or after `from`, and every removal
    /// of a message that is in the INBOX. They may also report changes outside that, which the
    /// engine ignores when it does not hold the message.
    ///
    /// `Ok(None)`, the default, when the source cannot track changes, or cannot for this window
    /// (a round that would hold more than the provider returns at once; a source should stop
    /// asking as soon as it knows): the engine then lists, as it did before cursors, and asks
    /// again on the next listing sync.
    fn start_changes(
        &mut self,
        from: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<IdChanges>, Self::Error>> {
        let _ = from;
        async { Ok(None) }
    }

    /// Every change since `cursor`, as a whole round: a source never returns part of one. Changes
    /// may repeat and may name messages the engine does not hold.
    ///
    /// `Ok(None)` when the provider refuses the cursor (it expired, or the provider reset change
    /// tracking): the engine lists and starts a new round.
    fn changes_since(
        &mut self,
        cursor: &Cursor,
    ) -> impl Future<Output = Result<Option<IdChanges>, Self::Error>> {
        let _ = cursor;
        async { Ok(None) }
    }
}

/// Brings the cached INBOX of `account` up to date with an [`IdSource`].
///
/// 1. A missing mailbox, or an IMAP one, is reset to a by-id mailbox; [`SyncReport::reset`] and
///    [`SyncReport::removed`] report it when the mailbox existed.
/// 2. With a stored [`Cursor`], the source reports what changed since it
///    ([`IdSource::changes_since`]), and the sync applies that round (see "Rounds" below) instead
///    of listing. When the source refuses the cursor, it is dropped and the sync lists.
/// 3. Listing: with nothing cached and no `seen_until`, the source lists its `limit` newest
///    messages. Otherwise it lists every message that arrived since the oldest cached arrival or
///    `seen_until`, whichever is earlier. That window covers every cached message, so a cached id
///    that is not listed was removed and is dropped, and a listed one whose flags differ is
///    updated.
/// 4. Listed ids that are not cached are fetched oldest arrival first, 50 (`FETCH_BATCH`) per
///    request and per transaction, with no cap after the first sync. Flags and arrival come from
///    the listing; the source is parsed as the IMAP path parses it.
/// 5. With nothing missing, `seen_until` becomes the newest listed arrival; otherwise the oldest
///    arrival that came back without a body, so the next sync asks for it again. That arrival is
///    saved with the batch it was missing from, so it holds even when a later batch fails.
///    Because the window starts at the oldest cached message, an interrupted first sync is
///    finished by the next one.
/// 6. With nothing missing, a listing sync starts a round from the start of the window: the
///    oldest arrival now cached, or `seen_until`, whichever is earlier
///    ([`IdSource::start_changes`]). It applies the round and stores its cursor. Every cached
///    message is inside the round, so the source reports later changes to all of them; a source
///    need not report changes to messages that arrived before the round's start. A listing that
///    alone holds [`IdSource::START_LIMIT`] messages or more starts no round.
///
/// Rounds: a later round ([`IdSource::changes_since`]) only adds, updates and removes what it
/// names; it never drops a message it leaves out. A starting round names every message from its
/// start on, so it also drops every cached message from its start on that it leaves out: one
/// deleted or moved out between the listing and the round, which no later round reports. A source
/// returns no starting round it had to cut short, so this cannot empty the cache. The last change
/// per id wins. A removal drops a cached message. A present message that is cached gets its flags;
/// one that is not is fetched when it arrived inside the window (the oldest cached arrival or
/// `seen_until`) and ignored otherwise, as is one without an arrival time. The round's cursor is
/// stored only after every change is applied and nothing came back without a body; when something
/// did, the cursor is cleared so the next sync lists from that message and asks again. A sync that
/// fails keeps the previous cursor, and the next round repeats the changes.
///
/// An empty window (nothing cached or listed, and no `seen_until`) starts its round a day
/// ([`EMPTY_WINDOW_MARGIN`]) before the local clock, and stores no `seen_until`: the clock may run
/// ahead of the server's, and mail stamped by the server before the local time must still come.
///
/// Known limit: mail moved into the inbox with an arrival time older than the window is never
/// seen: rounds leave out what arrived before their start, and listings filter on the window.
pub async fn sync_inbox_by_id<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
) -> Result<SyncReport, SyncError<S::Error>> {
    let mut report = SyncReport::default();
    sync_inbox_by_id_with_progress(source, cache, account, limit, &mut report, |_| {}).await?;
    Ok(report)
}

/// [`sync_inbox_by_id`], filling `report` as it goes and calling `progress` once the new messages
/// are known (`done` 0) and after each batch is stored. `progress` is not called when there is
/// nothing new to fetch.
///
/// Each batch is stored as it comes, so when the sync fails, `report` still holds what it changed
/// before the failure: [`SyncReport::new_ids`] has the messages already stored, which the next
/// sync finds cached and does not report again.
pub async fn sync_inbox_by_id_with_progress<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    account: &str,
    limit: NonZeroU32,
    report: &mut SyncReport,
    mut progress: impl FnMut(Progress),
) -> Result<(), SyncError<S::Error>> {
    let (mailbox_id, seen_until) = match cache.mailbox(account, INBOX)? {
        Some(MailboxRecord {
            id,
            kind: MailboxKind::ById { seen_until },
            synced,
        }) => {
            report.initial = !synced;
            (id, seen_until)
        }
        previous => {
            if let Some(old) = previous {
                report.reset = true;
                report.removed = cache.message_count(old.id)?;
            }
            report.initial = true;
            (cache.reset_id_mailbox(account, INBOX)?.id, None)
        }
    };
    let was_synced = !report.initial;

    if was_synced && let Some(cursor) = cache.cursor(mailbox_id)? {
        match source
            .changes_since(&cursor)
            .await
            .map_err(SyncError::Source)?
        {
            Some(round) => {
                return apply_round(
                    source,
                    cache,
                    mailbox_id,
                    seen_until,
                    round,
                    None,
                    report,
                    &mut progress,
                )
                .await;
            }
            None => cache.set_cursor(mailbox_id, None)?,
        }
    }

    let cached = cache.cached_ids(mailbox_id)?;
    let oldest_cached = cached.iter().map(|c| c.received).min();
    let since = earlier(oldest_cached, seen_until);
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

    let new: Vec<&IdListed> = server
        .values()
        .filter(|m| !cached.contains_key(&m.id))
        .collect();
    let oldest_missing = fetch_new(source, cache, mailbox_id, new, report, &mut progress).await?;

    let newest_listed = server.values().map(|m| m.received).max();
    let next = oldest_missing.or(newest_listed);
    if let Some(next) = next {
        cache.set_seen_until(mailbox_id, next)?;
    }
    // A first sync still owed a body is not done: the next sync stays initial, so the old mail it
    // fetches is not announced as new.
    if was_synced || oldest_missing.is_none() {
        cache.mark_synced(mailbox_id)?;
    }
    if oldest_missing.is_some() || S::START_LIMIT.is_some_and(|l| server.len() >= l) {
        return Ok(());
    }
    // The round covers the whole window, so later changes to every cached message come with it:
    // a source may leave out changes to messages that arrived before `from`. An empty window has
    // only the local clock, which may run ahead of the server's: it starts a margin earlier, and
    // stores no `seen_until`, so the round takes every arrival it names.
    let oldest_listed = server.values().map(|m| m.received).min();
    let from = earlier(since, oldest_listed).unwrap_or_else(|| Utc::now() - EMPTY_WINDOW_MARGIN);
    let Some(round) = source
        .start_changes(from)
        .await
        .map_err(SyncError::Source)?
    else {
        return Ok(());
    };
    apply_round(
        source,
        cache,
        mailbox_id,
        next.or(seen_until),
        round,
        Some(from),
        report,
        &mut progress,
    )
    .await
}

/// How far before the local clock the round of an empty window starts: far more than a clock
/// that keeps time drifts from the server's.
pub const EMPTY_WINDOW_MARGIN: TimeDelta = TimeDelta::days(1);

/// The earlier of two optional times.
fn earlier(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Applies a round of changes as [`sync_inbox_by_id`] describes, then stores or clears its cursor.
/// `starting_from`: the round came from [`IdSource::start_changes`] with this start, so it names
/// every message from there on.
#[allow(clippy::too_many_arguments)]
async fn apply_round<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    mailbox_id: i64,
    seen_until: Option<DateTime<Utc>>,
    round: IdChanges,
    starting_from: Option<DateTime<Utc>>,
    report: &mut SyncReport,
    progress: &mut impl FnMut(Progress),
) -> Result<(), SyncError<S::Error>> {
    let cached_list = cache.cached_ids(mailbox_id)?;
    let window = earlier(cached_list.iter().map(|c| c.received).min(), seen_until);
    let cached: BTreeMap<&RemoteId, &Flags> =
        cached_list.iter().map(|c| (&c.id, &c.flags)).collect();

    // The last change per id wins; a later update without a field keeps the earlier one's.
    let mut last: BTreeMap<RemoteId, IdChange> = BTreeMap::new();
    for change in round.changes {
        let merged = match (last.remove(change.id()), change) {
            (
                Some(IdChange::Present {
                    flags: f0,
                    received: r0,
                    ..
                }),
                IdChange::Present {
                    id,
                    flags,
                    received,
                },
            ) => IdChange::Present {
                id,
                flags: flags.or(f0),
                received: received.or(r0),
            },
            (_, change) => change,
        };
        last.insert(merged.id().clone(), merged);
    }

    let mut updates = Vec::new();
    let mut removed = Vec::new();
    let mut new = Vec::new();
    // A starting round is complete: a cached message from its start on that it leaves out was
    // deleted or moved out after the listing, and no later round would say so.
    if let Some(from) = starting_from {
        removed.extend(
            cached_list
                .iter()
                .filter(|c| c.received >= from && !last.contains_key(&c.id))
                .map(|c| c.id.clone()),
        );
    }
    for change in last.into_values() {
        match change {
            IdChange::Removed(id) => {
                if cached.contains_key(&id) {
                    removed.push(id);
                }
            }
            IdChange::Present {
                id,
                flags,
                received,
            } => match cached.get(&id) {
                Some(old) => {
                    if let Some(flags) = flags
                        && flags != **old
                    {
                        updates.push((id, flags));
                    }
                }
                // Not cached: new when it arrived inside the window; otherwise an update for a
                // message the cache never held, which is ignored.
                None => {
                    if let Some(received) = received
                        && window.is_none_or(|w| received >= w)
                    {
                        new.push(IdListed {
                            id,
                            flags: flags.unwrap_or_default(),
                            received,
                        });
                    }
                }
            },
        }
    }
    let gone = cache.apply_id_changes(mailbox_id, &updates, &removed)?;
    // A message this sync stored and a starting round then removed (deleted or moved out between
    // the listing and the round) was never in the cache for anyone: neither new nor removed.
    let before = report.new_ids.len();
    report.new_ids.retain(|id| !gone.contains(id));
    let unseen = before - report.new_ids.len();
    report.new = report.new.saturating_sub(unseen);
    report.updated += updates.len();
    report.removed += gone.len().saturating_sub(unseen);

    let newest_new = new.iter().map(|m| m.received).max();
    let oldest_missing = fetch_new(
        source,
        cache,
        mailbox_id,
        new.iter().collect(),
        report,
        progress,
    )
    .await?;
    if oldest_missing.is_some() {
        // `store_id_messages` lowered `seen_until` to it: the next sync lists from there.
        cache.set_cursor(mailbox_id, None)?;
        return Ok(());
    }
    if let Some(newest) = newest_new
        && seen_until.is_none_or(|s| newest > s)
    {
        cache.set_seen_until(mailbox_id, newest)?;
    }
    cache.set_cursor(mailbox_id, Some(&round.cursor))?;
    Ok(())
}

/// Fetches and stores `new`, oldest arrival first, [`FETCH_BATCH`] per request and per
/// transaction, reporting progress. Returns the oldest arrival that came back without a body.
async fn fetch_new<S: IdSource>(
    source: &mut S,
    cache: &mut Cache,
    mailbox_id: i64,
    mut new: Vec<&IdListed>,
    report: &mut SyncReport,
    progress: &mut impl FnMut(Progress),
) -> Result<Option<DateTime<Utc>>, SyncError<S::Error>> {
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
        let stored = cache.store_id_messages(mailbox_id, &store, oldest_missing)?;
        report.new += store.len();
        report.new_ids.extend(stored);
        done.done += batch.len();
        progress(done);
    }
    Ok(oldest_missing)
}

/// The `limit` messages of `listing` that arrived last, once each: a source may return more.
fn newest(mut listing: Vec<IdListed>, limit: NonZeroU32) -> Vec<IdListed> {
    listing.sort_by(|a, b| a.id.cmp(&b.id));
    listing.dedup_by(|a, b| a.id == b.id);
    listing.sort_by(|a, b| (b.received, &b.id).cmp(&(a.received, &a.id)));
    listing.truncate(usize::try_from(limit.get()).unwrap_or(usize::MAX));
    listing
}
