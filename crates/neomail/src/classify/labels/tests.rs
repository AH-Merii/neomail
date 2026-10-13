//! The category pass against a fake [`CategoryWriter`] and a cache in memory: no Graph, no
//! keyring, no lock file.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use nm_core::{Choice, Classification, Flags, IdListed, UidValidity};

use super::*;
use crate::daemon::Line;

/// One edit as the fake saw it: (account, remote id, want, last).
type Seen = (String, String, String, Option<String>);

/// What the fake answers for a message, `Set` unless told otherwise.
#[derive(Clone, Copy)]
enum Answer {
    Set,
    /// The message carried the category already: no PATCH.
    Already,
    Gone,
    /// A 500: that message fails, the others go on.
    Fails,
    /// A 403: Graph answers no edit after it, as `set_categories` stops there.
    Refused,
    /// A 401, which Graph stops at too.
    TokenRefused,
    /// Still throttled after Graph's retries.
    Throttled,
    /// Graph cannot be reached.
    Unreachable,
    /// Cut off at the deadline: no answer at all.
    CutOff,
}

#[derive(Default)]
struct Fake {
    read_only: bool,
    locked: bool,
    answers: HashMap<String, Answer>,
    /// Each write call's edits: (account, remote id, want, last).
    writes: Vec<Vec<Seen>>,
    logins: usize,
    /// Set during the first write call, as a signal would be.
    stop_on_write: Option<Arc<AtomicBool>>,
    /// A clock each write call moves on by the duration, as a slow Graph would.
    slow: Option<(Rc<Cell<Instant>>, Duration)>,
    /// The deadline each write call was given.
    deadlines: Vec<Option<Instant>>,
    /// Whether the account's lock is held now.
    held: Rc<Cell<bool>>,
    /// A warning each sign-in gives, as a token that could not be saved would.
    warn: Option<&'static str>,
    warnings: Vec<(String, String)>,
}

/// The fake's account lock: held until dropped.
struct Held(Rc<Cell<bool>>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl CategoryWriter for Fake {
    type Lock = Held;

    fn lock(&mut self, _account: &str) -> Result<Option<Held>> {
        if self.locked {
            return Ok(None);
        }
        self.held.set(true);
        Ok(Some(Held(self.held.clone())))
    }

    fn may_write(&mut self, account: &Account) -> Result<bool> {
        assert!(self.held.get(), "signed in without the account lock");
        self.logins += 1;
        if let Some(warning) = self.warn {
            self.warnings.push((account.name.clone(), warning.into()));
        }
        Ok(!self.read_only)
    }

    fn warnings(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.warnings)
    }

    fn write(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        edits: &[CategoryEdit],
        deadline: Option<Instant>,
    ) -> Result<Vec<(RemoteId, CategoryResult)>> {
        assert_eq!(prefix.as_str(), "nm-");
        assert!(self.held.get(), "wrote without the account lock");
        self.deadlines.push(deadline);
        if let Some((clock, took)) = &self.slow {
            clock.set(clock.get() + *took);
        }
        assert!(edits.len() <= 4, "at most 4 at once");
        if let Some(stop) = &self.stop_on_write {
            stop.store(true, Ordering::SeqCst);
        }
        self.writes.push(
            edits
                .iter()
                .map(|e| {
                    let id = e.id.as_str().to_owned();
                    (account.name.clone(), id, e.want.clone(), e.last.clone())
                })
                .collect(),
        );
        let mut refused = false;
        Ok(edits
            .iter()
            .filter_map(|e| {
                if refused {
                    return None;
                }
                let answer = self.answers.get(e.id.as_str()).copied();
                refused = matches!(answer, Some(Answer::Refused | Answer::TokenRefused));
                let result = match answer.unwrap_or(Answer::Set) {
                    Answer::Set => CategoryResult::Set,
                    Answer::Already => CategoryResult::Already,
                    Answer::Gone => CategoryResult::Gone,
                    Answer::Fails => CategoryResult::Failed(GraphError::Status {
                        status: 500,
                        code: None,
                    }),
                    Answer::Refused => CategoryResult::Failed(GraphError::Forbidden),
                    Answer::TokenRefused => CategoryResult::Failed(GraphError::Unauthorized),
                    Answer::Throttled => {
                        CategoryResult::Failed(GraphError::Throttled { status: 429 })
                    }
                    Answer::Unreachable => {
                        CategoryResult::Failed(GraphError::Transport("connection refused".into()))
                    }
                    Answer::CutOff => return None,
                };
                Some((e.id.clone(), result))
            })
            .collect())
    }
}

