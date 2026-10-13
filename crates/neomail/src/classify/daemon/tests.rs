//! The daemon's classifying against the fake jev on 127.0.0.1 (tests/support/fake_jev.rs) and a
//! cache in memory. The key and the consent are fixed answers, so no keyring or data directory
//! is opened, and retries are told to wait 0 seconds.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use nm_core::{Cache, FetchedMessage, Flags, MessageId, Uid, UidValidity};
use tokio::runtime::Runtime;

use super::{Access, CREDIT_PAUSE, Classifier, PER_WAKE, Wake};
use crate::classify::Reader;
use crate::config::ClassifySettings;
use crate::daemon::Line;
use crate::notify::NewMail;
use crate::oauth::Secret;

#[path = "../../../tests/support/fake_jev.rs"]
mod fake_jev;
use fake_jev::{FakeJev, Reply};

const OWNER: &str = "you@example.com";

/// Fixed answers in place of the keyring and the consent file. Counts the key reads.
struct Fixed {
    key: Option<&'static str>,
    /// The keyring fails to answer, as when the Secret Service restarts.
    keyring_fails: bool,
    consent: bool,
    reads: usize,
}

impl Access for Fixed {
    fn key(&mut self, _host: &str) -> anyhow::Result<Option<Secret>> {
        self.reads += 1;
        if self.keyring_fails {
            anyhow::bail!("cannot read the jev key from the keyring: no answer");
        }
        Ok(self.key.map(|k| Secret::new(k.to_owned())))
    }

    fn consented(&mut self, host: &str) -> anyhow::Result<bool> {
        assert_eq!(host, "127.0.0.1");
        Ok(self.consent)
    }
}

fn granted() -> Fixed {
    Fixed {
        key: Some("key-one"),
        keyring_fails: false,
        consent: true,
        reads: 0,
    }
}

/// A message `days` old from a named sender. The fake answers with the subject's first word.
fn mail(uid: u32, days: i64, subject: &str) -> FetchedMessage {
    FetchedMessage {
        uid: Uid::new(uid).unwrap(),
        flags: Flags::new(Vec::<String>::new()),
        received: Some(Utc::now() - TimeDelta::days(days)),
        raw: format!(
            "Message-ID: <m{uid}@acme.example>\r\nFrom: Alice Smith <alice.smith@shop.example>\r\n\
             To: {OWNER}\r\nSubject: {subject}\r\n\r\nHello {OWNER}, see you soon.\r\n"
        )
        .into_bytes(),
    }
}

/// `n` messages a day old, all answered "main".
fn many(n: u32) -> Vec<FetchedMessage> {
    (1..=n).map(|uid| mail(uid, 1, "main Hello")).collect()
}

/// The daemon's surroundings: the fake, the cache, the settings, the runtime and the stop flag.
struct World {
    jev: FakeJev,
    cache: Cache,
    settings: ClassifySettings,
    runtime: Runtime,
    stop: Arc<AtomicBool>,
    /// Each wake's time for classifying.
    budget: Duration,
    /// Each wake's own time.
    now: DateTime<Utc>,
    mailboxes: HashMap<String, i64>,
}

impl World {
    fn new() -> Self {
        let jev = FakeJev::start();
        let settings = ClassifySettings {
            url: jev.url(),
            ..ClassifySettings::default()
        };
        World {
            jev,
            cache: Cache::open_in_memory().unwrap(),
            settings,
            runtime: crate::account::runtime().unwrap(),
            stop: Arc::new(AtomicBool::new(false)),
            budget: super::BUDGET,
            now: Utc::now(),
            mailboxes: HashMap::new(),
        }
    }

