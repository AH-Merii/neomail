//! `action::apply` against a real cache (in memory, or in a file when a second connection reads
//! it) and a fake writer that records what reached "the server".
#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::DateTime;
use nm_core::action::{Failure, Report, apply};
use nm_core::{
    Action, Cache, FetchedMessage, Flags, IdListed, MailWriter, MessageId, RemoteId, Uid,
    UidValidity, Written,
};

const VALIDITY: u32 = 7;

fn uid(n: u32) -> Uid {
    Uid::new(n).unwrap()
}

fn imap_message(n: u32, flags: &[&str]) -> FetchedMessage {
    FetchedMessage {
        uid: uid(n),
        flags: Flags::new(flags.iter().copied()),
        received: DateTime::from_timestamp(i64::from(n) * 100, 0),
        raw: format!("Subject: {n}\r\n\r\nBody\r\n").into_bytes(),
    }
}

/// An in-memory cache with [`seed`]'s messages, and their ids by account.
fn seeded() -> (Cache, Vec<MessageId>, MessageId) {
    let mut cache = Cache::open_in_memory().unwrap();
    let (work, outlook) = seed(&mut cache);
    (cache, work, outlook)
}

/// Caches "work" (IMAP) with UIDs 1 to 3 unread except 3, which is read and flagged, and
/// "outlook" (by id) with one unread, flagged message "AAA". Returns the ids by account.
fn seed(cache: &mut Cache) -> (Vec<MessageId>, MessageId) {
    let work = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(VALIDITY).unwrap())
        .unwrap();
    cache
        .insert_messages(
            work.id,
            &[
                imap_message(1, &[]),
                imap_message(2, &[]),
                imap_message(3, &["\\Seen", "\\Flagged"]),
            ],
        )
        .unwrap();
    let outlook = cache.reset_id_mailbox("outlook", "INBOX").unwrap();
    let listed = IdListed {
        id: RemoteId::new("AAA").unwrap(),
        flags: Flags::new(["\\Flagged"]),
        received: DateTime::from_timestamp(50, 0).unwrap(),
    };
    let ids = cache
        .store_id_messages(outlook.id, &[(&listed, b"Subject: o\r\n\r\nx\r\n")], None)
        .unwrap();
    let work_ids = cache
        .list(Some("work"), 10)
        .unwrap()
        .into_iter()
        .map(|e| (e.uid.unwrap(), e.id))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    (work_ids, ids[0])
}

/// What the fake server answers for a UID write.
type UidAnswer = Box<dyn Fn(&[Uid], bool) -> Result<Written<Uid>, String>>;

/// What a lock reports about its account as it is released.
type AtUnlock = Rc<dyn Fn(&str) -> String>;

/// Records every call in order. `uid_answer` decides the IMAP side; by-id writes confirm every
/// id without flags; writes to the accounts in `fail_write` fail.
struct Fake {
    log: Rc<RefCell<Vec<String>>>,
    uid_answer: UidAnswer,
    fail_write: Vec<&'static str>,
    at_unlock: Option<AtUnlock>,
}

