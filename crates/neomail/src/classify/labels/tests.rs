//! The pass for groups on the server against a fake [`LabelWriter`] and a cache in memory: no
//! Graph, no IMAP server, no keyring, no lock file.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use nm_core::{Choice, Classification, Flags, IdListed};

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
    /// A clock each write call moves on by the duration, as a slow Graph would, up to the
    /// call's deadline, where the real writer cuts it off.
    slow: Option<(Rc<Cell<Instant>>, Duration)>,
    /// The deadline each write call was given.
    deadlines: Vec<Option<Instant>>,
    /// The deadline each sign-in was given, and whether it is cut off there.
    sign_in_deadlines: Vec<Option<Instant>>,
    sign_in_cut_off: bool,
    /// Whether the account's lock is held now.
    held: Rc<Cell<bool>>,
    /// A warning each sign-in gives, as a token that could not be saved would.
    warn: Option<&'static str>,
    warnings: Vec<(String, String)>,
    /// Each Graph Junk move's ids.
    moves: Vec<Vec<String>>,
    /// How a Graph Junk move ends for an id: moved unless told otherwise.
    move_answers: HashMap<String, Move>,
    /// What an IMAP server takes, and each probe's account.
    takes: Option<Takes>,
    probes: Vec<String>,
    /// Each IMAP batch: (account, uid, want, last, junk).
    batches: Vec<Vec<ImapSeen>>,
    /// IMAP messages no longer in INBOX, and ones whose move the server does not confirm.
    imap_gone: Vec<u32>,
    imap_stuck: Vec<u32>,
    /// An IMAP session that fails.
    imap_fails: bool,
}

/// One IMAP edit as the fake saw it: (account, uid, want, last, junk).
type ImapSeen = (String, u32, Option<String>, Option<String>, bool);

/// What the fake answers for a Graph Junk move.
#[derive(Clone, Copy)]
enum Move {
    Gone,
    /// Graph answered nothing for it.
    Unconfirmed,
}

/// Gmail as the probe sees it: labels and a Junk folder.
const GMAIL: Takes = Takes {
    labels: LabelKind::Gmail,
    no_junk: None,
    uidplus: true,
};

