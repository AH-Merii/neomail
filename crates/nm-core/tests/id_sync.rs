//! The by-id sync engine against an in-memory reference IdSource: no network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use nm_core::cache::MailboxKind;
use nm_core::{
    Cache, FetchedMessage, Flags, IdFetched, IdListed, IdSource, MessageId, Progress, RemoteId,
    SyncReport, Uid, UidValidity, sync_inbox_by_id, sync_inbox_by_id_with_progress,
};

const T0: i64 = 1_790_000_000;

/// Message `n` arrived `n` minutes after T0.
fn at(n: u32) -> DateTime<Utc> {
    DateTime::from_timestamp(T0 + i64::from(n) * 60, 0).unwrap()
}

/// Ids sort the other way round from arrival, so arrival order cannot come from id order.
fn id(n: u32) -> RemoteId {
    RemoteId::new(format!("AAMk{}=", 1000 - n)).unwrap()
}

fn num(id: &RemoteId) -> Option<u32> {
    let s = id.as_str();
    let n = 1000 - s.get(4..s.len() - 1)?.parse::<u32>().ok()?;
    (id == &self::id(n)).then_some(n)
}

fn raw(n: u32) -> Vec<u8> {
    format!("From: sender{n}@mail.example\r\nSubject: Message {n}\r\n\r\nBody {n}\r\n").into_bytes()
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Newest(u32),
    /// The message number whose arrival `since` was.
    Since(u32),
}

#[derive(Debug, thiserror::Error)]
#[error("the connection dropped")]
struct Dropped;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum SinceMode {
    /// What the contract asks for, plus one repeat.
    #[default]
    Honest,
    /// Every message, also older ones: allowed, the engine filters.
    Everything,
    /// Only the newest message: breaks the contract.
    #[cfg_attr(not(feature = "conformance"), allow(dead_code))]
    IgnoresSince,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "conformance"), allow(dead_code))]
enum NewestMode {
    #[default]
    Honest,
    /// The oldest `limit` messages: breaks the contract.
    Oldest,
    /// Lists nothing, from `newest` or `arrived_since`: a source that does nothing.
    Empty,
}

/// An in-memory INBOX: message `n` has id `id(n)` and arrived at `at(n)`.
#[derive(Default)]
struct RefSource {
    messages: BTreeMap<u32, Flags>,
    calls: Vec<Call>,
    fetched: Vec<u32>,
    /// Messages whose fetch comes back without them.
    bodiless: Vec<u32>,
    /// Fetch requests that succeed before the connection drops.
    fetches_left: Option<usize>,
    since_mode: SinceMode,
    newest_mode: NewestMode,
    /// Fetch returns the message as a JSON resource, not its RFC 822 source: breaks the contract.
    #[cfg_attr(not(feature = "conformance"), allow(dead_code))]
    fetches_json: bool,
}

impl RefSource {
    fn new(messages: impl IntoIterator<Item = u32>) -> Self {
        Self {
            messages: messages
                .into_iter()
                .map(|n| (n, Flags::default()))
                .collect(),
            ..Self::default()
        }
    }

    fn listed(&self, n: u32) -> IdListed {
        IdListed {
            id: id(n),
            flags: self.messages[&n].clone(),
            received: at(n),
        }
    }
}

impl IdSource for RefSource {
    type Error = Dropped;

    async fn newest(&mut self, limit: NonZeroU32) -> Result<Vec<IdListed>, Dropped> {
        self.calls.push(Call::Newest(limit.get()));
        let take = usize::try_from(limit.get()).unwrap();
        let keys = self.messages.keys();
        let chosen: Vec<u32> = match self.newest_mode {
            NewestMode::Honest => keys.rev().take(take).copied().collect(),
            NewestMode::Oldest => keys.take(take).copied().collect(),
            NewestMode::Empty => Vec::new(),
        };
        Ok(chosen.into_iter().map(|n| self.listed(n)).collect())
    }

    async fn arrived_since(&mut self, since: DateTime<Utc>) -> Result<Vec<IdListed>, Dropped> {
        let n = u32::try_from((since.timestamp() - T0) / 60).unwrap();
        self.calls.push(Call::Since(n));
        if self.newest_mode == NewestMode::Empty {
            return Ok(Vec::new());
        }
        let mut out: Vec<IdListed> = match self.since_mode {
            SinceMode::Honest => self
                .messages
                .range(n..)
                .map(|(n, _)| self.listed(*n))
                .collect(),
            SinceMode::Everything => self.messages.keys().map(|n| self.listed(*n)).collect(),
            SinceMode::IgnoresSince => self
                .messages
                .keys()
                .last()
                .map(|n| self.listed(*n))
                .into_iter()
                .collect(),
        };
        out.extend(out.first().cloned());
        Ok(out)
    }

