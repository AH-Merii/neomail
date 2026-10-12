//! The by-id sync engine with a source that tracks changes since a cursor (Graph's delta query,
//! later Gmail's history): an in-memory INBOX with a change log. No network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use nm_core::{
    Cache, Cursor, Flags, IdChange, IdChanges, IdFetched, IdListed, IdSource, RemoteId, SyncReport,
    sync_inbox_by_id,
};

const T0: i64 = 1_790_000_000;
const ACCOUNT: &str = "outlook";

fn at(n: u32) -> DateTime<Utc> {
    DateTime::from_timestamp(T0 + i64::from(n) * 60, 0).unwrap()
}

fn id(n: u32) -> RemoteId {
    RemoteId::new(format!("AAMk{n}=")).unwrap()
}

fn num(id: &RemoteId) -> u32 {
    id.as_str()[4..id.as_str().len() - 1].parse().unwrap()
}

fn seen(read: bool) -> Flags {
    if read {
        Flags::new([Flags::SEEN])
    } else {
        Flags::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Newest,
    /// The message number whose arrival the window started at.
    Since(u32),
    Start(u32),
    /// The log position the cursor stood at.
    Changes(usize),
}

#[derive(Debug, thiserror::Error)]
#[error("the connection dropped")]
struct Dropped;

/// Message `n` arrived at `at(n)`; `read` is its read state. Every change goes to `log`, and a
/// cursor is a position in it and the message number the round started at. As Graph's filtered
/// delta query did live, a round reports removals of any message but other changes only for
/// messages that arrived at or after its start.
#[derive(Default)]
struct DeltaSource {
    read: BTreeMap<u32, bool>,
    log: Vec<IdChange>,
    calls: Vec<Call>,
    fetched: Vec<u32>,
    bodiless: Vec<u32>,
    fetch_fails: bool,
    /// The provider refuses every cursor (expired or reset).
    refuses: bool,
    /// Every round is sent twice over.
    replays: bool,
    /// A start round over at least this many messages is refused, as the Graph source refuses
    /// one that reached Graph's 5,000.
    start_cap: Option<usize>,
    /// Rounds also report changes to messages older than their start, as Microsoft says Graph
    /// can.
    leaks: bool,
    /// Arrives just before a round starts, after the listing.
    arrives_at_start: Option<u32>,
    /// Is deleted or moved out just before a round starts, after the listing.
    removed_at_start: Option<u32>,
}

impl DeltaSource {
    fn new(messages: impl IntoIterator<Item = u32>) -> Self {
        Self {
            read: messages.into_iter().map(|n| (n, false)).collect(),
            ..Self::default()
        }
    }

    fn arrive(&mut self, n: u32) {
        self.read.insert(n, false);
        self.log.push(IdChange::Present {
            id: id(n),
            flags: Some(seen(false)),
            received: Some(at(n)),
        });
    }

    /// A read change, which names only the id and what changed.
    fn mark_read(&mut self, n: u32) {
        self.read.insert(n, true);
        self.log.push(IdChange::Present {
            id: id(n),
            flags: Some(seen(true)),
            received: None,
        });
    }

    fn remove(&mut self, n: u32) {
        self.read.remove(&n);
        self.log.push(IdChange::Removed(id(n)));
    }

    fn cursor(&self, from: u32) -> Cursor {
        Cursor::new(format!("{}/{from}", self.log.len())).unwrap()
    }

    fn listed(&self, n: u32) -> IdListed {
        IdListed {
            id: id(n),
            flags: seen(self.read[&n]),
            received: at(n),
        }
    }

    fn take_calls(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.calls)
    }
}

fn minute(t: DateTime<Utc>) -> u32 {
    u32::try_from((t.timestamp() - T0) / 60).unwrap()
}

impl IdSource for DeltaSource {
    type Error = Dropped;

    async fn newest(&mut self, limit: NonZeroU32) -> Result<Vec<IdListed>, Dropped> {
        self.calls.push(Call::Newest);
        let keys: Vec<u32> = self
            .read
            .keys()
            .rev()
            .take(limit.get() as usize)
            .copied()
            .collect();
        Ok(keys.into_iter().map(|n| self.listed(n)).collect())
    }