/// The fake's account lock: held until dropped.
struct Held(Rc<Cell<bool>>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl LabelWriter for Fake {
    type Lock = Held;

    fn lock(&mut self, _account: &str) -> Result<Option<Held>> {
        if self.locked {
            return Ok(None);
        }
        self.held.set(true);
        Ok(Some(Held(self.held.clone())))
    }

    fn may_write(&mut self, account: &Account, deadline: Option<Instant>) -> Result<Option<bool>> {
        assert!(self.held.get(), "signed in without the account lock");
        self.logins += 1;
        self.sign_in_deadlines.push(deadline);
        if self.sign_in_cut_off {
            return Ok(None);
        }
        if let Some(warning) = self.warn {
            self.warnings.push((account.name.clone(), warning.into()));
        }
        Ok(Some(!self.read_only))
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
            let end = clock.get() + *took;
            clock.set(deadline.map_or(end, |d| end.min(d)));
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

    fn move_junk(
        &mut self,
        account: &Account,
        ids: &[RemoteId],
        _deadline: Option<Instant>,
    ) -> Result<Option<Moved<RemoteId>>> {
        assert!(self.held.get(), "moved without the account lock");
        assert!(!matches!(account.server, Server::Imap(_)));
        self.moves
            .push(ids.iter().map(|id| id.as_str().to_owned()).collect());
        let mut moved = Moved {
            folder: "junkemail".into(),
            moved: Vec::new(),
            gone: Vec::new(),
        };
        for id in ids {
            match self.move_answers.get(id.as_str()) {
                None => moved.moved.push(id.clone()),
                Some(Move::Gone) => moved.gone.push(id.clone()),
                Some(Move::Unconfirmed) => {}
            }
        }
        Ok(Some(moved))
    }

    fn probe(&mut self, account: &Account, deadline: Option<Instant>) -> Result<Option<Takes>> {
        assert!(self.held.get(), "signed in without the account lock");
        self.probes.push(account.name.clone());
        self.sign_in_deadlines.push(deadline);
        if self.sign_in_cut_off {
            return Ok(None);
        }
        Ok(Some(self.takes.unwrap_or(GMAIL)))
    }

    fn write_imap(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        _uid_validity: UidValidity,
        edits: &[LabelEdit<'_>],
        deadline: Option<Instant>,
    ) -> Result<Option<LabelsWritten>> {
        assert_eq!(prefix.as_str(), "nm-");
        assert!(self.held.get(), "wrote without the account lock");
        self.deadlines.push(deadline);
        let takes = self.takes.unwrap_or(GMAIL);
        self.batches.push(
            edits
                .iter()
                .map(|e| {
                    let (want, last) = (e.want.map(Into::into), e.last.map(Into::into));
                    (account.name.clone(), e.uid.get(), want, last, e.junk)
                })
                .collect(),
        );
        if self.imap_fails {
            anyhow::bail!("IMAP UID STORE failed");
        }
        let mut out = LabelsWritten {
            takes: Some(takes),
            ..LabelsWritten::default()
        };
        for (i, e) in edits.iter().enumerate() {
            if self.imap_gone.contains(&e.uid.get()) {
                out.gone.push(i);
                continue;
            }
            let labelled = e.want.is_some() && takes.labels != LabelKind::None;
            if labelled {
                out.labelled.push((i, true));
            }
            if e.junk && takes.no_junk.is_none() && (e.want.is_none() || labelled) {
                if self.imap_stuck.contains(&e.uid.get()) {
                    out.unmoved.push(i);
                } else {
                    out.moved.push(i);
                }
            }
        }
        Ok(Some(out))
    }
}

const CONFIG: &str = "\
[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n\
[[accounts]]\nname = \"second\"\nusername = \"me@outlook.example\"\n\
oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n\
[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
imap = { host = \"imap.gmail.com\" }\n\
[[accounts]]\nname = \"plain\"\nusername = \"you@example.com\"\n\
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
    classify_at(cache, account, message_id, choice, 0.9);
}

fn classify_at(
    cache: &mut Cache,
    account: &str,
    message_id: &str,
    choice: Choice,
    confidence: f64,
) {
    let key = cache
        .pending(Some(account), true)
        .unwrap()
        .into_iter()
        .filter_map(|p| p.msg_key)
        .find(|k| k.contains(message_id.trim_matches(['<', '>'])))
        .unwrap();
    let result = Classification {
        choice,
        confidence,
        probabilities: vec![(choice, confidence)],
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
    /// Categories alone, as before spam moved: `junk = false`.
    fn new(extra: &str) -> Self {
        Self::with(&format!("junk = false\n{extra}"))
    }

    /// The `[classify]` table `extra` and nothing more.
    fn with(extra: &str) -> Self {
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

/// Each outcome's account and how many it held back after a failure.
fn held(outcomes: &[AccountOutcome]) -> Vec<(String, usize)> {
    outcomes
        .iter()
        .map(|o| (o.account.clone(), o.held))
        .collect()
}

#[test]
fn each_classified_outlook_message_gets_its_effective_group_once_and_again_when_it_changes() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 3, Some(Choice::Main));
    outlook(&mut cache, "outlook", "b", 2, Some(Choice::FollowUp));
    outlook(&mut cache, "outlook", "c", 1, Some(Choice::Spam));
    outlook(&mut cache, "outlook", "d", 0, None);

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
        "newest first, classified only"
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
fn a_failed_write_waits_before_it_is_tried_again_and_a_gone_one_is_not_asked_again() {
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
        Some("outlook: 1 category written, 1 failed (tried again later)")
    );
    // Right after, nothing is asked: a waits, b is gone until a sync drops it, c is done, and
    // so is d, which carried its category already and is not counted as written.
    // The run says a is held back, and does not fail for it.
    let mut next = Fake::default();
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, now());
    assert_eq!((next.logins, next.writes.len()), (0, 0));
    assert_eq!(held(&outcomes), [("outlook".into(), 1)]);
    assert!(!failed(&outcomes));
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 1 held back after a failure (tried again later)")
    );
    // a fails again after 15 minutes; then it waits 30.
    let later = |minutes| now() + TimeDelta::minutes(minutes);
    next.answers.insert("a".into(), Answer::Fails);
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, later(15));
    assert_eq!(counts(&outcomes), [("outlook".into(), 0, 1, 0)]);
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, later(44));
    assert_eq!(held(&outcomes), [("outlook".into(), 1)]);
    assert_eq!(
        edits(&next).len(),
        1,
        "not asked again before its wait is over"
    );
    next.answers.clear();
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, later(45));
    let ids: Vec<String> = edits(&next).into_iter().map(|e| e.1).collect();
    assert_eq!(ids, ["a", "a"], "b is never asked again");
    assert_eq!(counts(&outcomes), [("outlook".into(), 1, 0, 0)]);
    assert!(!failed(&outcomes));
}