    async fn fetch(&mut self, ids: &[RemoteId]) -> Result<Vec<IdFetched>, Dropped> {
        if let Some(left) = &mut self.fetches_left {
            *left = left.checked_sub(1).ok_or(Dropped)?;
        }
        let mut out = Vec::new();
        for id in ids {
            let Some(n) = num(id) else { continue };
            if self.messages.contains_key(&n) && !self.bodiless.contains(&n) {
                self.fetched.push(n);
                let raw = if self.fetches_json {
                    format!("{{\"id\":\"{id}\",\"subject\":\"Message {n}\"}}").into_bytes()
                } else {
                    raw(n)
                };
                out.push(IdFetched {
                    id: id.clone(),
                    raw,
                });
            }
        }
        Ok(out)
    }
}

const LIMIT: NonZeroU32 = NonZeroU32::new(3).unwrap();

async fn sync(source: &mut RefSource, cache: &mut Cache) -> SyncReport {
    source.fetched.clear();
    source.calls.clear();
    sync_inbox_by_id(source, cache, "work", LIMIT)
        .await
        .unwrap()
}

fn report(new: usize, updated: usize, removed: usize, reset: bool) -> SyncReport {
    SyncReport {
        new,
        updated,
        removed,
        missing: 0,
        reset,
    }
}

fn mailbox(cache: &Cache) -> (i64, Option<DateTime<Utc>>) {
    let mb = cache.mailbox("work", "INBOX").unwrap().unwrap();
    match mb.kind {
        MailboxKind::ById { seen_until } => (mb.id, seen_until),
        MailboxKind::Imap { .. } => panic!("not a by-id mailbox"),
    }
}

/// The cached message numbers, ascending.
fn cached(cache: &Cache) -> Vec<u32> {
    let mut ns: Vec<u32> = cache
        .cached_ids(mailbox(cache).0)
        .unwrap()
        .iter()
        .map(|m| num(&m.id).unwrap())
        .collect();
    ns.sort_unstable();
    ns
}

#[tokio::test]
async fn first_sync_takes_the_newest_and_second_fetches_nothing() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=5);

    assert_eq!(sync(&mut source, &mut cache).await, report(3, 0, 0, false));
    assert_eq!(source.calls, [Call::Newest(3)]);
    assert_eq!(source.fetched, [3, 4, 5], "oldest arrival first");
    assert_eq!(cached(&cache), [3, 4, 5]);
    assert_eq!(mailbox(&cache).1, Some(at(5)));
    // Parsed as the IMAP path parses it; flags and arrival come from the listing.
    let list = cache.list(Some("work"), 10).unwrap();
    assert_eq!(list[0].subject.as_deref(), Some("Message 5"));
    assert_eq!(list[0].uid, None);
    assert_eq!(
        cache.get(list[0].id).unwrap().unwrap().received,
        Some(at(5))
    );

    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
    assert_eq!(
        source.calls,
        [Call::Since(3)],
        "the window starts at the oldest cached"
    );
    assert!(source.fetched.is_empty());
}

#[tokio::test]
async fn progress_counts_up_per_batch_and_stays_quiet_with_nothing_new() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=120);
    let limit = NonZeroU32::new(100).unwrap();
    let mut seen = Vec::new();
    let r =
        sync_inbox_by_id_with_progress(&mut source, &mut cache, "work", limit, |p| seen.push(p))
            .await
            .unwrap();
    assert_eq!(r.new, 100);
    let done: Vec<(usize, usize)> = seen.iter().map(|p| (p.done, p.total)).collect();
    assert_eq!(done, [(0, 100), (50, 100), (100, 100)]);

    let mut again: Vec<Progress> = Vec::new();
    let r =
        sync_inbox_by_id_with_progress(&mut source, &mut cache, "work", limit, |p| again.push(p))
            .await
            .unwrap();
    assert_eq!(r, report(0, 0, 0, false));
    assert!(again.is_empty(), "{again:?}");
}

