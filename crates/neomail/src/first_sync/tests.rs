//! The first-sync question against a fake IMAP INBOX synced into a cache in memory, scripted
//! answers in place of the terminal, and a classify run that records its calls in place of
//! `neomail classify --account NAME`, the one way this path reaches jev. The answers file lives
//! in a temporary directory.

#![allow(clippy::unwrap_used, clippy::expect_used)] // Test helpers outside #[test] functions.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::num::NonZeroU32;
use std::process::ExitCode;

use chrono::{DateTime, TimeDelta, Utc};
use nm_core::{
    Cache, FetchedMessage, Flags, ListedMessage, MailSource, MailboxStatus, SyncReport, Uid,
    UidFlags, UidValidity,
};

use super::{Offered, Setup, can_ask, firsts, is_first, offer};
use crate::classify;
use crate::config::ClassifySettings;
use crate::secret::Input;

/// An INBOX of messages received the given number of days ago, keyed by UID.
struct FakeInbox {
    validity: u32,
    ages: Vec<(u32, i64)>,
    now: DateTime<Utc>,
}

impl FakeInbox {
    fn received(&self, days: i64) -> Option<DateTime<Utc>> {
        Some(self.now - TimeDelta::days(days))
    }
}

impl MailSource for FakeInbox {
    type Error = Infallible;

    async fn examine_inbox(&mut self) -> Result<MailboxStatus, Infallible> {
        Ok(MailboxStatus {
            uid_validity: UidValidity::new(self.validity).unwrap(),
            exists: u32::try_from(self.ages.len()).unwrap(),
        })
    }

    async fn list_all(&mut self) -> Result<Vec<ListedMessage>, Infallible> {
        Ok(self
            .ages
            .iter()
            .map(|&(uid, days)| ListedMessage {
                uid: Uid::new(uid).unwrap(),
                flags: Flags::default(),
                received: self.received(days),
            })
            .collect())
    }

    async fn flags_from(&mut self, from: Uid) -> Result<Vec<UidFlags>, Infallible> {
        Ok(self
            .ages
            .iter()
            .filter(|&&(uid, _)| uid >= from.get())
            .map(|&(uid, _)| UidFlags {
                uid: Uid::new(uid).unwrap(),
                flags: Flags::default(),
            })
            .collect())
    }

    async fn fetch(&mut self, uids: &[Uid]) -> Result<Vec<FetchedMessage>, Infallible> {
        Ok(self
            .ages
            .iter()
            .filter(|(uid, _)| uids.iter().any(|u| u.get() == *uid))
            .map(|&(uid, days)| FetchedMessage {
                uid: Uid::new(uid).unwrap(),
                flags: Flags::default(),
                received: self.received(days),
                raw: format!(
                    "From: shop{uid}@mail.example\r\nMessage-ID: <{uid}@mail.example>\r\n\
                     Subject: Order {uid}\r\n\r\nYour order {uid} has shipped.\r\n"
                )
                .into_bytes(),
            })
            .collect())
    }
}

fn sync(inbox: &mut FakeInbox, cache: &mut Cache, account: &str) -> SyncReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let limit = NonZeroU32::new(500).unwrap();
    runtime
        .block_on(nm_core::sync_inbox(inbox, cache, account, limit))
        .unwrap()
}

/// `work`'s INBOX, synced once: two messages from the last year, one from two years ago.
fn synced_work() -> (Cache, SyncReport) {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut inbox = FakeInbox {
        validity: 7,
        ages: vec![(1, 2), (2, 40), (3, 730)],
        now: Utc::now(),
    };
    let report = sync(&mut inbox, &mut cache, "work");
    (cache, report)
}

/// The real preview of `work`'s first sync, as `neomail sync` shows it.
fn ready(cache: &mut Cache) -> Setup {
    let settings = ClassifySettings::default();
    let args = super::classify_args("work").unwrap();
    let preview = classify::preview(cache, &args, &settings, &[], Utc::now()).unwrap();
    Setup::Ready {
        host: settings.host().unwrap(),
        rules: settings.rules,
        labels: settings.labels,
        preview,
    }
}

/// What one offer did: its result, what was asked, the lines on standard error, and the
/// prompts the classify run was given to ask through.
struct Run {
    offered: Offered,
    asked: Vec<String>,
    err: String,
    classify_runs: usize,
}

