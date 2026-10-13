//! `neomail daemon`'s classifying (DEC-2026-10-13-daemon-classify): once the config switches it
//! on with `classify.daemon = true` (off by default, DEC-2026-10-14-classify-when-asked), after a
//! wake's sync and notifications, each account's new mail goes to the backend through the same
//! [`job_for`] and [`send`] as `neomail classify`, so the two send the same requests. The daemon
//! never asks: it sends only with a key and the consent `neomail classify` stored for the
//! backend's host, and otherwise says once why classifying is off. While the switch is off it
//! reads no key and sends nothing.
//!
//! The daemon wakes the classifier only after a wake that synced ([`crate::daemon::after_sync`]):
//! a round, a resume, a push sync or an Outlook check. A wake sends at most [`PER_WAKE`] requests
//! and starts none once its [`budget`] is spent; the rest wait in memory for the next sync, as do
//! the messages of a run that stopped. A stop that
//! would happen again on every wake pauses classifying instead: a refused key until the key
//! read differs, an answer that is not jev's or a 4xx such as 404 until `classify.url` or
//! `classify.model` does, and a 402 (no credit left) for [`CREDIT_PAUSE`] or until the key
//! differs, while what waits stays queued. What waits is lost when the daemon ends, and
//! `neomail classify` still finds it.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use nm_core::{Cache, Classification, Group, GroupRules, MessageId};
use tokio::runtime::Runtime;

use super::{DEFAULT_DAYS, Job, Outcome, Reader, Repeats, Sink, job_for, send};
use crate::config::ClassifySettings;
use crate::daemon::Line;
use crate::keyring::{self, Keyring};
use crate::notify::NewMail;
use crate::oauth::Secret;

/// The most requests one wake sends, so that classifying never holds up the next sync for long:
/// one sync can add 500 messages (`nm_core::sync::DEFAULT_LIMIT`).
pub const PER_WAKE: usize = 100;

/// The longest a wake classifies: after it no request starts and no retry waits, so a slow or
/// rate-limited jev cannot hold up the next sync and its notifications. A request in flight
/// still gets its answer, within `http::TIMEOUT`.
pub const BUDGET: Duration = Duration::from_secs(60);

/// How long a 402 (out of credit) pauses classifying, unless the key changes first.
pub const CREDIT_PAUSE: TimeDelta = TimeDelta::hours(1);

/// How long a wake classifies: [`BUDGET`], or `interval` when the rounds are closer together
/// than that.
pub fn budget(interval: Duration) -> Duration {
    BUDGET.min(interval)
}

/// How the start line says background classifying is switched on.
const DISABLED_HOW: &str = "`daemon = true` under [classify] in the config turns it on";

/// Where the key and the consent come from: the keyring or `JEV_API_KEY`, and the consent file,
/// for the daemon ([`System`]); fixed answers for the tests, which open no keyring.
pub trait Access {
    /// The key for `host`, read anew; an error when the keyring fails to answer and
    /// `JEV_API_KEY` is not set, which is not the same as having no key.
    fn key(&mut self, host: &str) -> Result<Option<Secret>>;
    /// Whether `neomail classify` stored the user's consent for `host`, whatever version stored
    /// it: with the switch on, any yes for the host counts.
    fn consented(&mut self, host: &str) -> Result<bool>;
}

/// The keyring (else `JEV_API_KEY`) and the consent file in the data directory.
pub struct System {
    pub data_dir: PathBuf,
}

impl Access for System {
    fn key(&mut self, host: &str) -> Result<Option<Secret>> {
        // Outside `block_on`, as every keyring call (keyring.rs).
        let env = std::env::var(keyring::JEV_ENV).ok();
        let from_env = || keyring::jev_key(None, host, env.clone()).map(|(key, _)| key);
        // No keyring at all (no Secret Service in the daemon's session) is no key, as for
        // `neomail classify`: `JEV_API_KEY` is the way there.
        let Ok(store) = Keyring::open_for_jev() else {
            return Ok(from_env());
        };
        match store.get_jev_key(host) {
            Ok(Some(key)) if !key.expose().trim().is_empty() => Ok(Some(key)),
            Ok(_) => Ok(from_env()),
            // A keyring that is there but fails to answer may answer at the next wake.
            Err(e) => match from_env() {
                Some(key) => Ok(Some(key)),
                None => Err(e.context("cannot read the jev key from the keyring")),
            },
        }
    }