    /// Caches `messages` in `mailbox` of "work" and returns them as a sync's new mail.
    fn arrive(&mut self, mailbox: &str, messages: &[FetchedMessage]) -> Vec<NewMail> {
        let before = self.ids();
        let id = match self.mailboxes.get(mailbox) {
            Some(id) => *id,
            None => {
                let validity = UidValidity::new(1).unwrap();
                let record = self.cache.reset_mailbox("work", mailbox, validity).unwrap();
                self.mailboxes.insert(mailbox.to_owned(), record.id);
                record.id
            }
        };
        self.cache.insert_messages(id, messages).unwrap();
        let mut ids: Vec<MessageId> = self.ids().difference(&before).copied().collect();
        ids.sort();
        vec![NewMail {
            account: "work".to_owned(),
            ids,
        }]
    }

    fn ids(&self) -> HashSet<MessageId> {
        let listed = self.cache.list(Some("work"), 10_000).unwrap();
        listed.into_iter().map(|e| e.id).collect()
    }

    /// One wake; its lines as `out …` (standard output) and `err …` (standard error).
    fn wake(&mut self, c: &mut Classifier, new: &[NewMail], access: &mut Fixed) -> Vec<String> {
        let owners = [OWNER];
        let wake = Wake {
            settings: &self.settings,
            owners: &owners,
            runtime: &self.runtime,
            stop: &self.stop,
            budget: self.budget,
            now: self.now,
        };
        let lines = c.wake(new, &wake, &mut self.cache, access);
        lines
            .into_iter()
            .map(|line| match line {
                Line::Report(s) => format!("out {s}"),
                Line::Error(s) => format!("err {s}"),
            })
            .collect()
    }

    /// The request `neomail classify` builds for `id`.
    fn request(&mut self, id: MessageId) -> Vec<u8> {
        let reader = Reader {
            owners: &[OWNER],
            model: &self.settings.model,
        };
        reader.read(&mut self.cache, id).unwrap().unwrap().request
    }

    /// Whether `id` has a stored result.
    fn stored(&mut self, id: MessageId) -> bool {
        let reader = Reader {
            owners: &[OWNER],
            model: &self.settings.model,
        };
        let job = reader.read(&mut self.cache, id).unwrap().unwrap();
        let result = self.cache.classification(&job.account, &job.msg_key);
        result.unwrap().is_some()
    }
}

/// jev's 503 with no wait, so the retries take no time.
fn unavailable() -> Reply {
    Reply {
        status: 503,
        headers: vec![("Retry-After", "0")],
        body: "{}".to_owned(),
    }
}

#[test]
fn new_mail_is_sent_as_neomail_classify_sends_it_and_never_twice() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    assert_eq!(c.status(), "classifying new mail with 127.0.0.1");
    let weekly = mail(1, 1, "newsletters Weekly deals");
    let mut new = w.arrive(
        "INBOX",
        &[weekly.clone(), mail(2, 2, "main Lunch on Friday")],
    );
    // The same message in a second mailbox of the same wake.
    new[0]
        .ids
        .extend(&w.arrive("Archive", std::slice::from_ref(&weekly))[0].ids);
    let lines = w.wake(&mut c, &new, &mut access);
    assert_eq!(lines, ["out work: classified 2 (main 1, newsletters 1)"]);

    // The one owner of what leaves: the bytes `neomail classify` builds, and no sender's name or
    // local part.
    let inbox = &new[0].ids[..2];
    let mut expected: Vec<String> = inbox
        .iter()
        .map(|id| String::from_utf8(w.request(*id)).unwrap())
        .collect();
    let mut sent: Vec<String> = w
        .jev
        .captured
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.body.clone())
        .collect();
    expected.sort();
    sent.sort();
    assert_eq!(sent, expected);
    for body in &sent {
        assert!(!body.contains("alice") && !body.contains("Alice"), "{body}");
        assert!(!body.contains(OWNER), "{body}");
    }
    let authorization = w.jev.captured.lock().unwrap()[0].authorization.clone();
    assert_eq!(authorization.as_deref(), Some("Bearer key-one"));
    assert!(inbox.iter().all(|id| w.stored(*id)));
    let logged = lines.join("\n");
    assert!(
        !logged.contains("Weekly") && !logged.contains("lice"),
        "{logged}"
    );

    // A third copy, in a later wake: it already has a result.
    let again = w.arrive("Sent", &[weekly]);
    assert_eq!(w.wake(&mut c, &again, &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), 2);
}