#[test]
fn a_failed_write_is_tried_at_once_when_its_group_changes() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::FollowUp));
    let run = Run::new(
        "follow_up_old_days = 30
",
    );
    let mut fake = Fake::default();
    fake.answers.insert("a".into(), Answer::Fails);
    run.pass(&mut cache, &mut fake, PER_RUN, now());
    // The follow-up grows old a minute later: its new group is written at once.
    let old = now() + TimeDelta::days(29) + TimeDelta::minutes(1);
    let mut next = Fake::default();
    run.pass(
        &mut cache,
        &mut next,
        PER_RUN,
        now() + TimeDelta::minutes(1),
    );
    assert!(edits(&next).is_empty(), "still waiting for the same name");
    run.pass(&mut cache, &mut next, PER_RUN, old);
    assert_eq!(
        edits(&next),
        [edit("outlook", "a", "nm-follow-up-old", None)]
    );
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
        labels: true,
        written,
        off,
        ..AccountOutcome::default()
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

/// A pass over `run`'s four accounts with a deadline 60 seconds after `start`, on `clock`.
fn timed_pass(
    run: &Run,
    cache: &mut Cache,
    fake: &mut Fake,
    clock: &Rc<Cell<Instant>>,
) -> Vec<AccountOutcome> {
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let read = || clock.get();
    let pass = Pass {
        settings: &run.config.classify,
        ids: None,
        cap: PER_RUN,
        stop: &run.stop,
        deadline: Some(clock.get() + Duration::from_secs(60)),
        clock: &read,
        now: now(),
    };
    write_labels(cache, &accounts, &pass, fake)
}

#[test]
fn a_slow_account_gets_its_share_of_the_deadline_and_the_next_one_still_gets_its_turn() {
    let mut cache = Cache::open_in_memory().unwrap();
    ten_and_one(&mut cache);
    let run = Run::new("");
    let start = Instant::now();
    let clock = Rc::new(Cell::new(start));
    let mut fake = Fake {
        slow: Some((clock.clone(), Duration::from_secs(90))),
        ..Fake::default()
    };
    // The writer drops it at the deadline: it gets no answer.
    fake.answers.insert("m2".into(), Answer::CutOff);
    let outcomes = timed_pass(&run, &mut cache, &mut fake, &clock);
    let secs = |n| Some(start + Duration::from_secs(n));
    // Four accounts: the first gets a quarter of the 60 seconds, the next a third of what is
    // left, and so on.
    assert_eq!(
        fake.deadlines,
        [secs(15), secs(30)],
        "the writer cuts off at it"
    );
    assert_eq!(
        fake.sign_in_deadlines,
        [secs(15), secs(30)],
        "the sign-in counts against it too"
    );
    assert_eq!(
        counts(&outcomes),
        [("outlook".into(), 3, 0, 7), ("second".into(), 1, 0, 0)]
    );
    assert!(outcomes.iter().all(|o| o.off.is_none()));
    assert!(!failed(&outcomes));
}

#[test]
fn a_sign_in_cut_off_at_the_deadline_leaves_everything_waiting_and_is_no_failure() {
    let mut cache = Cache::open_in_memory().unwrap();
    ten_and_one(&mut cache);
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Main, 0.9)));
    let run = Run::new("");
    let clock = Rc::new(Cell::new(Instant::now()));
    let mut fake = Fake {
        sign_in_cut_off: true,
        ..Fake::default()
    };
    let outcomes = timed_pass(&run, &mut cache, &mut fake, &clock);
    assert_eq!(
        counts(&outcomes),
        [
            ("outlook".into(), 0, 0, 10),
            ("second".into(), 0, 0, 1),
            ("work".into(), 0, 0, 1)
        ]
    );
    assert!(fake.writes.is_empty() && fake.batches.is_empty());
    assert!(
        outcomes
            .iter()
            .all(|o| o.off.is_none() && off_line(o).is_none())
    );
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
    let mut fake = Fake::default();
    let mut reports = Reports::default();
    let mut sign_in = crate::daemon::Reports::default();
    let lines = daemon_wake(
        &mut cache,
        &accounts,
        &pass,
        &mut fake,
        &mut reports,
        &mut sign_in,
    );
    let after = Instant::now();
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
            .all(|d| d.is_some_and(|d| d >= before + budget / 4 && d <= after + budget / 4)),
        "every write is cut off at the first of four accounts' share of the budget"
    );
}