const CONFIG: &str = "\
[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n\
[[accounts]]\nname = \"second\"\nusername = \"me@outlook.example\"\n\
oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n\
[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
imap = { host = \"imap.example.com\" }\n";

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

/// Outlook message `name` of `account`, received `days` before [`now`], classified as `choice`.
fn outlook(cache: &mut Cache, account: &str, name: &str, days: i64, choice: Option<Choice>) {
    let mb = match cache.mailbox(account, "INBOX").unwrap() {
        Some(mb) => mb,
        None => cache.reset_id_mailbox(account, "INBOX").unwrap(),
    };
    let listed = IdListed {
        id: RemoteId::new(name).unwrap(),
        flags: Flags::default(),
        received: now() - TimeDelta::days(days),
    };
    let raw = format!("Message-ID: <{name}@acme.example>\r\nSubject: {name}\r\n\r\nhi\r\n");
    cache
        .store_id_messages(mb.id, &[(&listed, raw.as_bytes())], None)
        .unwrap();
    if let Some(choice) = choice {
        classify(cache, account, &format!("<{name}@acme.example>"), choice);
    }
}

fn classify(cache: &mut Cache, account: &str, message_id: &str, choice: Choice) {
    let key = cache
        .pending(Some(account), true)
        .unwrap()
        .into_iter()
        .filter_map(|p| p.msg_key)
        .find(|k| k.contains(message_id.trim_matches(['<', '>'])))
        .unwrap();
    let result = Classification {
        choice,
        confidence: 0.9,
        probabilities: vec![(choice, 0.9)],
        model: "jev-1.13.0".into(),
        request_version: 1,
        input_tokens: None,
        classified_at: now(),
    };
    cache.store_classification(account, &key, &result).unwrap();
}

struct Run {
    config: Config,
    stop: AtomicBool,
}

impl Run {
    fn new(extra: &str) -> Self {
        Self {
            config: Config::parse(&format!("{CONFIG}[classify]\n{extra}")).unwrap(),
            stop: AtomicBool::new(false),
        }
    }

    fn pass(
        &self,
        cache: &mut Cache,
        fake: &mut Fake,
        cap: usize,
        at: DateTime<Utc>,
    ) -> Vec<AccountOutcome> {
        let accounts: Vec<&Account> = self.config.accounts.iter().collect();
        let pass = Pass {
            settings: &self.config.classify,
            ids: None,
            cap,
            stop: &self.stop,
            deadline: None,
            clock: &Instant::now,
            now: at,
        };
        write_labels(cache, &accounts, &pass, fake)
    }
}

/// Every edit the fake saw, flattened: (account, id, want, last).
fn edits(fake: &Fake) -> Vec<Seen> {
    fake.writes.concat()
}

fn edit(account: &str, id: &str, want: &str, last: Option<&str>) -> Seen {
    (account.into(), id.into(), want.into(), last.map(Into::into))
}

fn counts(outcomes: &[AccountOutcome]) -> Vec<(String, usize, usize, usize)> {
    outcomes
        .iter()
        .map(|o| (o.account.clone(), o.written, o.failed, o.waiting))
        .collect()
}

#[test]
fn each_classified_outlook_message_gets_its_effective_group_once_and_again_when_it_changes() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 3, Some(Choice::Main));
    outlook(&mut cache, "outlook", "b", 2, Some(Choice::FollowUp));
    outlook(&mut cache, "outlook", "c", 1, Some(Choice::Spam));
    outlook(&mut cache, "outlook", "d", 0, None);
    // A classified IMAP message: its label is slice 10d's.
    let mb = cache
        .reset_mailbox("work", "INBOX", UidValidity::new(1).unwrap())
        .unwrap();
    let imap = nm_core::FetchedMessage {
        uid: nm_core::Uid::new(1).unwrap(),
        flags: Flags::default(),
        received: Some(now()),
        raw: b"Message-ID: <e@acme.example>\r\nSubject: e\r\n\r\nhi\r\n".to_vec(),
    };
    cache.insert_messages(mb.id, &[imap]).unwrap();
    classify(&mut cache, "work", "<e@acme.example>", Choice::Main);

    let run = Run::new("follow_up_old_days = 30\n");
    let mut fake = Fake::default();
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(
        edits(&fake),
        [
            edit("outlook", "c", "nm-spam", None),
            edit("outlook", "b", "nm-follow-up", None),
            edit("outlook", "a", "nm-main", None),
        ],
        "newest first, Outlook only, classified only"
    );
    assert_eq!(counts(&outcomes), [("outlook".into(), 3, 0, 0)]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 3 categories written")
    );

    // Nothing is due: no sign-in, no request, nothing to say.
    let mut again = Fake::default();
    assert!(run.pass(&mut cache, &mut again, PER_RUN, now()).is_empty());
    assert_eq!((again.logins, again.writes.len()), (0, 0));

    // A month on, the follow-up is old: its category changes, with no new mail.
    let later = now() + TimeDelta::days(29);
    let outcomes = run.pass(&mut cache, &mut again, PER_RUN, later);
    assert_eq!(
        edits(&again),
        [edit(
            "outlook",
            "b",
            "nm-follow-up-old",
            Some("nm-follow-up")
        )]
    );
    assert_eq!(
        summary(&outcomes[0], "wake").as_deref(),
        Some("outlook: 1 category written")
    );
}