#[test]
fn without_consent_nothing_is_sent_or_kept_and_the_reason_is_said_once() {
    let mut w = World::new();
    let mut access = Fixed {
        consent: false,
        ..granted()
    };
    let mut c = Classifier::new(&w.settings, &mut access);
    assert_eq!(
        c.status(),
        "classifying off: 127.0.0.1 needs your consent once; run `neomail classify` on a terminal"
    );
    for uid in 1..=3 {
        let new = w.arrive("INBOX", &[mail(uid, 1, "main Hi")]);
        assert_eq!(w.wake(&mut c, &new, &mut access), Vec::<String>::new());
    }
    assert_eq!(w.jev.count(), 0);

    access.consent = true;
    let new = w.arrive("INBOX", &[mail(4, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 1 (main 1)"
        ]
    );
    // Only this wake's mail: what came while off is left to `neomail classify`.
    assert_eq!(w.jev.count(), 1);

    // The consent is read again at each wake with mail: withdrawn, nothing more is sent.
    access.consent = false;
    let new = w.arrive("INBOX", &[mail(5, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying off: 127.0.0.1 needs your consent once; run \
             `neomail classify` on a terminal"
        ]
    );
    assert_eq!(w.jev.count(), 1);
}

#[test]
fn without_a_key_nothing_is_sent_and_the_key_is_read_again_at_each_wake_with_mail() {
    let mut w = World::new();
    let mut access = Fixed {
        key: None,
        ..granted()
    };
    let mut c = Classifier::new(&w.settings, &mut access);
    assert_eq!(
        c.status(),
        "classifying off: no jev key (run `neomail key set` or set JEV_API_KEY)"
    );
    for uid in 1..=3 {
        let new = w.arrive("INBOX", &[mail(uid, 1, "main Hi")]);
        assert_eq!(w.wake(&mut c, &new, &mut access), Vec::<String>::new());
    }
    assert_eq!(w.jev.count(), 0);
    assert_eq!(access.reads, 4, "at start and at each wake with mail");

    access.key = Some("key-one");
    let new = w.arrive("INBOX", &[mail(4, 1, "main Hi")]);
    let lines = w.wake(&mut c, &new, &mut access);
    assert_eq!(lines[0], "err neomail daemon: classifying on (127.0.0.1)");
    let new = w.arrive("INBOX", &[mail(5, 1, "main Hi")]);
    w.wake(&mut c, &new, &mut access);
    assert_eq!(w.jev.count(), 2);
    assert_eq!(access.reads, 6, "at start and at each wake with mail");
}

#[test]
fn a_key_deleted_while_running_is_not_sent_again() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
    w.wake(&mut c, &new, &mut access);
    assert_eq!(w.jev.count(), 1);

    // `neomail key delete` while the daemon runs.
    access.key = None;
    let new = w.arrive("INBOX", &[mail(2, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying off: no jev key (run `neomail key set` or set \
          JEV_API_KEY)"
        ]
    );
    assert_eq!(w.jev.count(), 1, "the deleted key is not sent");
    assert!(
        !c.waiting(),
        "what came while off is left to `neomail classify`"
    );
}

#[test]
fn a_refused_key_pauses_classifying_until_the_key_changes() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev.play([Reply::status(401, "{}")]);
    let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying paused: jev refused the key (401): run `neomail key \
             set` or set JEV_API_KEY; 1 waiting message is left to `neomail classify`"
        ]
    );
    let new = w.arrive("INBOX", &[mail(2, 1, "main Hi")]);
    assert_eq!(w.wake(&mut c, &new, &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), 1, "the same key is not sent again");
    assert!(c.key.is_none(), "only the refused key's digest is kept");

    access.key = Some("key-two");
    let new = w.arrive("INBOX", &[mail(3, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 1 (main 1)"
        ]
    );
    assert_eq!(w.jev.count(), 2);
    let last = w.jev.captured.lock().unwrap()[1].authorization.clone();
    assert_eq!(last.as_deref(), Some("Bearer key-two"));
}