    fn consented(&mut self, host: &str) -> Result<bool> {
        super::consented(&self.data_dir, host)
    }
}

/// Whether the daemon classifies, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    On(String),
    Disabled,
    NoKey,
    NoConsent(String),
    /// A stop that would repeat ([`Pause`]); the reason, with the status.
    Paused(String),
    /// Out of credit (a 402) until a time ([`Pause::Credit`]); the reason, with that time. What
    /// waits stays queued, as do new messages.
    NoCredit(String),
    /// The settings, the keyring or the consent file cannot be read. What waits is kept, as
    /// this may pass by the next wake.
    Broken(String),
}

impl State {
    /// For the start line.
    fn phrase(&self) -> String {
        match self {
            State::On(host) => format!("classifying new mail with {host}"),
            State::Disabled => format!("classifying off ({DISABLED_HOW})"),
            State::NoKey => format!(
                "classifying off: no jev key ({})",
                keyring::NO_JEV_KEY.trim_start_matches("no jev key: ")
            ),
            State::NoConsent(host) => format!(
                "classifying off: {host} needs your consent once; run `neomail classify` on a \
                 terminal"
            ),
            State::Paused(why) | State::NoCredit(why) => format!("classifying paused: {why}"),
            State::Broken(why) => format!("classifying off: {why}"),
        }
    }

    /// For the line printed when the state changes, with the number of waiting messages it
    /// dropped.
    fn line(&self, dropped: usize) -> Line {
        Line::Error(match self {
            State::On(host) => format!("neomail daemon: classifying on ({host})"),
            other => {
                let left = match dropped {
                    0 => String::new(),
                    1 => "; 1 waiting message is left to `neomail classify`".to_owned(),
                    n => format!("; {n} waiting messages are left to `neomail classify`"),
                };
                format!("neomail daemon: {}{left}", other.phrase())
            }
        })
    }
}

/// The daemon's classifying between wakes: what waits, the refused key, what was last said.
pub struct Classifier {
    /// Messages for the next wake, oldest first, with their account.
    queue: VecDeque<(String, MessageId)>,
    /// The key the last gate read; none once classifying is off or paused, when only its digest
    /// is kept ([`Pause`]).
    key: Option<Secret>,
    /// What the last stop that would repeat was given, and why it stopped.
    paused: Option<(Pause, String)>,
    /// The state last reported, in the start line or since.
    state: State,
    /// The last run's stop, reported once while it repeats.
    last_stop: Option<String>,
}

impl Classifier {
    /// Reads the key, so the start line can say whether classifying is on.
    pub fn new(settings: &ClassifySettings, access: &mut dyn Access) -> Self {
        let mut classifier = Classifier {
            queue: VecDeque::new(),
            key: None,
            paused: None,
            state: State::Disabled,
            last_stop: None,
        };
        classifier.state = classifier.gate(settings, access, Utc::now());
        classifier
    }

    /// The start line's words on classifying.
    pub fn status(&self) -> String {
        self.state.phrase()
    }