#[test]
fn the_cap_leaves_the_rest_for_the_next_pass() {
    let mut cache = Cache::open_in_memory().unwrap();
    for n in 0..6 {
        outlook(
            &mut cache,
            "outlook",
            &format!("m{n}"),
            n,
            Some(Choice::Newsletters),
        );
    }
    let run = Run::new("");
    let mut fake = Fake::default();
    let outcomes = run.pass(&mut cache, &mut fake, 5, now());
    assert_eq!(counts(&outcomes), [("outlook".into(), 5, 0, 1)]);
    assert_eq!(fake.writes.iter().map(Vec::len).collect::<Vec<_>>(), [4, 1]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 5 categories written, 1 waits for the next run")
    );
    let mut next = Fake::default();
    let outcomes = run.pass(&mut cache, &mut next, 5, now());
    assert_eq!(
        edits(&next),
        [edit("outlook", "m5", "nm-newsletters", None)]
    );
    assert_eq!(counts(&outcomes), [("outlook".into(), 1, 0, 0)]);
}

#[test]
fn a_failed_write_stays_due_and_fails_the_run_and_a_gone_one_does_not() {
    let mut cache = Cache::open_in_memory().unwrap();
    for name in ["a", "b", "c", "d"] {
        outlook(&mut cache, "outlook", name, 1, Some(Choice::Main));
    }
    let run = Run::new("");
    let mut fake = Fake::default();
    fake.answers.insert("a".into(), Answer::Fails);
    fake.answers.insert("b".into(), Answer::Gone);
    fake.answers.insert("d".into(), Answer::Already);
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(counts(&outcomes), [("outlook".into(), 1, 1, 0)]);
    assert!(failed(&outcomes));
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 1 category written, 1 failed (tried again next run)")
    );
    // a is tried again; b is tried again until a sync drops it; c is done, and so is d, which
    // carried its category already and is not counted as written.
    let mut next = Fake::default();
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, now());
    let ids: Vec<String> = edits(&next).into_iter().map(|e| e.1).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"a".to_owned()) && ids.contains(&"b".to_owned()));
    assert!(!failed(&outcomes));
}

#[test]
fn a_read_only_sign_in_sends_nothing_and_is_a_warning_not_a_failure() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    let run = Run::new("");
    let mut fake = Fake {
        read_only: true,
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert!(fake.writes.is_empty());
    assert!(matches!(outcomes[0].off, Some(Off::ReadOnly)));
    assert!(!failed(&outcomes));
    assert_eq!(
        off_line(&outcomes[0]).as_deref(),
        Some(
            "outlook: groups are not written to Outlook: run `neomail account login outlook` \
             to let neomail change mail"
        )
    );
    assert_eq!(summary(&outcomes[0], "run"), None);
}