#[test]
fn a_run_that_stops_puts_its_messages_back_for_the_next_wake() {
    // Both ways a run stops for a while: an outage that outlasts a message's tries, and a
    // backend that cannot be reached. More messages than workers, so some are never asked.
    for unreachable in [false, true] {
        let mut w = World::new();
        let mut access = granted();
        let mut c = Classifier::new(&w.settings, &mut access);
        if unreachable {
            let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            w.settings.url = format!("http://{}", closed.local_addr().unwrap());
        } else {
            w.jev.play(std::iter::repeat_n(unavailable(), 6 * 3));
        }
        let new = w.arrive("INBOX", &many(6));
        let lines = w.wake(&mut c, &new, &mut access);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("err neomail daemon: classifying stopped: ")
                && lines[0].ends_with("; 6 messages wait for the next round"),
            "{lines:?}"
        );
        assert!(c.waiting());

        if unreachable {
            assert_eq!(w.jev.count(), 0);
        }

        // The next wake, with the backend back, has no new mail and asks about all six, once
        // each. A fresh fake, as the outage's 503s left over are not this wake's.
        w.jev = FakeJev::start();
        w.settings.url = w.jev.url();
        assert_eq!(
            w.wake(&mut c, &[], &mut access),
            ["out work: classified 6 (main 6)"]
        );
        assert_eq!(w.jev.count(), 6);
        assert!(!c.waiting());
        assert!(new[0].ids.clone().into_iter().all(|id| w.stored(id)));
    }
}

#[test]
fn a_422_fails_one_message_which_is_not_asked_again() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev.play([Reply::status(422, "{}")]);
    let new = w.arrive("INBOX", &many(2));
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        ["out work: classified 1 (main 1), 1 failed"]
    );
    assert!(!c.waiting());
    assert_eq!(w.wake(&mut c, &[], &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), 2);
}

#[test]
fn an_answer_that_is_not_jevs_pauses_classifying_until_the_settings_change() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev
        .play([Reply::status(200, "<html>a login page</html>")]);
    let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
    let lines = w.wake(&mut c, &new, &mut access);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with(
            "err neomail daemon: classifying paused: the backend's answer is not jev's"
        ) && lines[0].ends_with("; change classify.url or classify.model, or restart the daemon"),
        "{lines:?}"
    );
    assert_eq!(lines[1], "out work: classified 0, 1 failed");
    assert!(!c.waiting(), "nothing waits to be sent again");

    // Neither what failed nor new mail is sent again, and the pause is not said again.
    let new = w.arrive("INBOX", &[mail(2, 1, "main Hi")]);
    assert_eq!(w.wake(&mut c, &new, &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), 1);

    w.settings.model = "another-model".to_owned();
    let new = w.arrive("INBOX", &[mail(3, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 1 (main 1)"
        ]
    );
    assert_eq!(w.jev.count(), 2);
}

#[test]
fn a_4xx_every_request_would_get_pauses_until_the_settings_change() {
    for status in [400, 404] {
        let mut w = World::new();
        let mut access = granted();
        let mut c = Classifier::new(&w.settings, &mut access);
        w.jev.play([Reply::status(status, "{}")]);
        let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
        let lines = w.wake(&mut c, &new, &mut access);
        assert!(
            lines.len() == 1
                && lines[0].starts_with(&format!(
                    "err neomail daemon: classifying paused: jev answered {status}"
                )),
            "{status}: {lines:?}"
        );
        assert!(!c.waiting(), "{status}");
        assert!(c.key.is_none(), "{status}: the paused key is not kept");
        let new = w.arrive("INBOX", &[mail(2, 1, "main Hi")]);
        assert_eq!(w.wake(&mut c, &new, &mut access), Vec::<String>::new());
        assert_eq!(w.jev.count(), 1, "{status}");
    }
}