    /// Whether messages wait for the next wake.
    #[cfg(test)]
    pub fn waiting(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Whether to classify now. Reads the key anew each time, as the consent: a key deleted
    /// with `neomail key delete` or changed with `neomail key set` counts from the next wake
    /// with work, not from the next start. `now` is the wake's time, against which a pause for
    /// credit ends.
    fn gate(
        &mut self,
        settings: &ClassifySettings,
        access: &mut dyn Access,
        now: DateTime<Utc>,
    ) -> State {
        self.key = None;
        if !settings.daemon {
            return State::Disabled;
        }
        let host = match settings.host() {
            Ok(host) => host,
            Err(e) => return State::Broken(format!("classify.url: {e:#}")),
        };
        self.key = match access.key(&host) {
            Ok(key) => key,
            Err(e) => return State::Broken(format!("{e:#}")),
        };
        let Some(key) = &self.key else {
            return State::NoKey;
        };
        if let Some((pause, why)) = &self.paused {
            let state = match pause {
                Pause::Key(digest) => (*digest == sha256(key)).then(|| State::Paused(why.clone())),
                Pause::Settings { url, model } => (*url == settings.url
                    && *model == settings.model)
                    .then(|| State::Paused(why.clone())),
                Pause::Credit { key: digest, until } => {
                    (*digest == sha256(key) && now < *until).then(|| State::NoCredit(why.clone()))
                }
            };
            if let Some(state) = state {
                // Nothing is sent while paused, so only the refused key's digest is kept.
                self.key = None;
                return state;
            }
            self.paused = None;
        }
        match access.consented(&host) {
            Ok(true) => State::On(host),
            Ok(false) => State::NoConsent(host),
            Err(e) => State::Broken(format!("{e:#}")),
        }
    }

    /// Reports `state` when it differs from the last one. Unless classifying is on or broken
    /// (which may pass), drops what waits, and the line says how many.
    fn report(&mut self, state: State, lines: &mut Vec<Line>) {
        let dropped = if matches!(state, State::On(_) | State::Broken(_) | State::NoCredit(_)) {
            0
        } else {
            let n = self.queue.len();
            self.queue.clear();
            n
        };
        if state != self.state {
            lines.push(state.line(dropped));
            self.state = state;
        }
    }

    /// Reports a run's stop when it differs from the last one.
    fn stopped(&mut self, why: String, lines: &mut Vec<Line>) {
        if self.last_stop.as_ref() != Some(&why) {
            let n = self.queue.len();
            let waits = if n == 1 {
                "1 message waits".to_owned()
            } else {
                format!("{n} messages wait")
            };
            lines.push(Line::Error(format!(
                "neomail daemon: classifying stopped: {why}; {waits} for the next sync"
            )));
            self.last_stop = Some(why);
        }
    }
}

/// What a stop that would repeat was given, so that classifying resumes once it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pause {
    /// The SHA-256 of the key jev refused; never the key itself.
    Key(Vec<u8>),
    /// `classify.url` and `classify.model` when the backend's answer was not jev's, or a 4xx
    /// that every request would get.
    Settings { url: String, model: String },
    /// A 402: the SHA-256 of the key that has no credit, and the wake's time from which
    /// classifying tries again.
    Credit { key: Vec<u8>, until: DateTime<Utc> },
}

fn sha256(key: &Secret) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, key.expose().as_bytes())
        .as_ref()
        .to_vec()
}

/// What one wake needs besides its new mail.
pub struct Wake<'a> {
    pub settings: &'a ClassifySettings,
    /// The user's own addresses, masked whole (`redact::owner_addresses`).
    pub owners: &'a [&'a str],
    pub runtime: &'a Runtime,
    /// The daemon's stop: once set, no request starts.
    pub stop: &'a Arc<AtomicBool>,
    /// How long this wake classifies, from when it starts ([`budget`]): no request starts
    /// after it, and what is left waits for the next wake.
    pub budget: Duration,
    pub now: DateTime<Utc>,
}