#[test]
fn labels_false_writes_nothing_and_signs_in_to_nothing() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    let mut fake = Fake::default();
    let outcomes = Run::new("labels = false\n").pass(&mut cache, &mut fake, PER_RUN, now());
    assert!(outcomes.is_empty());
    assert_eq!((fake.logins, fake.writes.len()), (0, 0));
}

#[test]
fn a_locked_account_is_skipped_quietly() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    let mut fake = Fake {
        locked: true,
        ..Fake::default()
    };
    let outcomes = Run::new("").pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!((fake.logins, fake.writes.len()), (0, 0));
    assert_eq!(counts(&outcomes), [("outlook".into(), 0, 0, 1)]);
    assert_eq!(off_line(&outcomes[0]), None);
    assert!(!failed(&outcomes));
}

#[test]
fn a_refused_grant_stops_that_account_only_and_its_message_stays_due() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    outlook(&mut cache, "second", "b", 1, Some(Choice::Main));
    let run = Run::new("");
    let mut fake = Fake::default();
    fake.answers.insert("a".into(), Answer::Refused);
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(
        counts(&outcomes),
        [("outlook".into(), 0, 0, 1), ("second".into(), 1, 0, 0)]
    );
    assert!(failed(&outcomes));
    let off = off_line(&outcomes[0]).unwrap();
    assert!(
        off.starts_with("outlook: categories not written: ")
            && off.contains("neomail account login outlook"),
        "{off}"
    );
    let mut next = Fake::default();
    run.pass(&mut cache, &mut next, PER_RUN, now());
    assert_eq!(edits(&next), [edit("outlook", "a", "nm-main", None)]);
}

#[test]
fn a_stop_ends_the_pass_after_the_writes_in_flight() {
    let mut cache = Cache::open_in_memory().unwrap();
    for n in 0..10 {
        outlook(
            &mut cache,
            "outlook",
            &format!("m{n}"),
            n,
            Some(Choice::Main),
        );
    }
    let run = Run::new("");
    let stop = Arc::new(AtomicBool::new(false));
    let mut fake = Fake {
        stop_on_write: Some(stop.clone()),
        ..Fake::default()
    };
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let pass = Pass {
        settings: &run.config.classify,
        ids: None,
        cap: PER_RUN,
        stop: &stop,
        deadline: None,
        clock: &Instant::now,
        now: now(),
    };
    let outcomes = write_labels(&mut cache, &accounts, &pass, &mut fake);
    assert_eq!(fake.writes.len(), 1);
    assert_eq!(counts(&outcomes), [("outlook".into(), 4, 0, 6)]);
}

#[test]
fn the_daemon_says_why_an_account_is_off_once_per_state() {
    let off = |off: Option<Off>, written: usize| AccountOutcome {
        account: "outlook".into(),
        written,
        failed: 0,
        waiting: 0,
        off,
    };
    let mut reports = Reports::default();
    let read_only = "neomail: outlook: groups are not written to Outlook: run `neomail account \
                     login outlook` to let neomail change mail";
    assert_eq!(
        reports.lines(&[off(Some(Off::ReadOnly), 0)]),
        [Line::Error(read_only.into())]
    );
    assert_eq!(reports.lines(&[off(Some(Off::ReadOnly), 0)]), []);
    let failed = || Some(Off::Failed(anyhow::anyhow!("access token refused")));
    let refused = "neomail: outlook: categories not written: access token refused";
    assert_eq!(
        reports.lines(&[off(failed(), 0)]),
        [Line::Error(refused.into())]
    );
    assert_eq!(reports.lines(&[off(failed(), 0)]), []);
    assert_eq!(
        reports.lines(&[off(None, 2)]),
        [Line::Report("outlook: 2 categories written".into())]
    );
    assert_eq!(
        reports.lines(&[off(Some(Off::ReadOnly), 0)]),
        [Line::Error(read_only.into())],
        "said again after it changed"
    );
}

/// Ten due Outlook messages, m0 the newest, and one on the second account.
fn ten_and_one(cache: &mut Cache) {
    for n in 0..10 {
        let name = format!("m{n}");
        outlook(cache, "outlook", &name, n, Some(Choice::Main));
    }
    outlook(cache, "second", "s", 1, Some(Choice::Main));
}

