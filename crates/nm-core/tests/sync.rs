//! The sync engine against a fake MailSource: no network.
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use nm_core::{
    Cache, FetchedMessage, Flags, ListedMessage, MailSource, MailboxStatus, SyncReport, Uid,
    UidFlags, UidValidity, sync_inbox,
};

/// An in-memory INBOX that records which UIDs were fetched.
struct FakeSource {
    validity: u32,
    messages: BTreeMap<u32, Flags>,
    /// INTERNALDATEs by UID; a message without one has no date.
    dates: BTreeMap<u32, DateTime<Utc>>,
    fetched: Vec<u32>,
}

impl FakeSource {
    fn new(validity: u32, uids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            validity,
            messages: uids.into_iter().map(|u| (u, Flags::default())).collect(),
            dates: BTreeMap::new(),
            fetched: Vec::new(),
        }
    }

    fn listing(&self, uids: impl Iterator<Item = u32>) -> Vec<UidFlags> {
        uids.map(|u| UidFlags {
            uid: Uid::new(u).unwrap(),
            flags: self.messages[&u].clone(),
        })
        .collect()
    }
}

fn raw(uid: u32) -> Vec<u8> {
    format!("From: sender{uid}@mail.example\r\nSubject: Message {uid}\r\n\r\nBody {uid}\r\n")
        .into_bytes()
}

impl MailSource for FakeSource {
    type Error = Infallible;

    async fn examine_inbox(&mut self) -> Result<MailboxStatus, Infallible> {
        Ok(MailboxStatus {
            uid_validity: UidValidity::new(self.validity).unwrap(),
            exists: u32::try_from(self.messages.len()).unwrap(),
        })
    }

    async fn list_all(&mut self) -> Result<Vec<ListedMessage>, Infallible> {
        Ok(self
            .messages
            .iter()
            .map(|(u, flags)| ListedMessage {
                uid: Uid::new(*u).unwrap(),
                flags: flags.clone(),
                received: self.dates.get(u).copied(),
            })
            .collect())
    }

    async fn flags_from(&mut self, from: Uid) -> Result<Vec<UidFlags>, Infallible> {
        let mut keys: Vec<u32> = self.messages.range(from.get()..).map(|(u, _)| *u).collect();
        // Like a real server, `n:*` past the end still returns the last message.
        if keys.is_empty() {
            keys.extend(self.messages.keys().next_back());
        }
        Ok(self.listing(keys.into_iter()))
    }

    async fn fetch(&mut self, uids: &[Uid]) -> Result<Vec<FetchedMessage>, Infallible> {
        let mut out = Vec::new();
        for uid in uids {
            if let Some(flags) = self.messages.get(&uid.get()) {
                self.fetched.push(uid.get());
                out.push(FetchedMessage {
                    uid: *uid,
                    flags: flags.clone(),
                    received: self.dates.get(&uid.get()).copied(),
                    raw: raw(uid.get()),
                });
            }
        }
        Ok(out)
    }
}

const LIMIT: NonZeroU32 = NonZeroU32::new(3).unwrap();

async fn sync(source: &mut FakeSource, cache: &mut Cache) -> SyncReport {
    let report = sync_inbox(source, cache, "work", LIMIT).await.unwrap();
    source.fetched.clear();
    report
}

fn cached_uids(cache: &Cache) -> Vec<u32> {
    let mut uids: Vec<u32> = cache
        .list(Some("work"), 100)
        .unwrap()
        .iter()
        .map(|e| e.uid.get())
        .collect();
    uids.sort_unstable();
    uids
}

fn report(new: usize, updated: usize, removed: usize, reset: bool) -> SyncReport {
    SyncReport {
        new,
        updated,
        removed,
        reset,
    }
}

#[tokio::test]
async fn first_sync_takes_the_newest_and_second_fetches_nothing() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);

    let first = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(first, report(3, 0, 0, false));
    assert_eq!(source.fetched, vec![3, 4, 5]);
    assert_eq!(cached_uids(&cache), vec![3, 4, 5]);
    source.fetched.clear();

    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
    assert!(source.fetched.is_empty());
}

#[tokio::test]
async fn new_mail_is_fetched_alone() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    sync(&mut source, &mut cache).await;

    source.messages.insert(9, Flags::default());
    let r = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(1, 0, 0, false));
    assert_eq!(source.fetched, vec![9]);
    assert_eq!(cached_uids(&cache), vec![3, 4, 5, 9]);
}