impl Classifier {
    /// Classifies what waits, then `new_mail` (never a first sync's or a rebuild's:
    /// [`NewMail::from_report`] leaves those out), at most [`PER_WAKE`] requests. Returns the
    /// lines to print: a state that changed, each account's results, a stop. Never fails: a
    /// failure is a line, and the sync and the notifications go on as before.
    pub fn wake(
        &mut self,
        new_mail: &[NewMail],
        wake: &Wake<'_>,
        cache: &mut Cache,
        access: &mut dyn Access,
    ) -> Vec<Line> {
        let deadline = Instant::now() + wake.budget;
        let mut lines = Vec::new();
        if wake.stop.load(Ordering::SeqCst) {
            return lines;
        }
        if !wake.settings.daemon {
            // A key read while the switch was on is not held while it is off.
            self.key = None;
            self.report(State::Disabled, &mut lines);
            return lines;
        }
        if new_mail.is_empty() && self.queue.is_empty() {
            return lines;
        }
        let state = self.gate(wake.settings, access, wake.now);
        self.report(state, &mut lines);
        if matches!(self.state, State::NoCredit(_)) {
            // Waits for the credit, with what waited before.
            for new in new_mail {
                self.queue
                    .extend(new.ids.iter().map(|id| (new.account.clone(), *id)));
            }
            return lines;
        }
        if !matches!(self.state, State::On(_)) {
            // Mail that came while off is left to `neomail classify`; what waited was dropped
            // by `report` unless this may pass.
            return lines;
        }
        for new in new_mail {
            self.queue
                .extend(new.ids.iter().map(|id| (new.account.clone(), *id)));
        }
        let jobs = match self.take(cache, wake) {
            Ok(jobs) => jobs,
            Err(e) => {
                self.stopped(format!("{e:#}"), &mut lines);
                return lines;
            }
        };
        if jobs.is_empty() {
            return lines;
        }
        let asked: Vec<(String, MessageId)> =
            jobs.iter().map(|j| (j.account.clone(), j.id)).collect();
        let Some(key) = self.key.clone() else {
            return lines;
        };
        let mut tally = Tally::default();
        let sent = send(
            wake.settings,
            &key,
            jobs,
            cache,
            &mut tally,
            wake.now,
            wake.runtime,
            wake.stop,
            Some(deadline),
        );
        let halted = wake.stop.load(Ordering::SeqCst);
        let (outcome, error) = match sent {
            Ok(mut outcome) => {
                let error = outcome.error.take();
                (outcome, error)
            }
            // Nothing was sent.
            Err(e) => {
                let retry = asked.iter().map(|(_, id)| *id).collect();
                (
                    Outcome {
                        retry,
                        ..Outcome::default()
                    },
                    Some(e),
                )
            }
        };
        // What neither got a stored result nor failed on its own (a 422, never asked again).
        let retry: HashSet<MessageId> = outcome.retry.iter().copied().collect();
        tally.forget(&retry);
        let stop = if let Some(repeats) = outcome.repeats {
            // Asking again would get the same answer each wake, paid for if it is a 2xx.
            let stopped = outcome.stopped.unwrap_or_default();
            let (pause, why) = match repeats {
                Repeats::Key => (Pause::Key(sha256(&key)), stopped),
                Repeats::Settings => (
                    Pause::Settings {
                        url: wake.settings.url.clone(),
                        model: wake.settings.model.clone(),
                    },
                    format!(
                        "{stopped}; change classify.url or classify.model, or restart the daemon"
                    ),
                ),
                Repeats::Credit => {
                    let until = wake.now + CREDIT_PAUSE;
                    let at = until.with_timezone(&chrono::Local).format("%H:%M");
                    (
                        Pause::Credit {
                            key: sha256(&key),
                            until,
                        },
                        format!(
                            "{stopped}; the jev account is out of credit; trying again at {at}, \
                             or sooner if the key changes"
                        ),
                    )
                }
            };
            let state = match pause {
                Pause::Credit { .. } => State::NoCredit(why.clone()),
                _ => State::Paused(why.clone()),
            };
            self.paused = Some((pause, why));
            self.key = None;
            // `report` drops them with the rest of what waits and says how many, unless they
            // wait for the credit.
            self.requeue(asked.into_iter().filter(|(_, id)| retry.contains(id)));
            self.report(state, &mut lines);
            None
        } else {
            // A message that ran out of tries, or whose result could not be stored, waits for
            // the next sync: it did not fail.
            if !halted {
                self.requeue(asked.into_iter().filter(|(_, id)| retry.contains(id)));
            }
            match (error, outcome.stopped) {
                (Some(e), _) => Some(format!("{e:#}")),
                // Left unasked with no stop of its own or the daemon's: the deadline.
                (None, None) if !halted && !retry.is_empty() => {
                    Some("out of time for this sync".to_owned())
                }
                (None, stopped) => stopped,
            }
        };
        lines.extend(tally.lines(&self.queue));
        match stop {
            Some(_) if halted => {}
            Some(why) => self.stopped(why, &mut lines),
            None => self.last_stop = None,
        }
        lines
    }