#[test]
fn a_throttled_or_unreachable_graph_ends_that_accounts_pass_and_the_rest_wait() {
    for (answer, says) in [
        (Answer::Throttled, "throttling requests (HTTP 429)"),
        (Answer::Unreachable, "cannot reach Microsoft Graph"),
    ] {
        let mut cache = Cache::open_in_memory().unwrap();
        ten_and_one(&mut cache);
        let run = Run::new("");
        let mut fake = Fake::default();
        fake.answers.insert("m1".into(), answer);
        let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
        let outlook_writes = fake.writes.iter().filter(|w| w[0].0 == "outlook").count();
        assert_eq!(outlook_writes, 1, "no chunk after the first: {says}");
        assert_eq!(
            counts(&outcomes),
            [("outlook".into(), 3, 0, 7), ("second".into(), 1, 0, 0)],
            "not counted as failed writes: {says}"
        );
        let off = off_line(&outcomes[0]).unwrap();
        assert!(
            off.starts_with("outlook: categories not written: "),
            "{off}"
        );
        assert!(off.contains(says), "{off}");
        assert!(failed(&outcomes));
        assert_eq!(
            summary(&outcomes[0], "wake").as_deref(),
            Some("outlook: 3 categories written, 7 wait for the next wake")
        );

        let mut next = Fake::default();
        run.pass(&mut cache, &mut next, PER_RUN, now());
        let mut ids: Vec<String> = edits(&next).into_iter().map(|e| e.1).collect();
        ids.sort();
        assert_eq!(ids, ["m1", "m4", "m5", "m6", "m7", "m8", "m9"], "{says}");
    }
}

#[test]
fn the_deadline_ends_the_pass_between_writes_and_what_is_cut_off_waits() {
    let mut cache = Cache::open_in_memory().unwrap();
    ten_and_one(&mut cache);
    let run = Run::new("");
    let start = Instant::now();
    let clock = Rc::new(Cell::new(start));
    let deadline = start + Duration::from_secs(60);
    let mut fake = Fake {
        slow: Some((clock.clone(), Duration::from_secs(90))),
        ..Fake::default()
    };
    // The writer drops it at the deadline: it gets no answer.
    fake.answers.insert("m2".into(), Answer::CutOff);
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let read = || clock.get();
    let pass = Pass {
        settings: &run.config.classify,
        ids: None,
        cap: PER_RUN,
        stop: &run.stop,
        deadline: Some(deadline),
        clock: &read,
        now: now(),
    };
    let outcomes = write_labels(&mut cache, &accounts, &pass, &mut fake);
    assert_eq!(fake.writes.len(), 1, "no write starts after the deadline");
    assert_eq!(
        fake.deadlines,
        [Some(deadline)],
        "the writer cuts off at it"
    );
    assert_eq!(fake.logins, 1, "no sign-in after the deadline either");
    assert_eq!(
        counts(&outcomes),
        [("outlook".into(), 3, 0, 7), ("second".into(), 0, 0, 1)]
    );
    assert!(outcomes.iter().all(|o| o.off.is_none()));
    assert!(!failed(&outcomes));
}

#[test]
fn a_refusal_mid_chunk_leaves_the_unanswered_messages_waiting_and_asks_for_no_command() {
    let mut cache = Cache::open_in_memory().unwrap();
    for n in 0..6 {
        outlook(
            &mut cache,
            "outlook",
            &format!("m{n}"),
            n,
            Some(Choice::Main),
        );
    }
    let run = Run::new("");
    let mut fake = Fake::default();
    // The first of the second chunk: Graph answers nothing for m5 after it.
    fake.answers.insert("m4".into(), Answer::TokenRefused);
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(counts(&outcomes), [("outlook".into(), 4, 0, 2)]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 4 categories written, 2 wait for the next run")
    );
    assert_eq!(
        off_line(&outcomes[0]).as_deref(),
        Some("outlook: categories not written: access token refused; signed in again next time")
    );
}

/// `count` due Outlook messages.
fn many(cache: &mut Cache, count: i64) {
    for n in 0..count {
        let name = format!("m{n}");
        outlook(cache, "outlook", &name, n % 300, Some(Choice::Main));
    }
}