/// Offers `account`'s question with `answers` scripted in order (`None`: standard input
/// closed). A classify run asks its consent question through the `ask` it is handed and then
/// succeeds.
fn run(
    data: &std::path::Path,
    can_ask: bool,
    setup: &mut dyn FnMut() -> anyhow::Result<Setup>,
    answers: &[Option<&str>],
) -> Run {
    let mut answers: VecDeque<Option<String>> =
        answers.iter().map(|a| a.map(str::to_owned)).collect();
    let mut asked = Vec::new();
    let mut ask = |text: &str, prompt: &str| {
        asked.push(format!("{text}{prompt}"));
        match answers.pop_front().expect("an answer for each question") {
            Some(answer) => Ok(answer),
            None => anyhow::bail!("standard input closed"),
        }
    };
    let mut classify_runs = 0;
    let mut classify = |ask: &mut dyn FnMut(&str, &str) -> anyhow::Result<String>| {
        classify_runs += 1;
        ask("consent text\n", "Send to api.typesafe.ai? [y/N] ")?;
        Ok(ExitCode::SUCCESS)
    };
    let mut err = Vec::new();
    let offered = offer(
        "work",
        data,
        can_ask,
        setup,
        &mut ask,
        &mut classify,
        &mut err,
    )
    .unwrap();
    Run {
        offered,
        asked,
        err: String::from_utf8(err).unwrap(),
        classify_runs,
    }
}

fn never() -> anyhow::Result<Setup> {
    panic!("classifying's setup is read only when the question can be asked")
}

#[test]
fn only_an_accounts_first_sync_is_offered_not_a_later_one_or_a_rebuild() {
    let (mut cache, work) = synced_work();
    let mut inbox = FakeInbox {
        validity: 7,
        ages: vec![(1, 2), (2, 40)],
        now: Utc::now(),
    };
    sync(&mut inbox, &mut cache, "home");
    inbox.ages.push((3, 0));
    let home_later = sync(&mut inbox, &mut cache, "home");
    assert_eq!(home_later.new, 1);
    sync(&mut inbox, &mut cache, "old");
    // A new UIDVALIDITY rebuilds the mailbox: its mail was the account's before.
    inbox.validity = 8;
    let old_rebuilt = sync(&mut inbox, &mut cache, "old");
    assert!(old_rebuilt.reset);
    let synced = [
        ("work".to_owned(), work),
        ("home".to_owned(), home_later),
        ("old".to_owned(), old_rebuilt),
    ];
    assert_eq!(firsts(&synced), ["work"]);
}

#[test]
fn the_question_is_asked_only_with_both_ends_on_a_terminal_and_without_no_input() {
    let input = |terminal, no_input| Input { terminal, no_input };
    assert!(can_ask(input(true, false), true));
    assert!(!can_ask(input(true, true), true), "--no-input");
    assert!(
        !can_ask(input(false, false), true),
        "standard input not a terminal"
    );
    assert!(
        !can_ask(input(true, false), false),
        "standard error not a terminal"
    );
}

#[test]
fn the_question_shows_the_years_estimate_the_rest_and_the_group_rules() {
    let (mut cache, _) = synced_work();
    let dir = tempfile::tempdir().unwrap();
    let r = run(
        dir.path(),
        true,
        &mut || Ok(ready(&mut cache)),
        &[Some("n")],
    );
    assert_eq!(r.asked.len(), 1, "{:?}", r.asked);
    let q = &r.asked[0];
    for part in [
        "work's first sync is done",
        "jev at api.typesafe.ai",
        "the last 365 days: 2 messages, 2 requests, about ",
        "per million input tokens",
        "past the 1,000-message cap): 1 message, 1 request, about ",
        "\n    `neomail classify --all --account work` sends it later\n",
        "Spam is grouped as spam",
        "confidence of 0.4 lists as other",
        "a follow-up older than 90 days as follow-up-old",
        "Classifying also writes each message's group to its server as a label or category\n\
         and moves spam at a confidence of 0.9 or more to Junk.\n",
        "Nothing is sent without a yes",
        "classify-first-sync.json\n",
        "and not asked again for work.\n",
    ] {
        assert!(q.contains(part), "{part:?} missing from:\n{q}");
    }
    assert!(q.ends_with("Classify work's last year now? [y/N] "), "{q}");
}