    async fn arrived_since(&mut self, since: DateTime<Utc>) -> Result<Vec<IdListed>, Dropped> {
        let from = minute(since);
        self.calls.push(Call::Since(from));
        let keys: Vec<u32> = self.read.range(from..).map(|(n, _)| *n).collect();
        Ok(keys.into_iter().map(|n| self.listed(n)).collect())
    }

    async fn fetch(&mut self, ids: &[RemoteId]) -> Result<Vec<IdFetched>, Dropped> {
        if self.fetch_fails {
            return Err(Dropped);
        }
        let mut out = Vec::new();
        for n in ids.iter().map(num) {
            if self.read.contains_key(&n) && !self.bodiless.contains(&n) {
                self.fetched.push(n);
                out.push(IdFetched {
                    id: id(n),
                    raw: format!("From: a{n}@mail.example\r\nSubject: {n}\r\n\r\nBody\r\n").into(),
                });
            }
        }
        Ok(out)
    }

    async fn start_changes(&mut self, from: DateTime<Utc>) -> Result<Option<IdChanges>, Dropped> {
        self.calls.push(Call::Start(minute(from)));
        if let Some(n) = self.arrives_at_start.take() {
            self.arrive(n);
        }
        if let Some(n) = self.removed_at_start.take() {
            self.remove(n);
        }
        let keys: Vec<u32> = self
            .read
            .range(minute(from)..)
            .rev()
            .map(|(n, _)| *n)
            .collect();
        if self.start_cap.is_some_and(|cap| keys.len() >= cap) {
            return Ok(None);
        }
        let changes = keys
            .iter()
            .map(|n| IdChange::Present {
                id: id(*n),
                flags: Some(seen(self.read[n])),
                received: Some(at(*n)),
            })
            .collect();
        Ok(Some(IdChanges {
            changes,
            cursor: self.cursor(minute(from)),
        }))
    }

    async fn changes_since(&mut self, cursor: &Cursor) -> Result<Option<IdChanges>, Dropped> {
        let (position, start) = cursor.as_str().split_once('/').unwrap();
        let (position, start): (usize, u32) = (position.parse().unwrap(), start.parse().unwrap());
        self.calls.push(Call::Changes(position));
        if self.refuses {
            return Ok(None);
        }
        let mut changes: Vec<IdChange> = self.log[position..]
            .iter()
            .filter(|c| matches!(c, IdChange::Removed(_)) || self.leaks || num(c.id()) >= start)
            .cloned()
            .collect();
        if self.replays {
            changes.extend(changes.clone());
        }
        Ok(Some(IdChanges {
            changes,
            cursor: self.cursor(start),
        }))
    }
}

async fn sync(source: &mut DeltaSource, cache: &mut Cache) -> SyncReport {
    sync_inbox_by_id(source, cache, ACCOUNT, NonZeroU32::new(3).unwrap())
        .await
        .unwrap()
}

/// The cached messages and whether each is read.
fn cached(cache: &Cache) -> Vec<(u32, bool)> {
    let mailbox = cache.mailbox(ACCOUNT, "INBOX").unwrap().unwrap();
    cache
        .cached_ids(mailbox.id)
        .unwrap()
        .iter()
        .map(|m| (num(&m.id), m.flags.is_seen()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn counts(r: &SyncReport) -> (usize, usize, usize, usize) {
    (r.new, r.updated, r.removed, r.missing)
}

/// A cache synced once from messages 1 to 5 (the newest 3 kept), with a round started.
async fn synced(source: &mut DeltaSource) -> Cache {
    let mut cache = Cache::open_in_memory().unwrap();
    sync(source, &mut cache).await;
    // The round starts at the oldest cached message, so it covers every one.
    assert_eq!(source.take_calls(), [Call::Newest, Call::Start(3)]);
    assert_eq!(cached(&cache), [(3, false), (4, false), (5, false)]);
    cache
}

#[tokio::test]
async fn after_the_first_sync_only_changes_are_asked_for_and_applied() {
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    let position = source.log.len();

    source.arrive(6);
    source.mark_read(4);
    source.remove(3);
    source.replays = true;
    source.fetched.clear();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Changes(position)]);
    assert_eq!(source.fetched, [6], "fetched once, though replayed");
    assert_eq!(counts(&r), (1, 1, 1, 0));
    assert_eq!(r.new_ids.len(), 1);
    assert_eq!(cached(&cache), [(4, true), (5, false), (6, false)]);

    // Nothing new: one round, nothing fetched, nothing reported.
    source.fetched.clear();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Changes(source.log.len())]);
    assert!(source.fetched.is_empty());
    assert_eq!(counts(&r), (0, 0, 0, 0));
}

