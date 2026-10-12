//! Push in the daemon: account wakes and their settle time, the watchers' start limit and push
//! lines, a resume and a stop. The watchers are fakes that send scripted events; time runs on
//! tokio's paused clock.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use nm_imap::Login;

use super::*;
use crate::config::Server;
use crate::push::{Events, Kind};

/// One step of a fake watcher.
#[derive(Clone)]
enum Step {
    Wait(u64),
    Send(Kind),
    /// Idle until ended.
    Idle,
}

use Step::{Idle, Send, Wait};

/// What the fake watchers did: each start as (account, second), the live ones, and each account's
/// latest way to the daemon.
#[derive(Clone, Default)]
struct Log {
    starts: Arc<Mutex<Vec<(String, u64)>>>,
    alive: Arc<AtomicUsize>,
    events: Arc<Mutex<HashMap<String, Events>>>,
}

impl Log {
    fn alive(&self) -> usize {
        self.alive.load(Ordering::SeqCst)
    }

    fn starts(&self, account: &str) -> Vec<u64> {
        let starts = self.starts.lock().unwrap();
        starts
            .iter()
            .filter(|(a, _)| a == account)
            .map(|(_, s)| *s)
            .collect()
    }
}

/// Counts a fake watcher as alive until its future is dropped.
struct Alive(Arc<AtomicUsize>);

impl Alive {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(count))
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

type Fake = Pin<Box<dyn Future<Output = ()> + std::marker::Send + 'static>>;