/// jev's 402 for each request of a wake: no credit left.
fn no_credit(n: usize) -> impl Iterator<Item = Reply> {
    std::iter::repeat_n(Reply::status(402, "{}"), n)
}

#[test]
fn a_402_pauses_for_an_hour_says_so_once_and_keeps_what_waits() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev.play(no_credit(PER_WAKE));
    let first = w.arrive("INBOX", &many(8));
    let lines = w.wake(&mut c, &first, &mut access);
    let at = (w.now + CREDIT_PAUSE)
        .with_timezone(&chrono::Local)
        .format("%H:%M");
    assert_eq!(
        lines,
        [format!(
            "err neomail daemon: classifying paused: jev answered 402: {{}}; the jev account is \
             out of credit; trying again at {at}, or sooner if the key changes"
        )]
    );
    let sent = w.jev.count();
    assert!((1..=8).contains(&sent), "{sent}");
    assert!(c.waiting());
    assert!(
        c.key.is_none(),
        "only the key's digest is kept while paused"
    );

    // Within the hour no request goes out and nothing more is said, with or without new mail;
    // new mail waits with the rest.
    let more = w.arrive("INBOX", &[mail(9, 1, "main Later")]);
    w.now += TimeDelta::minutes(30);
    assert_eq!(w.wake(&mut c, &more, &mut access), Vec::<String>::new());
    w.now += TimeDelta::minutes(29);
    assert_eq!(w.wake(&mut c, &[], &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), sent);
    assert!(c.waiting());

    // After the hour, with credit again, all nine are sent, each once.
    w.jev.script.lock().unwrap().clear();
    w.now += TimeDelta::minutes(1);
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 9 (main 9)"
        ]
    );
    assert_eq!(w.jev.count(), sent + 9);
    assert!(!c.waiting());
    assert!(
        first[0]
            .ids
            .iter()
            .chain(&more[0].ids)
            .all(|id| w.stored(*id))
    );
}

#[test]
fn a_402_pause_lifts_when_the_key_changes() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev.play(no_credit(PER_WAKE));
    let new = w.arrive("INBOX", &many(3));
    assert_eq!(w.wake(&mut c, &new, &mut access).len(), 1);
    let sent = w.jev.count();
    w.now += TimeDelta::minutes(5);
    assert_eq!(w.wake(&mut c, &[], &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), sent);

    w.jev.script.lock().unwrap().clear();
    access.key = Some("key-two");
    w.now += TimeDelta::minutes(5);
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 3 (main 3)"
        ]
    );
    assert_eq!(w.jev.count(), sent + 3);
}

#[test]
fn a_402_after_the_pause_pauses_again_with_one_new_line() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.jev.play(no_credit(2 * PER_WAKE));
    let new = w.arrive("INBOX", &many(2));
    assert_eq!(w.wake(&mut c, &new, &mut access).len(), 1);
    w.now += CREDIT_PAUSE;
    let lines = w.wake(&mut c, &[], &mut access);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], "err neomail daemon: classifying on (127.0.0.1)");
    assert!(lines[1].contains("out of credit"), "{lines:?}");
    let sent = w.jev.count();
    w.now += TimeDelta::minutes(10);
    assert_eq!(w.wake(&mut c, &[], &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), sent);
    assert!(c.waiting());
}

#[test]
fn a_wake_sends_at_most_its_share_and_the_next_wake_sends_the_rest() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    let new = w.arrive("INBOX", &many(150));
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        ["out work: classified 100 (main 100), 50 wait for the next round"]
    );
    assert_eq!(w.jev.count(), PER_WAKE);
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        ["out work: classified 50 (main 50)"]
    );
    assert_eq!(w.jev.count(), 150);
    assert!(new[0].ids.iter().all(|id| w.stored(*id)));
}