#[test]
fn the_daemon_prints_a_sign_in_warning_once_whether_the_sync_or_the_writes_signed_in() {
    let mut cache = Cache::open_in_memory().unwrap();
    let run = Run::new("");
    let accounts: Vec<&Account> = run.config.accounts.iter().collect();
    let mut fake = Fake {
        warn: Some("the new refresh token could not be saved"),
        ..Fake::default()
    };
    let mut reports = Reports::default();
    let mut sign_in = crate::daemon::Reports::default();
    let warning = "neomail: outlook: the new refresh token could not be saved";
    // The sync signed in first and printed it, as `neomail daemon` shares its reports.
    assert_eq!(
        sign_in.warning("second", "the new refresh token could not be saved"),
        Some(Line::Error(
            "neomail: second: the new refresh token could not be saved".into()
        ))
    );
    let mut wake = |cache: &mut Cache, fake: &mut Fake| {
        let pass = Pass::wake(
            &run.config.classify,
            &run.stop,
            Duration::from_secs(60),
            now(),
        );
        daemon_wake(cache, &accounts, &pass, fake, &mut reports, &mut sign_in)
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
    outlook(&mut cache, "second", "c", 1, Some(Choice::Main));
    assert_eq!(
        wake(&mut cache, &mut fake),
        [Line::Report("second: 1 category written".into())],
        "printed by the sync already"
    );
}

#[test]
fn the_daemon_says_the_same_counts_once_and_a_locked_wake_changes_nothing() {
    let outcome = |off: Option<Off>, written: usize, failed: usize| AccountOutcome {
        account: "outlook".into(),
        labels: true,
        written,
        failed,
        off,
        ..AccountOutcome::default()
    };
    let mut reports = Reports::default();
    let failing = "outlook: 0 categories written, 1 failed (tried again later)";
    assert_eq!(
        reports.lines(&[outcome(None, 0, 1)]),
        [Line::Report(failing.into())]
    );
    assert_eq!(
        reports.lines(&[outcome(None, 0, 1)]),
        [],
        "the same failure again"
    );
    let held_back = AccountOutcome {
        held: 1,
        ..outcome(None, 0, 0)
    };
    assert_eq!(
        reports.lines(&[held_back]),
        [],
        "a wake while the failed message waits"
    );
    assert_eq!(
        reports.lines(&[outcome(None, 0, 1)]),
        [],
        "the same failure at its retry"
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
            "outlook: 1 category written, 1 failed (tried again later)".into()
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

/// IMAP message `uid` of `account`, received `days` before [`now`], Message-ID
/// `<name@acme.example>`, classified as `choice` unless `None`.
fn imap(
    cache: &mut Cache,
    account: &str,
    uid: u32,
    name: &str,
    days: i64,
    choice: Option<(Choice, f64)>,
) {
    let mb = match cache.mailbox(account, "INBOX").unwrap() {
        Some(mb) => mb,
        None => cache
            .reset_mailbox(account, "INBOX", UidValidity::new(1).unwrap())
            .unwrap(),
    };
    let message = nm_core::FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::default(),
        received: Some(now() - TimeDelta::days(days)),
        raw: format!("Message-ID: <{name}@acme.example>\r\nSubject: {name}\r\n\r\nhi\r\n")
            .into_bytes(),
    };
    cache.insert_messages(mb.id, &[message]).unwrap();
    if let Some((choice, confidence)) = choice {
        let id = format!("<{name}@acme.example>");
        classify_at(cache, account, &id, choice, confidence);
    }
}

fn seen(uid: u32, want: Option<&str>, last: Option<&str>, junk: bool) -> ImapSeen {
    (
        "work".into(),
        uid,
        want.map(Into::into),
        last.map(Into::into),
        junk,
    )
}

/// (account, written, moved, failed, waiting) of each outcome.
fn moves(outcomes: &[AccountOutcome]) -> Vec<(String, usize, usize, usize, usize)> {
    outcomes
        .iter()
        .map(|o| (o.account.clone(), o.written, o.junked, o.failed, o.waiting))
        .collect()
}

/// The subjects the cache lists.
fn listed(cache: &Cache) -> Vec<String> {
    let mut subjects: Vec<String> = cache
        .list(None, 100)
        .unwrap()
        .into_iter()
        .filter_map(|e| e.subject)
        .collect();
    subjects.sort();
    subjects
}

#[test]
fn imap_mail_gets_its_label_and_confident_spam_moves_to_junk_once() {
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 1, "a", 3, Some((Choice::Main, 0.9)));
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Spam, 0.95)));
    imap(&mut cache, "work", 3, "c", 1, Some((Choice::Spam, 0.6)));
    imap(&mut cache, "work", 4, "d", 0, None);
    let run = Run::with("");
    let mut fake = Fake::default();
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(fake.probes, ["work"]);
    assert_eq!(
        fake.batches,
        [vec![
            seen(3, Some("nm-spam"), None, false),
            seen(2, Some("nm-spam"), None, true),
            seen(1, Some("nm-main"), None, false),
        ]],
        "newest first; only spam at or above junk_at moves"
    );
    assert_eq!(moves(&outcomes), [("work".into(), 3, 1, 0, 0)]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("work: 3 labels written, 1 moved to Junk")
    );
    assert_eq!(
        listed(&cache),
        ["a", "c", "d"],
        "a move takes it out of the cache"
    );

    // Nothing is due: no sign-in, no session.
    let mut again = Fake::default();
    assert!(run.pass(&mut cache, &mut again, PER_RUN, now()).is_empty());
    assert!(again.probes.is_empty() && again.batches.is_empty());
    // The user moves it back: listed again, its label still written, never moved again.
    imap(&mut cache, "work", 9, "b", 2, None);
    assert!(run.pass(&mut cache, &mut again, PER_RUN, now()).is_empty());
    assert!(again.batches.is_empty());
}