#[test]
fn without_a_yes_nothing_is_classified_and_the_question_never_comes_back() {
    for answer in [Some("n"), Some(""), Some("later"), None] {
        let (mut cache, _) = synced_work();
        let dir = tempfile::tempdir().unwrap();
        let r = run(dir.path(), true, &mut || Ok(ready(&mut cache)), &[answer]);
        assert_eq!(r.offered, Offered::Declined, "{answer:?}");
        assert_eq!(r.classify_runs, 0, "{answer:?}");
        assert!(
            r.err
                .contains("nothing was classified; run `neomail classify --account work`"),
            "{answer:?}: {}",
            r.err
        );
        // Kept privately, as the consent file is.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(dir.path().join(super::ANSWERS_FILE)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{answer:?}");
        }
        // The next first sync of this account (a deleted cache) asks nothing and says nothing.
        let again = run(dir.path(), true, &mut never, &[]);
        assert_eq!((again.offered, again.err.as_str()), (Offered::Quiet, ""));
        assert!(again.asked.is_empty());
    }
}

#[test]
fn a_yes_runs_classify_once_whose_consent_question_follows_on_the_same_terminal() {
    for yes in ["y", "YES"] {
        let (mut cache, _) = synced_work();
        let dir = tempfile::tempdir().unwrap();
        let mut setup = || Ok(ready(&mut cache));
        let r = run(dir.path(), true, &mut setup, &[Some(yes), Some("y")]);
        assert_eq!(r.offered, Offered::Classified(ExitCode::SUCCESS));
        assert_eq!(r.classify_runs, 1);
        assert_eq!(r.asked.len(), 2, "{:?}", r.asked);
        assert!(r.asked[1].ends_with("Send to api.typesafe.ai? [y/N] "));
        let again = run(dir.path(), true, &mut never, &[]);
        assert_eq!((again.offered, again.classify_runs), (Offered::Quiet, 0));
    }
}

#[test]
fn each_account_is_asked_for_itself() {
    let (mut cache, _) = synced_work();
    let dir = tempfile::tempdir().unwrap();
    super::remember(dir.path(), "home", false).unwrap();
    let r = run(
        dir.path(),
        true,
        &mut || Ok(ready(&mut cache)),
        &[Some("n")],
    );
    assert_eq!(r.offered, Offered::Declined);
    assert_eq!(super::answered(dir.path(), "home").unwrap(), Some(false));
    assert_eq!(super::answered(dir.path(), "work").unwrap(), Some(false));
}

#[test]
fn off_a_terminal_one_line_says_how_to_classify_and_nothing_is_asked_or_kept() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), false, &mut never, &[]);
    assert_eq!(r.offered, Offered::Hinted);
    assert!(r.asked.is_empty());
    assert_eq!(r.classify_runs, 0);
    assert_eq!(r.err.lines().count(), 1, "{}", r.err);
    assert!(
        r.err.contains("`neomail classify --account work`"),
        "{}",
        r.err
    );
    assert_eq!(super::answered(dir.path(), "work").unwrap(), None);
}

#[test]
fn without_a_key_one_line_says_how_to_set_it_up_and_nothing_is_asked_or_kept() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), true, &mut || Ok(Setup::NoKey), &[]);
    assert_eq!(r.offered, Offered::Hinted);
    assert!(r.asked.is_empty());
    assert_eq!(r.classify_runs, 0);
    assert_eq!(r.err.lines().count(), 1, "{}", r.err);
    assert!(r.err.contains("`neomail key set`"), "{}", r.err);
    assert_eq!(super::answered(dir.path(), "work").unwrap(), None);
}

#[test]
fn nothing_from_the_last_year_is_not_worth_a_question() {
    let mut cache = Cache::open_in_memory().unwrap();
    let mut inbox = FakeInbox {
        validity: 7,
        ages: vec![(1, 400), (2, 900)],
        now: Utc::now(),
    };
    assert!(is_first(&sync(&mut inbox, &mut cache, "work")));
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), true, &mut || Ok(ready(&mut cache)), &[]);
    assert_eq!((r.offered, r.err.as_str()), (Offered::Quiet, ""));
    assert!(r.asked.is_empty());
}