#[tokio::test]
async fn read_and_removed_messages_are_applied() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=5);
    sync(&mut source, &mut cache).await;

    source.messages.insert(4, Flags::new([Flags::SEEN]));
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 1, 0, false));
    let seen: Vec<bool> = cache
        .list(Some("work"), 10)
        .unwrap()
        .iter()
        .map(|e| e.flags.is_seen())
        .collect();
    assert_eq!(seen, [false, true, false]);

    source.messages.remove(&3);
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 1, false));
    assert_eq!(cached(&cache), [4, 5]);
    assert!(
        source.fetched.is_empty(),
        "message 2 is older than the window"
    );
}

#[tokio::test]
async fn an_interrupted_first_sync_is_completed_by_the_next() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=120);
    source.fetches_left = Some(1);
    let limit = NonZeroU32::new(100).unwrap();
    assert!(
        sync_inbox_by_id(&mut source, &mut cache, "work", limit)
            .await
            .is_err()
    );
    assert_eq!(
        cached(&cache),
        (21..=70).collect::<Vec<_>>(),
        "the oldest 50 of the newest 100"
    );
    assert_eq!(mailbox(&cache).1, None);

    source.fetches_left = None;
    source.fetched.clear();
    let r = sync_inbox_by_id(&mut source, &mut cache, "work", limit)
        .await
        .unwrap();
    assert_eq!(r, report(50, 0, 0, false));
    assert_eq!(source.fetched, (71..=120).collect::<Vec<_>>());
    assert_eq!(cached(&cache), (21..=120).collect::<Vec<_>>());
    assert_eq!(mailbox(&cache).1, Some(at(120)));
}

#[tokio::test]
async fn a_message_without_a_body_is_asked_for_again() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=5);
    source.bodiless.push(4);
    let r = sync(&mut source, &mut cache).await;
    assert_eq!((r.new, r.missing), (2, 1));
    assert_eq!(cached(&cache), [3, 5]);
    assert_eq!(mailbox(&cache).1, Some(at(4)));

    source.bodiless.clear();
    let r = sync(&mut source, &mut cache).await;
    assert_eq!((r.new, r.missing), (1, 0));
    assert_eq!(source.fetched, [4]);
    assert_eq!(cached(&cache), [3, 4, 5]);
    assert_eq!(mailbox(&cache).1, Some(at(5)));

    // The oldest of the chosen comes back without a body: the window still reaches it.
    let mut cache = Cache::open_in_memory().unwrap();
    source.bodiless.push(3);
    sync(&mut source, &mut cache).await;
    source.bodiless.clear();
    sync(&mut source, &mut cache).await;
    assert_eq!(source.calls, [Call::Since(3)]);
    assert_eq!(source.fetched, [3]);
}

#[tokio::test]
async fn a_missing_body_survives_a_later_failed_batch() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=120);
    source.bodiless.push(21);
    source.fetches_left = Some(1);
    let limit = NonZeroU32::new(100).unwrap();
    assert!(
        sync_inbox_by_id(&mut source, &mut cache, "work", limit)
            .await
            .is_err()
    );
    assert_eq!(cached(&cache), (22..=70).collect::<Vec<_>>());
    assert_eq!(mailbox(&cache).1, Some(at(21)), "saved with batch 1");

    source.bodiless.clear();
    source.fetches_left = None;
    source.fetched.clear();
    source.calls.clear();
    let r = sync_inbox_by_id(&mut source, &mut cache, "work", limit)
        .await
        .unwrap();
    assert_eq!(r, report(51, 0, 0, false));
    assert_eq!(source.calls, [Call::Since(21)]);
    assert_eq!(source.fetched[0], 21);
    assert_eq!(cached(&cache), (21..=120).collect::<Vec<_>>());
    assert_eq!(mailbox(&cache).1, Some(at(120)));
}

#[tokio::test]
async fn more_than_limit_new_after_the_cache_emptied_are_all_fetched() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=5);
    sync(&mut source, &mut cache).await;

    source.messages.clear();
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 3, false));
    assert_eq!(
        mailbox(&cache).1,
        Some(at(5)),
        "kept while nothing is listed"
    );

    source
        .messages
        .extend((6..=9).map(|n| (n, Flags::default())));
    assert_eq!(sync(&mut source, &mut cache).await, report(4, 0, 0, false));
    assert_eq!(source.calls, [Call::Since(5)]);
    assert_eq!(source.fetched, [6, 7, 8, 9]);
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
}