#[test]
fn mail_received_more_than_a_year_ago_is_left_alone() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    let new = w.arrive(
        "INBOX",
        &[mail(1, 400, "main Old"), mail(2, 300, "main Newer")],
    );
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        ["out work: classified 1 (main 1)"]
    );
    assert_eq!(w.jev.count(), 1);
    assert!(!w.stored(new[0].ids[0]) && w.stored(new[0].ids[1]));
}

#[test]
fn classify_daemon_false_sends_nothing_and_drops_what_waits() {
    let mut w = World::new();
    let mut access = granted();
    w.settings.daemon = false;
    let c = Classifier::new(&w.settings, &mut access);
    assert_eq!(c.status(), "classifying off (classify.daemon = false)");
    assert_eq!(access.reads, 0, "no key is read for nothing");

    w.settings.daemon = true;
    let mut c = Classifier::new(&w.settings, &mut access);
    let new = w.arrive("INBOX", &many(150));
    w.wake(&mut c, &new, &mut access);
    assert!(c.waiting());
    w.settings.daemon = false;
    let new = w.arrive("INBOX", &[mail(151, 1, "main Hi")]);
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying off (classify.daemon = false); 50 waiting messages \
             are left to `neomail classify`"
        ]
    );
    assert!(!c.waiting());
    w.settings.daemon = true;
    assert_eq!(w.wake(&mut c, &[], &mut access), Vec::<String>::new());
    assert_eq!(w.jev.count(), PER_WAKE);
}

#[test]
fn a_stop_starts_no_new_request_and_keeps_what_was_stored() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);

    // A jev that answers each request, then sets the daemon's stop.
    let (url, captured) = stopping_jev(&w.stop, VecDeque::new());
    w.settings.url = url;
    let new = w.arrive("INBOX", &many(10));
    let lines = w.wake(&mut c, &new, &mut access);
    let sent = captured.lock().unwrap().len();
    // Every worker starts one request at once. The fake sets the stop only after writing its
    // first answer, so the worker it answered may start one more before the stop exists; every
    // other worker gets its answer after the stop. Nothing starts after it: ignoring the stop
    // sends all 10.
    assert!(
        (1..=super::super::CONCURRENCY + 1).contains(&sent),
        "{sent} requests"
    );
    assert!(lines.iter().all(|l| !l.contains("stopped")), "{lines:?}");
    let stored = new[0].ids.iter().filter(|id| w.stored(**id)).count();
    assert!(stored >= 1, "the answer that came before the stop is kept");

    // Once stopped, a wake with new mail sends nothing at all.
    let more = w.arrive("INBOX", &[mail(12, 1, "main Hi")]);
    assert_eq!(w.wake(&mut c, &more, &mut access), Vec::<String>::new());
    assert_eq!(captured.lock().unwrap().len(), sent);
}

/// A jev on 127.0.0.1 that answers each request from `script` (else as the fake does), then
/// sets `stop`, as a SIGTERM would. Its url, and the requests it got.
fn stopping_jev(
    stop: &Arc<AtomicBool>,
    script: VecDeque<Reply>,
) -> (String, Arc<Mutex<Vec<fake_jev::Captured>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stop = Arc::clone(stop);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&captured);
    std::thread::spawn(move || {
        let script = Mutex::new(script);
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            fake_jev::serve(stream, &seen, &script);
            stop.store(true, Ordering::SeqCst);
        }
    });
    (url, captured)
}

#[test]
fn a_stop_ends_a_retry_wait_at_once() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    let limited = Reply {
        status: 429,
        headers: vec![("Retry-After", "30")],
        body: "{}".to_owned(),
    };
    let (url, captured) = stopping_jev(&w.stop, VecDeque::from([limited]));
    w.settings.url = url;
    let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
    let started = Instant::now();
    w.wake(&mut c, &new, &mut access);
    // Well within the 30 s jev asked for, and within the 60 s budget.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(captured.lock().unwrap().len(), 1);
}