#[test]
fn a_server_is_asked_only_for_what_it_takes_and_says_what_it_cannot_do_once() {
    let mut cache = Cache::open_in_memory().unwrap();
    for uid in 1..=3 {
        imap(
            &mut cache,
            "work",
            uid,
            &format!("m{uid}"),
            3,
            Some((Choice::Main, 0.9)),
        );
    }
    imap(&mut cache, "work", 4, "s", 9, Some((Choice::Spam, 0.99)));
    // A server with Junk and no labels: the move is not left behind the labels it cannot take.
    let plain = Takes {
        labels: LabelKind::None,
        no_junk: None,
        uidplus: true,
    };
    let mut fake = Fake {
        takes: Some(plain),
        ..Fake::default()
    };
    let run = Run::with("");
    let outcomes = run.pass(&mut cache, &mut fake, 1, now());
    assert_eq!(fake.batches, [vec![seen(4, None, None, true)]]);
    assert_eq!(moves(&outcomes), [("work".into(), 0, 1, 0, 0)]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("work: 1 moved to Junk")
    );

    // Gmail without a Junk folder: labels only, and a warning, said once by the daemon.
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 4, "s", 9, Some((Choice::Spam, 0.99)));
    let mut fake = Fake {
        takes: Some(Takes {
            no_junk: Some(NoJunk::NoFolder),
            ..GMAIL
        }),
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(fake.batches, [vec![seen(4, Some("nm-spam"), None, false)]]);
    let note = "work: spam is not moved to Junk: the server has no Junk folder: no mailbox has \
                the \\Junk attribute (RFC 6154)";
    assert_eq!(outcomes[0].notes, [note]);
    assert!(!failed(&outcomes), "a warning, not a failure");
    let mut reports = Reports::default();
    let said = reports.lines(&outcomes);
    assert_eq!(said[0], Line::Error(format!("neomail: {note}")));
    let mut quiet = Fake {
        takes: fake.takes,
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut quiet, PER_RUN, now());
    assert_eq!(outcomes[0].notes, [note]);
    assert_eq!(reports.lines(&outcomes), [], "the same warning again");
}

#[test]
fn a_server_that_cannot_take_labels_is_signed_in_to_only_for_spam() {
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "plain", 1, "m", 3, Some((Choice::Main, 0.9)));
    let run = Run::with("");
    let mut fake = Fake::default();
    assert!(run.pass(&mut cache, &mut fake, PER_RUN, now()).is_empty());
    assert!(
        fake.probes.is_empty(),
        "no sign-in for labels it cannot take"
    );

    imap(&mut cache, "plain", 2, "s", 2, Some((Choice::Spam, 0.95)));
    let mut fake = Fake {
        imap_fails: true,
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(fake.probes, ["plain"]);
    assert_eq!(
        off_line(&outcomes[0]).as_deref(),
        Some("plain: spam not moved to Junk: IMAP UID STORE failed")
    );
    let mut fake = Fake::default();
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(
        fake.batches,
        [vec![("plain".into(), 2, None, None, true)]],
        "the move alone, no label"
    );
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("plain: 1 moved to Junk")
    );
}