#[tokio::test]
async fn a_change_that_reads_and_then_removes_a_message_removes_it() {
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    source.mark_read(5);
    source.remove(5);
    source.arrive(7);
    source.remove(7);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(counts(&r), (0, 0, 1, 0));
    assert_eq!(cached(&cache), [(3, false), (4, false)]);
}

#[tokio::test]
async fn a_missing_body_is_asked_for_again_by_listing() {
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    source.arrive(6);
    source.arrive(7);
    source.bodiless = vec![6];
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(counts(&r), (1, 0, 0, 1));
    assert_eq!(cached(&cache).len(), 4);
    source.take_calls();

    source.bodiless.clear();
    source.fetched.clear();
    let r = sync(&mut source, &mut cache).await;
    // The window reaches back to the oldest cached message, which covers 6.
    assert_eq!(source.take_calls(), [Call::Since(3), Call::Start(3)]);
    assert_eq!(source.fetched, [6]);
    assert_eq!(counts(&r), (1, 0, 0, 0));

    // Then rounds again.
    sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Changes(source.log.len())]);
}

#[tokio::test]
async fn an_update_for_an_uncached_message_older_than_the_window_is_ignored() {
    // Messages 1 and 2 are in the INBOX but older than the 3 the first sync kept.
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    source.leaks = true;
    source.mark_read(1);
    // An old message that changed in full (Graph may send every selected field).
    source.log.push(IdChange::Present {
        id: id(2),
        flags: Some(seen(true)),
        received: Some(at(2)),
    });
    source.arrive(6);
    source.fetched.clear();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.fetched, [6]);
    assert_eq!(counts(&r), (1, 0, 0, 0));
    assert_eq!(
        cached(&cache),
        [(3, false), (4, false), (5, false), (6, false)]
    );
}

#[tokio::test]
async fn a_read_change_to_a_cached_message_older_than_the_newest_comes_back() {
    // The round started at 3, the oldest cached message, so the source reports changes to 3 and
    // 4 though 5 is newer.
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    source.mark_read(3);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(counts(&r), (0, 1, 0, 0));
    assert_eq!(cached(&cache), [(3, true), (4, false), (5, false)]);

    source.mark_read(4);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(counts(&r), (0, 1, 0, 0));
    assert_eq!(cached(&cache), [(3, true), (4, true), (5, false)]);
}

#[tokio::test]
async fn a_window_too_big_for_a_round_keeps_listing_and_never_drops_or_misses_mail() {
    // A source refuses a round over the whole window (Graph's 5,000 with $filter): no cursor is
    // stored, every sync lists, and nothing cached is dropped.
    let mut source = DeltaSource::new(1..=5);
    source.start_cap = Some(3);
    let mut cache = Cache::open_in_memory().unwrap();
    sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Newest, Call::Start(3)]);
    assert_eq!(cached(&cache), [(3, false), (4, false), (5, false)]);

    source.arrive(6);
    source.mark_read(3);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Since(3), Call::Start(3)]);
    assert_eq!(counts(&r), (1, 1, 0, 0));
    assert_eq!(
        cached(&cache),
        [(3, true), (4, false), (5, false), (6, false)]
    );
}

#[tokio::test]
async fn mail_that_arrives_as_the_round_starts_comes_with_the_round() {
    // Arrives after the listing, as the round starts: only the round can bring it.
    let mut source = DeltaSource::new(1..=5);
    source.arrives_at_start = Some(6);
    let mut cache = Cache::open_in_memory().unwrap();
    sync(&mut source, &mut cache).await;
    assert_eq!(
        cached(&cache),
        [(3, false), (4, false), (5, false), (6, false)]
    );

    source.arrive(7);
    sync(&mut source, &mut cache).await;
    assert_eq!(
        cached(&cache),
        [(3, false), (4, false), (5, false), (6, false), (7, false)]
    );
}