#[tokio::test]
async fn flag_change_is_refreshed() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    sync(&mut source, &mut cache).await;
    let unread = |cache: &Cache| {
        let list = cache.list(Some("work"), 10).unwrap();
        list.iter().filter(|e| !e.flags.is_seen()).count()
    };
    assert_eq!(unread(&cache), 3);

    source.messages.insert(4, Flags::new([Flags::SEEN]));
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 1, 0, false));
    assert_eq!(unread(&cache), 2);
}

#[tokio::test]
async fn expunged_messages_are_dropped() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    sync(&mut source, &mut cache).await;

    source.messages.remove(&3);
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 1, false));
    assert_eq!(cached_uids(&cache), vec![4, 5]);

    // Everything expunged, then one new message: only it is fetched, not old mail.
    source.messages.retain(|_, _| false);
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 2, false));
    source.messages.insert(6, Flags::default());
    let r = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(1, 0, 0, false));
    assert_eq!(source.fetched, vec![6]);
}

#[tokio::test]
async fn uidvalidity_change_rebuilds_the_cache() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    sync(&mut source, &mut cache).await;

    let mut renumbered = FakeSource::new(8, 101..=104);
    let r = sync_inbox(&mut renumbered, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(3, 0, 3, true));
    assert_eq!(renumbered.fetched, vec![102, 103, 104]);
    assert_eq!(cached_uids(&cache), vec![102, 103, 104]);
}

#[tokio::test]
async fn accounts_are_kept_apart_and_empty_inbox_is_fine() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=2);
    sync(&mut source, &mut cache).await;

    let mut empty = FakeSource::new(1, []);
    let r = sync_inbox(&mut empty, &mut cache, "home", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(0, 0, 0, false));
    assert_eq!(cache.list(None, 10).unwrap().len(), 2);
    assert!(cache.list(Some("home"), 10).unwrap().is_empty());
}

#[tokio::test]
async fn more_than_limit_new_messages_are_all_fetched() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    sync(&mut source, &mut cache).await;
    assert_eq!(cached_uids(&cache), vec![3, 4, 5]);

    // Four new messages with a limit of three: the limit is for the first sync only.
    source
        .messages
        .extend((6..=9).map(|u| (u, Flags::default())));
    let r = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(4, 0, 0, false));
    assert_eq!(source.fetched, vec![6, 7, 8, 9]);
    source.fetched.clear();

    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
    assert_eq!(cached_uids(&cache), vec![3, 4, 5, 6, 7, 8, 9]);
}

#[tokio::test]
async fn first_sync_picks_by_arrival_when_uids_run_backwards() {
    // Like Proton Bridge: UID 1 arrived last, UID 5 first.
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    for uid in 1..=5 {
        let at = DateTime::from_timestamp(1_000_000 - i64::from(uid) * 60, 0).unwrap();
        source.dates.insert(uid, at);
    }

    let first = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(first, report(3, 0, 0, false));
    assert_eq!(source.fetched, vec![1, 2, 3]);
    source.fetched.clear();

    // The older, higher UIDs 4 and 5 are not new; only mail that arrives later is.
    assert_eq!(sync(&mut source, &mut cache).await, report(0, 0, 0, false));
    source.messages.insert(6, Flags::default());
    let r = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(1, 0, 0, false));
    assert_eq!(source.fetched, vec![6]);
    assert_eq!(cached_uids(&cache), vec![1, 2, 3, 6]);
}

#[tokio::test]
async fn interrupted_first_sync_is_completed_by_the_next() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut source = FakeSource::new(7, 1..=5);
    // An earlier first sync stored UID 4 and stopped before setting the watermark.
    let mailbox = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(7).unwrap())
        .unwrap();
    let partial = source.fetch(&[Uid::new(4).unwrap()]).await.unwrap();
    cache.store_messages(mailbox.id, &partial).unwrap();
    source.fetched.clear();

    let r = sync_inbox(&mut source, &mut cache, "work", LIMIT)
        .await
        .unwrap();
    assert_eq!(r, report(2, 0, 0, false));
    assert_eq!(source.fetched, vec![3, 5]);
    assert_eq!(cached_uids(&cache), vec![3, 4, 5]);
}