#[test]
fn a_failed_session_stops_that_account_and_its_move_stays_due() {
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Spam, 0.95)));
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Main));
    let run = Run::with("");
    let mut fake = Fake {
        imap_fails: true,
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(
        moves(&outcomes),
        [("outlook".into(), 1, 0, 0, 0), ("work".into(), 0, 0, 0, 1)]
    );
    assert!(failed(&outcomes));
    assert_eq!(
        off_line(&outcomes[1]).as_deref(),
        Some("work: labels not written: IMAP UID STORE failed")
    );
    let mut next = Fake::default();
    run.pass(&mut cache, &mut next, PER_RUN, now());
    assert_eq!(next.batches, [vec![seen(2, Some("nm-spam"), None, true)]]);
}

#[test]
fn an_unconfirmed_move_is_tried_again_and_a_message_gone_from_inbox_is_never_moved() {
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Spam, 0.95)));
    imap(&mut cache, "work", 3, "c", 2, Some((Choice::Spam, 0.95)));
    let run = Run::with("");
    let mut fake = Fake {
        imap_stuck: vec![2],
        imap_gone: vec![3],
        ..Fake::default()
    };
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(moves(&outcomes), [("work".into(), 1, 0, 1, 0)]);
    assert!(failed(&outcomes));
    // b's label is written and its move waits, then is tried again. c, gone from INBOX, is
    // never asked about again until the next sync drops it.
    let mut next = Fake::default();
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, now());
    assert_eq!(held(&outcomes), [("work".into(), 1)], "b alone: c is gone");
    assert!(next.batches.is_empty());
    let later = now() + TimeDelta::minutes(15);
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, later);
    assert_eq!(next.batches, [vec![seen(2, None, Some("nm-spam"), true)]]);
    assert_eq!(moves(&outcomes), [("work".into(), 0, 1, 0, 0)]);
}

#[test]
fn outlook_spam_moves_to_junkemail_after_its_category_and_only_once() {
    let mut cache = Cache::open_in_memory().unwrap();
    outlook(&mut cache, "outlook", "a", 1, Some(Choice::Spam));
    outlook(&mut cache, "outlook", "b", 2, Some(Choice::Main));
    outlook(&mut cache, "outlook", "c", 3, Some(Choice::Spam));
    let run = Run::with("junk_at = 0.9\n");
    let mut fake = Fake::default();
    fake.move_answers.insert("a".into(), Move::Unconfirmed);
    // The user moved c away already: it is never moved, even if it comes back.
    fake.move_answers.insert("c".into(), Move::Gone);
    let outcomes = run.pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(
        edits(&fake),
        [
            edit("outlook", "a", "nm-spam", None),
            edit("outlook", "b", "nm-main", None),
            edit("outlook", "c", "nm-spam", None)
        ]
    );
    assert_eq!(fake.moves, [vec!["a".to_owned(), "c".to_owned()]]);
    assert_eq!(moves(&outcomes), [("outlook".into(), 3, 0, 1, 0)]);
    // Its category is written: the move alone is tried again, after a wait.
    let mut next = Fake::default();
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, now());
    assert_eq!(held(&outcomes), [("outlook".into(), 1)]);
    assert!(next.moves.is_empty());
    let later = now() + TimeDelta::minutes(15);
    let outcomes = run.pass(&mut cache, &mut next, PER_RUN, later);
    assert!(edits(&next).is_empty());
    assert_eq!(next.moves, [vec!["a".to_owned()]]);
    assert_eq!(moves(&outcomes), [("outlook".into(), 0, 1, 0, 0)]);
    assert_eq!(
        summary(&outcomes[0], "run").as_deref(),
        Some("outlook: 1 moved to Junk")
    );
    assert_eq!(listed(&cache), ["b", "c"]);
    let mut last = Fake::default();
    assert!(run.pass(&mut cache, &mut last, PER_RUN, later).is_empty());
}

#[test]
fn junk_false_moves_nothing_and_labels_false_still_moves_spam() {
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Spam, 0.95)));
    let mut fake = Fake::default();
    Run::new("").pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(fake.batches, [vec![seen(2, Some("nm-spam"), None, false)]]);
    let mut cache = Cache::open_in_memory().unwrap();
    imap(&mut cache, "work", 2, "b", 2, Some((Choice::Spam, 0.95)));
    let mut fake = Fake::default();
    Run::with("labels = false\n").pass(&mut cache, &mut fake, PER_RUN, now());
    assert_eq!(fake.batches, [vec![seen(2, None, None, true)]]);
}