impl Fake {
    fn new() -> Self {
        Self {
            log: Rc::default(),
            uid_answer: Box::new(|uids, _| {
                Ok(Written {
                    done: uids.iter().map(|&u| (u, None)).collect(),
                    gone: Vec::new(),
                })
            }),
            fail_write: Vec::new(),
            at_unlock: None,
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

/// Logs its release, with what `at_unlock` reports then, so the tests see what happened while
/// it was held.
struct Lock {
    log: Rc<RefCell<Vec<String>>>,
    account: String,
    at_unlock: Option<AtUnlock>,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let line = match &self.at_unlock {
            Some(report) => format!("unlock {}: {}", self.account, report(&self.account)),
            None => format!("unlock {}", self.account),
        };
        self.log.borrow_mut().push(line);
    }
}

impl MailWriter for Fake {
    type Lock = Lock;
    type Error = String;

    fn lock(&mut self, account: &str) -> Result<Lock, String> {
        self.log.borrow_mut().push(format!("lock {account}"));
        Ok(Lock {
            log: self.log.clone(),
            account: account.to_owned(),
            at_unlock: self.at_unlock.clone(),
        })
    }

    fn set_seen_by_uid(
        &mut self,
        account: &str,
        uid_validity: UidValidity,
        uids: &[Uid],
        seen: bool,
    ) -> Result<Written<Uid>, String> {
        let list: Vec<String> = uids.iter().map(Uid::to_string).collect();
        self.log.borrow_mut().push(format!(
            "uid {account} v{uid_validity} {} seen={seen}",
            list.join(",")
        ));
        if self.fail_write.contains(&account) {
            return Err(format!("{account} cannot be reached"));
        }
        (self.uid_answer)(uids, seen)
    }

    fn set_seen_by_id(
        &mut self,
        account: &str,
        ids: &[RemoteId],
        seen: bool,
    ) -> Result<Written<RemoteId>, String> {
        let list: Vec<&str> = ids.iter().map(RemoteId::as_str).collect();
        self.log
            .borrow_mut()
            .push(format!("id {account} {} seen={seen}", list.join(",")));
        if self.fail_write.contains(&account) {
            return Err(format!("{account} cannot be reached"));
        }
        Ok(Written {
            done: ids.iter().map(|id| (id.clone(), None)).collect(),
            gone: Vec::new(),
        })
    }
}

fn flags_of(cache: &Cache, id: MessageId) -> Vec<String> {
    cache
        .get(id)
        .unwrap()
        .unwrap()
        .flags
        .iter()
        .map(str::to_owned)
        .collect()
}

fn done_ids(report: &Report<String>) -> Vec<MessageId> {
    report.done.iter().map(|d| d.id).collect()
}

#[test]
fn the_actions_have_stable_names() {
    let names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
    assert_eq!(names, ["mark-read", "mark-unread"]);
}

#[test]
fn each_account_is_written_under_its_lock_with_its_own_kind_of_id() {
    let (mut cache, work, outlook) = seeded();
    let mut fake = Fake::new();
    let report = apply(
        Action::MarkRead,
        &[outlook, work[1], work[0]],
        &mut cache,
        &mut fake,
    )
    .unwrap();
    assert_eq!(
        fake.log(),
        [
            "lock outlook",
            "id outlook AAA seen=true",
            "unlock outlook",
            "lock work",
            "uid work v7 2,1 seen=true",
            "unlock work",
        ]
    );
    // In the order asked, the cache updated, other flags kept.
    assert_eq!(done_ids(&report), [outlook, work[1], work[0]]);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(flags_of(&cache, outlook), ["\\Flagged", "\\Seen"]);
    assert_eq!(flags_of(&cache, work[0]), ["\\Seen"]);
}

/// A sync must not run between the write and the cache update: a second connection to the cache
/// file, reading as each account's lock is released, already sees the confirmed flags.
#[test]
fn each_account_is_cached_before_its_lock_is_released() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let mut cache = Cache::open(&path).unwrap();
    let (work, outlook) = seed(&mut cache);
    let first = work[0];
    let mut fake = Fake::new();
    fake.at_unlock = Some(Rc::new(move |account: &str| {
        let id = if account == "work" { first } else { outlook };
        flags_of(&Cache::open(&path).unwrap(), id).join(" ")
    }));
    apply(Action::MarkRead, &[outlook, first], &mut cache, &mut fake).unwrap();
    assert_eq!(
        fake.log(),
        [
            "lock outlook",
            "id outlook AAA seen=true",
            "unlock outlook: \\Flagged \\Seen",
            "lock work",
            "uid work v7 1 seen=true",
            "unlock work: \\Seen",
        ]
    );
}

#[test]
fn one_failing_account_keeps_the_others_and_changes_nothing_of_its_own() {
    let (mut cache, work, outlook) = seeded();
    let mut fake = Fake::new();
    fake.fail_write = vec!["work"];
    let report = apply(
        Action::MarkRead,
        &[work[0], outlook, work[1]],
        &mut cache,
        &mut fake,
    )
    .unwrap();
    assert_eq!(done_ids(&report), [outlook]);
    match report.failures.as_slice() {
        [
            Failure::Account {
                account,
                ids,
                error,
            },
        ] => {
            assert_eq!(account, "work");
            assert_eq!(ids, &[work[0], work[1]]);
            assert_eq!(error, "work cannot be reached");
        }
        other => panic!("{other:?}"),
    }
    assert!(flags_of(&cache, work[0]).is_empty());
    assert_eq!(flags_of(&cache, outlook), ["\\Flagged", "\\Seen"]);
}

#[test]
fn an_unknown_id_fails_alone_and_never_reaches_a_server() {
    let (mut cache, work, _) = seeded();
    let mut fake = Fake::new();
    let unknown = MessageId::new(999);
    let report = apply(Action::MarkRead, &[unknown, work[0]], &mut cache, &mut fake).unwrap();
    assert_eq!(done_ids(&report), [work[0]]);
    assert!(
        matches!(report.failures.as_slice(), [Failure::Unknown(id)] if *id == unknown),
        "{:?}",
        report.failures
    );
    assert_eq!(
        fake.log(),
        ["lock work", "uid work v7 1 seen=true", "unlock work"]
    );
}

#[test]
fn the_cache_changes_only_for_what_the_server_confirmed() {
    let (mut cache, work, _) = seeded();
    let mut fake = Fake::new();
    // UID 1 confirmed with the server's own flags, 2 gone, 3 not mentioned.
    fake.uid_answer = Box::new(|_, _| {
        Ok(Written {
            done: vec![(uid(1), Some(Flags::new(["\\Answered"])))],
            gone: vec![uid(2)],
        })
    });
    let report = apply(Action::MarkUnread, &work, &mut cache, &mut fake).unwrap();
    assert_eq!(done_ids(&report), [work[0]]);
    assert_eq!(
        flags_of(&cache, work[0]),
        ["\\Answered"],
        "the server's flags"
    );
    assert!(flags_of(&cache, work[1]).is_empty());
    assert_eq!(flags_of(&cache, work[2]), ["\\Flagged", "\\Seen"]);
    match report.failures.as_slice() {
        [
            Failure::Gone { id: gone, .. },
            Failure::NotConfirmed {
                id: unconfirmed, ..
            },
        ] => {
            assert_eq!((*gone, *unconfirmed), (work[1], work[2]));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn flags_from_the_server_in_the_wrong_state_are_cached_but_not_done() {
    let (mut cache, work, _) = seeded();
    let mut fake = Fake::new();
    fake.uid_answer = Box::new(|_, _| {
        Ok(Written {
            done: vec![(uid(1), Some(Flags::new(["\\Flagged"])))],
            gone: Vec::new(),
        })
    });
    let report = apply(Action::MarkRead, &[work[0]], &mut cache, &mut fake).unwrap();
    assert!(report.done.is_empty());
    assert!(matches!(
        report.failures.as_slice(),
        [Failure::NotConfirmed { .. }]
    ));
    assert_eq!(flags_of(&cache, work[0]), ["\\Flagged"]);
}

#[test]
fn a_message_already_in_the_asked_state_is_done_and_repeats_count_once() {
    let (mut cache, work, _) = seeded();
    let mut fake = Fake::new();
    let report = apply(Action::MarkRead, &[work[2], work[2]], &mut cache, &mut fake).unwrap();
    assert_eq!(done_ids(&report), [work[2]]);
    assert_eq!(fake.log()[1], "uid work v7 3 seen=true");
    assert_eq!(flags_of(&cache, work[2]), ["\\Flagged", "\\Seen"]);
}