#[test]
fn a_classify_run_writes_up_to_its_cap_with_no_deadline() {
    let mut cache = Cache::open_in_memory().unwrap();
    many(&mut cache, 1001);
    let run = Run::new("");
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let pass = Pass::run(&run.config.classify, None, &run.stop, now());
    let mut fake = Fake::default();
    let outcomes = write_labels(&mut cache, &accounts, &pass, &mut fake);
    assert_eq!(counts(&outcomes), [("outlook".into(), 1000, 0, 1)]);
    assert!(fake.deadlines.iter().all(Option::is_none));
}

#[test]
fn a_daemon_wake_writes_up_to_its_cap_within_its_budget() {
    let mut cache = Cache::open_in_memory().unwrap();
    many(&mut cache, 101);
    let run = Run::new("");
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let budget = Duration::from_secs(60);
    let before = Instant::now();
    let pass = Pass::wake(&run.config.classify, &run.stop, budget, now());
    let after = Instant::now();
    let mut fake = Fake::default();
    let mut reports = Reports::default();
    let lines = daemon_wake(&mut cache, &accounts, &pass, &mut fake, &mut reports);
    assert_eq!(
        lines,
        [Line::Report(
            "outlook: 100 categories written, 1 waits for the next wake".into()
        )]
    );
    assert!(!fake.deadlines.is_empty());
    assert!(
        fake.deadlines
            .iter()
            .all(|d| d.is_some_and(|d| d >= before + budget && d <= after + budget)),
        "every write is cut off at the budget"
    );
}

#[test]
fn the_daemon_prints_a_sign_in_warning_once_per_account_state() {
    let mut cache = Cache::open_in_memory().unwrap();
    let run = Run::new("");
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let mut fake = Fake {
        warn: Some("the new refresh token could not be saved"),
        ..Fake::default()
    };
    let mut reports = Reports::default();
    let warning = "neomail: outlook: the new refresh token could not be saved";
    let mut wake = |cache: &mut Cache, fake: &mut Fake| {
        let pass = Pass::wake(
            &run.config.classify,
            &run.stop,
            Duration::from_secs(60),
            now(),
        );
        daemon_wake(cache, &accounts, &pass, fake, &mut reports)
    };
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    assert_eq!(
        wake(&mut cache, &mut fake),
        [
            Line::Error(warning.into()),
            Line::Report("outlook: 1 category written".into())
        ]
    );
    outlook(&mut cache, "outlook", "b", 1, Some(Choice::Main));
    assert_eq!(
        wake(&mut cache, &mut fake),
        [Line::Report("outlook: 1 category written".into())]
    );
}

#[test]
fn the_daemon_says_the_same_counts_once_and_a_locked_wake_changes_nothing() {
    let outcome = |off: Option<Off>, written: usize, failed: usize| AccountOutcome {
        account: "outlook".into(),
        written,
        failed,
        waiting: 0,
        off,
    };
    let mut reports = Reports::default();
    let failing = "outlook: 0 categories written, 1 failed (tried again next wake)";
    assert_eq!(
        reports.lines(&[outcome(None, 0, 1)]),
        [Line::Report(failing.into())]
    );
    assert_eq!(
        reports.lines(&[outcome(None, 0, 1)]),
        [],
        "the same failure again"
    );
    let untried = AccountOutcome {
        waiting: 5,
        ..outcome(None, 0, 0)
    };
    assert_eq!(
        reports.lines(&[untried]),
        [],
        "a stop or a spent budget before any write"
    );
    assert_eq!(
        reports.lines(&[outcome(None, 1, 1)]),
        [Line::Report(
            "outlook: 1 category written, 1 failed (tried again next wake)".into()
        )],
        "a write is news"
    );

    let read_only = reports.lines(&[outcome(Some(Off::ReadOnly), 0, 0)]);
    assert_eq!(read_only.len(), 1);
    assert_eq!(reports.lines(&[outcome(Some(Off::Locked), 0, 0)]), []);
    assert_eq!(
        reports.lines(&[outcome(Some(Off::ReadOnly), 0, 0)]),
        [],
        "nothing was said in between"
    );
}