#[tokio::test]
async fn an_empty_inbox_lists_the_newest_again_and_extra_results_are_ignored() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new([]);
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
    assert_eq!(mailbox(&cache).1, None);
    source
        .messages
        .extend((1..=5).map(|n| (n, Flags::default())));
    assert_eq!(sync(&mut source, &mut cache).await, report(3, 0, 0, false));
    assert_eq!(source.calls, [Call::Newest(3)]);

    // Older messages outside the window are not fetched, and repeats are fetched once.
    source.since_mode = SinceMode::Everything;
    source.messages.insert(6, Flags::default());
    assert_eq!(sync(&mut source, &mut cache).await, report(1, 0, 0, false));
    assert_eq!(source.fetched, [6]);
}

#[tokio::test]
async fn an_imap_mailbox_is_reset_to_by_id() {
    let mut cache = Cache::open_in_memory().unwrap();
    let imap = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(7).unwrap())
        .unwrap();
    let old: Vec<FetchedMessage> = (1..=2)
        .map(|u| FetchedMessage {
            uid: Uid::new(u).unwrap(),
            flags: Flags::default(),
            received: None,
            raw: raw(u),
        })
        .collect();
    cache.insert_messages(imap.id, &old).unwrap();

    let mut source = RefSource::new(1..=5);
    assert_eq!(sync(&mut source, &mut cache).await, report(3, 0, 2, true));
    assert_eq!(cached(&cache), [3, 4, 5]);
    assert!(
        cache
            .list(Some("work"), 10)
            .unwrap()
            .iter()
            .all(|e| e.uid.is_none())
    );
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
}

#[tokio::test]
async fn ids_stay_stable_across_syncs() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = RefSource::new(1..=5);
    sync(&mut source, &mut cache).await;
    let ids = |cache: &Cache| -> BTreeMap<String, MessageId> {
        cache
            .list(Some("work"), 100)
            .unwrap()
            .into_iter()
            .map(|e| (e.subject.unwrap(), e.id))
            .collect()
    };
    let before = ids(&cache);

    source.messages.insert(4, Flags::new([Flags::SEEN]));
    source.messages.remove(&5);
    source.messages.insert(6, Flags::default());
    sync(&mut source, &mut cache).await;
    let after = ids(&cache);
    for (subject, id) in &after {
        match before.get(subject) {
            Some(old) => assert_eq!(old, id, "{subject}"),
            None => assert!(
                before.values().all(|old| old != id),
                "a fresh id for {subject}"
            ),
        }
    }
    assert_eq!(after.len(), 3);
}

#[cfg(feature = "conformance")]
#[tokio::test]
async fn the_reference_source_conforms() {
    let mut source = RefSource::new(1..=12);
    source.messages.insert(11, Flags::new([Flags::SEEN]));
    nm_core::conformance::id_source(&mut source).await.unwrap();
}

#[cfg(feature = "conformance")]
#[tokio::test]
async fn a_source_that_ignores_since_fails_conformance() {
    let mut source = RefSource::new(1..=12);
    source.since_mode = SinceMode::IgnoresSince;
    let err = nm_core::conformance::id_source(&mut source)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("arrived_since"), "{err}");
}

#[cfg(feature = "conformance")]
#[tokio::test]
async fn a_source_that_fetches_json_instead_of_mail_fails_conformance() {
    let mut source = RefSource::new(1..=12);
    source.fetches_json = true;
    let err = nm_core::conformance::id_source(&mut source)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not parse"), "{err}");
    assert!(
        matches!(err, nm_core::conformance::ConformanceError::Contract(_)),
        "{err}"
    );
}

#[cfg(feature = "conformance")]
#[tokio::test]
async fn a_source_whose_newest_lists_the_oldest_fails_conformance() {
    let mut source = RefSource::new(1..=12);
    source.newest_mode = NewestMode::Oldest;
    let err = nm_core::conformance::id_source(&mut source)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("newest"), "{err}");
    assert!(
        matches!(err, nm_core::conformance::ConformanceError::Contract(_)),
        "{err}"
    );
}

#[cfg(feature = "conformance")]
#[tokio::test]
async fn a_source_that_lists_nothing_fails_conformance() {
    let mut source = RefSource::new(1..=12);
    source.newest_mode = NewestMode::Empty;
    let err = nm_core::conformance::id_source(&mut source)
        .await
        .unwrap_err();
    assert!(
        matches!(err, nm_core::conformance::ConformanceError::Precondition(_)),
        "{err}"
    );

    // An inbox of one message cannot show the order either.
    let mut source = RefSource::new([1]);
    let err = nm_core::conformance::id_source(&mut source)
        .await
        .unwrap_err();
    assert!(
        matches!(err, nm_core::conformance::ConformanceError::Precondition(_)),
        "{err}"
    );
}