#[test]
fn a_keyring_that_fails_to_answer_keeps_what_waits_for_the_next_wake() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.budget = Duration::ZERO;
    let new = w.arrive("INBOX", &many(3));
    w.wake(&mut c, &new, &mut access);
    assert!(c.waiting());

    w.budget = super::BUDGET;
    access.keyring_fails = true;
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        [
            "err neomail daemon: classifying off: cannot read the jev key from the keyring: no \
             answer"
        ]
    );
    assert_eq!(w.jev.count(), 0);
    assert!(c.waiting());

    access.keyring_fails = false;
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        [
            "err neomail daemon: classifying on (127.0.0.1)",
            "out work: classified 3 (main 3)"
        ]
    );
}

/// Fails to print a message's failure, as a closed pipe would.
struct ClosedPipe;

impl crate::classify::Sink for ClosedPipe {
    fn result(
        &mut self,
        _: &crate::classify::Job,
        _: &nm_core::Classification,
        _: &nm_core::GroupRules,
        _: chrono::DateTime<Utc>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn failed(&mut self, _: &crate::classify::Job, _: &str) -> anyhow::Result<()> {
        anyhow::bail!("closed pipe")
    }
}

#[test]
fn a_run_that_fails_to_print_or_store_still_settles_a_422() {
    let mut w = World::new();
    w.jev.play([Reply::status(422, "{}")]);
    let new = w.arrive("INBOX", &[mail(1, 1, "main Hi")]);
    let reader = Reader {
        owners: &[OWNER],
        model: &w.settings.model,
    };
    let job = reader.read(&mut w.cache, new[0].ids[0]).unwrap().unwrap();
    let key = Secret::new("key-one".to_owned());
    let outcome = crate::classify::send(
        &w.settings,
        &key,
        vec![job],
        &mut w.cache,
        &mut ClosedPipe,
        Utc::now(),
        &w.runtime,
        &w.stop,
        None,
    )
    .unwrap();
    assert!(outcome.error.is_some());
    assert_eq!(outcome.failed, 1);
    // The daemon puts back only `retry`: a 422 is never sent again.
    assert_eq!(outcome.retry, Vec::<MessageId>::new());
}

#[test]
fn a_wake_out_of_time_starts_no_request_and_the_next_wake_sends_what_waits() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.budget = Duration::ZERO;
    let new = w.arrive("INBOX", &many(3));
    assert_eq!(
        w.wake(&mut c, &new, &mut access),
        [
            "err neomail daemon: classifying stopped: out of time for this round; 3 messages wait \
          for the next round"
        ]
    );
    assert_eq!(w.jev.count(), 0);

    w.budget = super::BUDGET;
    assert_eq!(
        w.wake(&mut c, &[], &mut access),
        ["out work: classified 3 (main 3)"]
    );
    assert_eq!(w.jev.count(), 3);
}

#[test]
fn a_retry_that_would_wait_past_the_budget_leaves_the_rest_for_the_next_wake() {
    let mut w = World::new();
    let mut access = granted();
    let mut c = Classifier::new(&w.settings, &mut access);
    w.budget = Duration::from_secs(5);
    let limited = Reply {
        status: 429,
        headers: vec![("Retry-After", "30")],
        body: "{}".to_owned(),
    };
    w.jev.play(std::iter::repeat_n(limited, 10));
    let new = w.arrive("INBOX", &many(10));
    let started = Instant::now();
    let lines = w.wake(&mut c, &new, &mut access);
    assert!(started.elapsed() < w.budget, "{:?}", started.elapsed());
    assert_eq!(
        lines,
        [
            "err neomail daemon: classifying stopped: out of time for this round; 10 messages wait \
          for the next round"
        ]
    );
    // One request per worker at most: none starts after jev asked for a wait past the budget.
    let sent = w.jev.count();
    assert!(
        (1..=super::super::CONCURRENCY).contains(&sent),
        "{sent} requests"
    );
    assert!(c.waiting());
}