    /// Up to [`PER_WAKE`] jobs from the front of the queue.
    fn take(&mut self, cache: &mut Cache, wake: &Wake<'_>) -> Result<Vec<Job>> {
        let reader = Reader {
            owners: wake.owners,
            model: &wake.settings.model,
        };
        let cutoff = wake.now - TimeDelta::days(i64::from(DEFAULT_DAYS));
        let mut seen = HashSet::new();
        let mut jobs = Vec::new();
        while jobs.len() < PER_WAKE {
            let Some((account, id)) = self.queue.pop_front() else {
                break;
            };
            match job_for(cache, id, &reader, cutoff, &mut seen) {
                Ok(Some(job)) => jobs.push(job),
                Ok(None) => {}
                Err(e) => {
                    self.queue.push_front((account, id));
                    let asked = jobs.into_iter().map(|j: Job| (j.account, j.id));
                    self.requeue(asked);
                    return Err(e);
                }
            }
        }
        Ok(jobs)
    }

    /// Puts messages back at the front of the queue, in their order.
    fn requeue(&mut self, messages: impl DoubleEndedIterator<Item = (String, MessageId)>) {
        for message in messages.rev() {
            self.queue.push_front(message);
        }
    }
}

/// Each account's results in a run, for its line: never a subject or a sender.
#[derive(Default)]
struct Tally {
    accounts: BTreeMap<String, Count>,
}

#[derive(Default)]
struct Count {
    groups: BTreeMap<Group, usize>,
    failed: HashSet<MessageId>,
}

impl Sink for Tally {
    fn result(
        &mut self,
        job: &Job,
        result: &Classification,
        rules: &GroupRules,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let group = rules.effective(result.choice, result.confidence, job.received, now);
        let count = self.accounts.entry(job.account.clone()).or_default();
        *count.groups.entry(group).or_default() += 1;
        Ok(())
    }

    fn failed(&mut self, job: &Job, _why: &str) -> Result<()> {
        let count = self.accounts.entry(job.account.clone()).or_default();
        count.failed.insert(job.id);
        Ok(())
    }
}

impl Tally {
    /// Forgets the failures of messages that are asked about again.
    fn forget(&mut self, retry: &HashSet<MessageId>) {
        for count in self.accounts.values_mut() {
            count.failed.retain(|id| !retry.contains(id));
        }
        self.accounts
            .retain(|_, c| !c.groups.is_empty() || !c.failed.is_empty());
    }

    /// `work: classified 3 (newsletters 2, main 1), 1 failed, 20 wait for the next sync`.
    fn lines(&self, queue: &VecDeque<(String, MessageId)>) -> Vec<Line> {
        self.accounts
            .iter()
            .map(|(account, count)| {
                let classified: usize = count.groups.values().sum();
                let mut groups: Vec<(&Group, &usize)> = count.groups.iter().collect();
                groups.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                let mut line = format!("{account}: classified {classified}");
                if !groups.is_empty() {
                    let named: Vec<String> = groups
                        .iter()
                        .map(|(g, n)| format!("{} {n}", g.name()))
                        .collect();
                    line.push_str(&format!(" ({})", named.join(", ")));
                }
                if !count.failed.is_empty() {
                    line.push_str(&format!(", {} failed", count.failed.len()));
                }
                let waiting = queue.iter().filter(|(a, _)| a == account).count();
                if waiting > 0 {
                    line.push_str(&format!(", {waiting} wait for the next sync"));
                }
                Line::Report(line)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