/// Fake watchers: an account's n-th start (from 0) runs `script(account, n)`.
fn fakes(
    log: Log,
    t0: Instant,
    script: impl Fn(&str, usize) -> Vec<Step>,
) -> impl FnMut(WatchLogin, Events) -> Fake {
    let mut counts = HashMap::<String, usize>::new();
    move |login, events| {
        // A fake has no use for the secret either.
        drop(login);
        let account = events.account().to_owned();
        let n = counts.entry(account.clone()).or_default();
        let steps = script(&account, *n);
        *n += 1;
        let log = log.clone();
        let alive = Alive::new(&log.alive);
        Box::pin(async move {
            let _alive = alive;
            let secs = (Instant::now() - t0).as_secs();
            log.starts.lock().unwrap().push((account.clone(), secs));
            log.events.lock().unwrap().insert(account, events.clone());
            for step in steps {
                match step {
                    Wait(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                    Send(kind) => events.send(kind),
                    Idle => std::future::pending().await,
                }
            }
        })
    }
}

/// A successful sync that got `new_ids` and hands over its login.
fn synced(account: &Account, new_ids: &[i64]) -> Synced {
    let Server::Imap(imap) = &account.server else {
        unreachable!("the test accounts are IMAP accounts");
    };
    Synced {
        report: Ok(SyncReport {
            new: new_ids.len(),
            new_ids: new_ids.iter().copied().map(MessageId::new).collect(),
            ..SyncReport::default()
        }),
        partial: SyncReport::default(),
        warnings: Vec::new(),
        watch: Some(WatchLogin::new(imap, Login::Password("secret"))),
    }
}

/// What a daemon run did.
struct Run {
    /// Each sync as (second, account).
    syncs: Vec<(u64, String)>,
    /// Each notification as (second, summary).
    notified: Vec<(u64, String)>,
    err: String,
    log: Log,
    /// The watchers alive at the stop, before it ended them.
    alive: usize,
}

impl Run {
    fn syncs(&self, account: &str) -> Vec<u64> {
        let syncs = self.syncs.iter().filter(|(_, a)| a == account);
        syncs.map(|(s, _)| *s).collect()
    }
}

/// The daemon's loop, as `run` drives it, for "work" and "home" every 5 minutes, until the first
/// wake at or after `until` seconds. `sync` gets the second, the account and the log; `jump` is
/// what the wall clock adds (a suspend). Every watcher is ended at the end, as `run` does.
fn daemon(
    until: u64,
    jump: impl Fn(Duration) -> Duration,
    script: impl Fn(&str, usize) -> Vec<Step>,
    mut sync: impl FnMut(&Runtime, u64, &Account, &Log) -> Synced,
) -> Run {
    let runtime = paused_runtime();
    let stop = Stop::new().unwrap();
    let t0 = now(&runtime);
    let log = Log::default();
    let (mut push, events) = Push::new(runtime.handle().clone(), fakes(log.clone(), t0, script));
    let mut wakes = Wakes::new(events);
    let all = accounts(&["work", "home"]);
    let mut reports = Reports::default();
    let mut p = printer();
    let (mut syncs, mut notified) = (Vec::new(), Vec::new());
    let interval = Duration::from_secs(300);
    let wall = wall_clock(t0, jump);
    run_rounds(&runtime, interval, &stop, wall, &mut wakes, |wake| {
        let secs = (now(&runtime) - t0).as_secs();
        let new_mail = on_wake(
            wake,
            &all,
            unlocked,
            &stop,
            &mut push,
            &mut reports,
            &mut p,
            |a| {
                syncs.push((secs, a.name.clone()));
                sync(&runtime, secs, a, &log)
            },
        )
        .unwrap();
        let mut notify = |n: &Notification| {
            notified.push((secs, n.summary.clone()));
            Ok(())
        };
        notify_round(
            &new_mail,
            &stop,
            |_| Ok(Vec::new()),
            &mut notify,
            &mut reports,
            &mut p,
        )
        .unwrap();
        if secs >= until {
            stop.request();
        }
    });
    runtime.block_on(tokio::task::yield_now());
    let alive = log.alive();
    push.stop_all();
    runtime.block_on(tokio::task::yield_now());
    assert_eq!(log.alive(), 0, "a stop ends every watcher");
    Run {
        syncs,
        notified,
        err: String::from_utf8(p.err).unwrap(),
        log,
        alive,
    }
}

fn no_jump(_: Duration) -> Duration {
    Duration::ZERO
}

/// "home"'s server has no IDLE.
fn no_idle_at_home(account: &str, script: Vec<Step>) -> Vec<Step> {
    if account == "home" {
        vec![Send(Kind::NoIdle), Send(Kind::Ended)]
    } else {
        script
    }
}

#[test]
fn an_account_wake_syncs_only_that_account_and_notifies_without_moving_the_round() {
    let run = daemon(
        300,
        no_jump,
        |account, _| {
            // The first IDLE wakes the account at 0 s, then three changes come within a second.
            let changes = [Wait(100), Send(Kind::Wake), Send(Kind::Wake), Wait(1)];
            let mut script = vec![Send(Kind::On), Send(Kind::Wake)];
            script.extend(changes.into_iter().chain([Send(Kind::Wake), Idle]));
            no_idle_at_home(account, script)
        },
        |_, secs, a, _| synced(a, if secs == 102 { &[7] } else { &[] }),
    );
    // Rounds at 0 and 300, untouched; the IDLE start's wake at 2, the three changes' at 102.
    assert_eq!(run.syncs("work"), [0, 2, 102, 300]);
    assert_eq!(run.syncs("home"), [0, 300]);
    assert_eq!(run.notified, [(102, "work: 1 new message".to_owned())]);
    // "home" is not asked again in the second round: its server has no IDLE.
    assert_eq!(run.log.starts("work"), [0]);
    assert_eq!(run.log.starts("home"), [0]);
    assert_eq!(run.alive, 1);
    assert_eq!(
        run.err,
        "12:00:00 neomail daemon: work: push on\n\
         12:00:00 neomail daemon: home: push off (the server has no IDLE)\n"
    );
}

#[test]
fn wakes_during_an_accounts_sync_make_one_more_sync_after_it() {
    let run = daemon(
        300,
        no_jump,
        |account, _| no_idle_at_home(account, vec![Send(Kind::On), Send(Kind::Wake), Idle]),
        |runtime, secs, a, log| {
            if secs == 2 {
                // Two changes while the sync runs for 5 s.
                let events = log.events.lock().unwrap()["work"].clone();
                events.send(Kind::Wake);
                events.send(Kind::Wake);
                runtime.block_on(async { tokio::time::sleep(Duration::from_secs(5)).await });
            }
            synced(a, &[])
        },
    );
    assert_eq!(run.syncs("work"), [0, 2, 9, 300]);
    assert_eq!(run.syncs("home"), [0, 300]);
}

/// A server that drops every IDLE connection `after` seconds: each end wakes the account once,
/// and that sync starts a new watcher only a minute or more after the last start. A sync too soon
/// for one is followed by another once the minute is up, which starts it.
#[test]
fn a_watcher_starts_at_most_once_a_minute_and_its_end_wakes_its_account() {
    let drops = |after: u64| {
        move |account: &str, _| {
            let script = [Send(Kind::On), Send(Kind::Wake), Wait(after)];
            let script = script
                .into_iter()
                .chain([Send(Kind::Wake), Send(Kind::Ended)]);
            no_idle_at_home(account, script.collect())
        }
    };
    let quick = daemon(122, no_jump, drops(10), |_, _, a, _| synced(a, &[]));
    // 2, 62 and 122: the IDLE start; 12 and 72: the drop, too soon for another watcher; 60 and
    // 120: the minute is up, and that sync starts one.
    assert_eq!(quick.syncs("work"), [0, 2, 12, 60, 62, 72, 120, 122]);
    assert_eq!(quick.log.starts("work"), [0, 60, 120]);
    assert_eq!(quick.syncs("home"), [0]);

    let slow = daemon(144, no_jump, drops(70), |_, _, a, _| synced(a, &[]));
    assert_eq!(slow.syncs("work"), [0, 2, 72, 74, 144]);
    assert_eq!(slow.log.starts("work"), [0, 72, 144]);
}

#[test]
fn push_failing_is_said_once_per_distinct_error_and_push_on_after_it() {
    let run = daemon(
        901,
        no_jump,
        |account, n| {
            let script = match n {
                0 | 1 => vec![Send(Kind::Failed("refused".to_owned())), Send(Kind::Ended)],
                2 => vec![
                    Send(Kind::Failed("timed out".to_owned())),
                    Send(Kind::Ended),
                ],
                _ => vec![Send(Kind::On), Idle],
            };
            no_idle_at_home(account, script)
        },
        |_, _, a, _| synced(a, &[]),
    );
    // A watcher that never idled does not wake its account: the rounds retry it.
    assert_eq!(run.syncs("work"), [0, 300, 600, 900, 1200]);
    let work: Vec<_> = run.err.lines().filter(|l| l.contains(" work: ")).collect();
    assert_eq!(
        work,
        [
            "12:00:00 neomail: work: push failing: refused",
            "12:00:00 neomail: work: push failing: timed out",
            "12:00:00 neomail daemon: work: push on",
        ]
    );
}

/// The machine sleeps for two hours at 20 s; the wall clock shows it at 30 s, and that round
/// replaces the watcher at once, within a minute of the last start.
#[test]
fn a_resumed_round_replaces_every_watcher() {
    let suspended = |elapsed: Duration| {
        if elapsed >= Duration::from_secs(20) {
            Duration::from_secs(2 * 3600)
        } else {
            Duration::ZERO
        }
    };
    let run = daemon(
        30,
        suspended,
        |account, _| no_idle_at_home(account, vec![Send(Kind::On), Idle]),
        |_, _, a, _| synced(a, &[]),
    );
    assert_eq!(run.syncs("work"), [0, 30]);
    assert_eq!(run.log.starts("work"), [0, 30]);
    assert_eq!(run.alive, 1, "the replaced watcher ended");
}