#[tokio::test]
async fn mail_removed_between_the_listing_and_the_round_leaves_the_cache() {
    // Listed and fetched, then deleted before the round starts: the round leaves it out, and no
    // later round reports the removal, which came before the round's state.
    let mut source = DeltaSource::new(1..=5);
    source.removed_at_start = Some(4);
    let mut cache = Cache::open_in_memory().unwrap();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Newest, Call::Start(3)]);
    // Stored and gone in the same sync: neither new nor removed.
    assert_eq!(counts(&r), (2, 0, 0, 0));
    assert_eq!(r.new_ids.len(), 2);
    assert_eq!(cached(&cache), [(3, false), (5, false)]);
    sync(&mut source, &mut cache).await;
    assert_eq!(cached(&cache), [(3, false), (5, false)]);

    // The same for a cached message on a later listing sync, here the oldest, where the round
    // starts.
    source.refuses = true;
    source.removed_at_start = Some(3);
    source.take_calls();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls()[1..], [Call::Since(3), Call::Start(3)]);
    assert_eq!(counts(&r), (0, 0, 1, 0));
    assert_eq!(cached(&cache), [(5, false)]);
    source.refuses = false;
    sync(&mut source, &mut cache).await;
    assert_eq!(cached(&cache), [(5, false)]);
}

#[tokio::test]
async fn new_mail_removed_before_the_round_is_not_reported_as_new() {
    // A later listing sync fetches 6, which is deleted before the round starts: the report must
    // not name it, or the daemon would announce mail that is not in the cache.
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    source.arrive(6);
    source.refuses = true;
    source.removed_at_start = Some(6);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(source.fetched, [3, 4, 5, 6]);
    assert_eq!(counts(&r), (0, 0, 0, 0));
    assert!(!r.initial);
    assert!(r.new_ids.is_empty());
    assert_eq!(cached(&cache), [(3, false), (4, false), (5, false)]);
}

#[tokio::test]
async fn the_first_mail_after_an_empty_first_sync_comes_though_stamped_before_the_local_clock() {
    // An empty INBOX gives the round only the local clock to start from. When it runs ahead of
    // the server's, the next mail is stamped before the local time of the first sync.
    let mut source = DeltaSource::new([]);
    let mut cache = Cache::open_in_memory().unwrap();
    let local_start = minute(Utc::now());
    sync(&mut source, &mut cache).await;
    assert!(cached(&cache).is_empty());

    let n = local_start - 5;
    source.arrive(n);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(cached(&cache), [(n, false)]);
    assert_eq!(counts(&r), (1, 0, 0, 0));
    assert!(!r.initial, "new mail, not the first sync");
}

#[tokio::test]
async fn a_later_round_never_drops_what_it_leaves_out() {
    // Only a starting round is complete: a later one that names nothing keeps every message.
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(counts(&r), (0, 0, 0, 0));
    assert_eq!(cached(&cache), [(3, false), (4, false), (5, false)]);
}

#[tokio::test]
async fn a_sync_that_fails_mid_round_keeps_the_previous_cursor() {
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    let position = source.log.len();
    source.arrive(6);
    source.mark_read(5);
    source.fetch_fails = true;
    let mut report = SyncReport::default();
    let failed = nm_core::sync_inbox_by_id_with_progress(
        &mut source,
        &mut cache,
        ACCOUNT,
        NonZeroU32::new(3).unwrap(),
        &mut report,
        |_| {},
    )
    .await;
    assert!(failed.is_err());
    assert_eq!(source.take_calls(), [Call::Changes(position)]);

    source.fetch_fails = false;
    source.fetched.clear();
    sync(&mut source, &mut cache).await;
    assert_eq!(
        source.take_calls(),
        [Call::Changes(position)],
        "the round again"
    );
    assert_eq!(source.fetched, [6]);
    assert_eq!(
        cached(&cache),
        [(3, false), (4, false), (5, true), (6, false)]
    );
}

#[tokio::test]
async fn a_refused_cursor_lists_and_starts_a_new_round() {
    let mut source = DeltaSource::new(1..=5);
    let mut cache = synced(&mut source).await;
    let position = source.log.len();
    source.remove(4);
    source.arrive(6);
    source.refuses = true;
    let r = sync(&mut source, &mut cache).await;
    assert_eq!(
        source.take_calls(),
        [Call::Changes(position), Call::Since(3), Call::Start(3)]
    );
    assert_eq!(counts(&r), (1, 0, 1, 0));
    assert_eq!(cached(&cache), [(3, false), (5, false), (6, false)]);

    source.refuses = false;
    sync(&mut source, &mut cache).await;
    assert_eq!(source.take_calls(), [Call::Changes(source.log.len())]);
}
