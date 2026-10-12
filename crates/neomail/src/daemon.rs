//! `neomail daemon`: syncs every account on an interval until SIGINT or SIGTERM, so the cache
//! stays current without `neomail sync`.
//!
//! A round syncs every account of the config, read again before each round, through the same
//! [`Syncer`] as `neomail sync`. What starts a round is a [`Wake`]: the timer for now. Push (IMAP
//! IDLE, Graph delta, Gmail history) will be another kind of wake that names one account and
//! feeds the same `Syncer` under the same account lock.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use nm_core::SyncReport;
use tokio::runtime::Runtime;
use tokio::time::Instant;

use crate::config::{self, Account, Config};
use crate::sync::{PASSWORD_COMMAND_TIMEOUT, Synced, Syncer, report_line};
use crate::{lock, output};

/// The default interval between the starts of two rounds.
pub const DEFAULT_INTERVAL: &str = "5m";
/// The shortest interval: providers throttle clients that poll more often.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);
/// The longest interval, which keeps every deadline far from overflowing.
pub const MAX_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// `--interval`: digits followed by `s`, `m` or `h`, from 30s to 24h.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    count: u64,
    unit: Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Second,
    Minute,
    Hour,
}

impl Interval {
    pub fn duration(self) -> Duration {
        let seconds = match self.unit {
            Unit::Second => 1,
            Unit::Minute => 60,
            Unit::Hour => 3600,
        };
        // Bounded by parse_interval.
        Duration::from_secs(self.count.saturating_mul(seconds))
    }
}

/// "5 minutes", in the unit it was given in.
impl std::fmt::Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unit = match self.unit {
            Unit::Second => "second",
            Unit::Minute => "minute",
            Unit::Hour => "hour",
        };
        let s = if self.count == 1 { "" } else { "s" };
        write!(f, "{} {unit}{s}", self.count)
    }
}

pub fn parse_interval(s: &str) -> Result<Interval, String> {
    const EXPECTED: &str = "expected digits followed by s, m or h, such as 30s, 5m or 1h";
    let (digits, unit) = match s.char_indices().last() {
        Some((i, 's')) => (&s[..i], Unit::Second),
        Some((i, 'm')) => (&s[..i], Unit::Minute),
        Some((i, 'h')) => (&s[..i], Unit::Hour),
        _ => return Err(EXPECTED.to_owned()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EXPECTED.to_owned());
    }
    let interval = Interval {
        // Digits too many for u64 are far above the maximum.
        count: digits.parse().unwrap_or(u64::MAX),
        unit,
    };
    let every = interval.duration();
    if every < MIN_INTERVAL {
        return Err("the shortest interval is 30s".to_owned());
    }
    if every > MAX_INTERVAL {
        return Err("the longest interval is 24h".to_owned());
    }
    Ok(interval)
}

/// Why the daemon woke up.
#[derive(Debug, PartialEq, Eq)]
pub enum Wake {
    /// The next round is due.
    Timer,
    /// SIGINT or SIGTERM: stop after the account being synced.
    Stop,
}

/// The stop request: set by the first SIGINT or SIGTERM, which also writes a byte to a socket so
/// a wait ends at once. A second signal takes the default action and ends the process.
pub struct Stop {
    requested: Arc<AtomicBool>,
    wake: std::os::unix::net::UnixStream,
    poke: std::os::unix::net::UnixStream,
}

impl Stop {
    pub fn new() -> io::Result<Self> {
        let (wake, poke) = std::os::unix::net::UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        poke.set_nonblocking(true)?;
        Ok(Self {
            requested: Arc::new(AtomicBool::new(false)),
            wake,
            poke,
        })
    }

    /// A stop that SIGINT and SIGTERM request. A signal that was ignored when neomail started
    /// (`nohup`, or a background job without job control for SIGINT) stays ignored.
    pub fn on_signals() -> Result<Self> {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let stop = Self::new().context("cannot set up the signal handlers")?;
        for signal in [SIGINT, SIGTERM] {
            if crate::pager::ignored_now(signal) {
                continue;
            }
            // Handlers run in the order they were registered. The default action comes first, so
            // it applies only once a stop was already requested. The flag comes before the byte,
            // so a wait woken by the byte always sees the stop; the other way round, it could
            // drain the byte, find no stop and sleep until the next round.
            signal_hook::flag::register_conditional_default(signal, Arc::clone(&stop.requested))
                .and_then(|_| signal_hook::flag::register(signal, Arc::clone(&stop.requested)))
                .and_then(|_| {
                    signal_hook::low_level::pipe::register(signal, stop.poke.try_clone()?)
                })
                .context("cannot set up the signal handlers")?;
        }
        Ok(stop)
    }

    /// What a signal does, for the tests.
    #[cfg(test)]
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        let _ = (&self.poke).write(&[1]);
    }

    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Waits until `due` has passed or a stop is requested, whichever comes first. `wall` is the
    /// wall clock, read inside the runtime.
    pub async fn wait_until(&self, due: Due, wall: &impl Fn() -> SystemTime) -> Wake {
        let wake = self
            .wake
            .try_clone()
            .and_then(tokio::net::UnixStream::from_std);
        loop {
            if self.requested() {
                return Wake::Stop;
            }
            let now = Instant::now();
            if due.passed(now, wall()) {
                return Wake::Timer;
            }
            // The runtime's clock stops while the machine sleeps; look at the wall clock too.
            let until = due.at.min(now + WALL_CLOCK_CHECK);
            let Ok(wake) = &wake else {
                // Without the socket a signal still sets the flag; look at it every second.
                tokio::time::sleep_until(until.min(now + Duration::from_secs(1))).await;
                continue;
            };
            tokio::select! {
                () = tokio::time::sleep_until(until) => {}
                ready = wake.readable() => {
                    if ready.is_ok() {
                        let mut drain = [0; 16];
                        while matches!(wake.try_read(&mut drain), Ok(n) if n > 0) {}
                    }
                }
            }
        }
    }
}

/// How often a wait looks at the wall clock.
const WALL_CLOCK_CHECK: Duration = Duration::from_secs(30);

/// When the next round is due, on two clocks. The runtime's [`Instant`] is `CLOCK_MONOTONIC` on
/// Linux, which stops while the machine is suspended: alone, it would make a resumed daemon wait
/// out the rest of the interval (up to 24 hours) before syncing. The wall clock keeps counting,
/// so the round is due once either clock says so. A wall clock set back before the round
/// started counts as due too, so a clock change never delays a round.
#[derive(Debug, Clone, Copy)]
pub struct Due {
    at: Instant,
    wall_at: SystemTime,
    wall_from: SystemTime,
}

impl Due {
    /// Due at once.
    fn now(at: Instant, wall: SystemTime) -> Self {
        Self {
            at,
            wall_at: wall,
            wall_from: wall,
        }
    }

    fn passed(&self, now: Instant, wall: SystemTime) -> bool {
        now >= self.at || wall >= self.wall_at || wall < self.wall_from
    }
}

/// The time on the runtime's clock (paused in the tests). Read inside the runtime, so the code
/// between rounds never runs inside it: keyring calls panic there.
fn now(runtime: &Runtime) -> Instant {
    runtime.block_on(async { Instant::now() })
}

/// When the next round starts: one interval after this one `started` (on each clock), or at
/// once if this one ran longer than that. Missed rounds are not made up.
pub fn next_round(started: (Instant, SystemTime), interval: Duration, now: Instant) -> Due {
    let (at, wall) = started;
    Due {
        at: (at + interval).max(now),
        // Bounded by MAX_INTERVAL; a wall clock that far out is due at once.
        wall_at: wall.checked_add(interval).unwrap_or(wall),
        wall_from: wall,
    }
}

/// Runs `round` now and then every `interval` until `stop` is requested. `round` runs outside
/// the runtime; `wall` (the wall clock) inside it.
pub fn run_rounds(
    runtime: &Runtime,
    interval: Duration,
    stop: &Stop,
    wall: impl Fn() -> SystemTime,
    mut round: impl FnMut(),
) {
    let clocks = || runtime.block_on(async { (Instant::now(), wall()) });
    let (at, wall_now) = clocks();
    let mut due = Due::now(at, wall_now);
    while runtime.block_on(stop.wait_until(due, &wall)) == Wake::Timer {
        let started = clocks();
        round();
        if stop.requested() {
            break;
        }
        due = next_round(started, interval, now(runtime));
    }
}

/// The repeated-error rule: an account's failure is reported when it differs from the last one,
/// and its next success is reported even with nothing changed. A warning is reported when it
/// differs from the account's last warning. A success is reported when it is the account's first
/// since the daemon started (so the log shows it working) or when something changed; messages
/// still without a body count only when their number changed, since they are retried every round.
#[derive(Default)]
pub struct Reports {
    last_error: HashMap<String, String>,
    last_warning: HashMap<String, String>,
    /// The last success's messages without a body; no entry before the first success.
    last_missing: HashMap<String, usize>,
}

/// One line to print, without the time.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// To standard output.
    Report(String),
    /// To standard error.
    Error(String),
}

impl Reports {
    pub fn line(&mut self, account: &str, result: &Result<SyncReport>) -> Option<Line> {
        match result {
            Ok(report) => {
                let recovered = self.last_error.remove(account).is_some();
                // None, the first success, differs from every count.
                let missing = self.last_missing.insert(account.to_owned(), report.missing);
                let changed = changed(report) || missing != Some(report.missing);
                (recovered || changed).then(|| Line::Report(report_line(account, report)))
            }
            Err(e) => {
                let message = format!("{e:#}");
                if self.last_error.get(account) == Some(&message) {
                    return None;
                }
                self.last_error.insert(account.to_owned(), message.clone());
                Some(Line::Error(format!("neomail: {account}: {message}")))
            }
        }
    }

    /// A warning that did not fail the account's sync, unless it repeats the last one.
    pub fn warning(&mut self, account: &str, warning: &str) -> Option<Line> {
        if self
            .last_warning
            .get(account)
            .is_some_and(|last| last == warning)
        {
            return None;
        }
        self.last_warning
            .insert(account.to_owned(), warning.to_owned());
        Some(Line::Error(format!("neomail: {account}: {warning}")))
    }

    /// Forgets the accounts that are no longer configured.
    pub fn retain(&mut self, accounts: &[Account]) {
        let configured = |name: &String| accounts.iter().any(|a| &a.name == name);
        self.last_error.retain(|name, _| configured(name));
        self.last_warning.retain(|name, _| configured(name));
        self.last_missing.retain(|name, _| configured(name));
    }
}

/// A change other than the messages without a body.
fn changed(r: &SyncReport) -> bool {
    r.new > 0 || r.updated > 0 || r.removed > 0 || r.reset
}

/// Where a round prints: standard output and error, each line after the local time.
pub struct Printer<O, E> {
    pub out: O,
    pub err: E,
    pub clock: fn() -> String,
}

impl<O: Write, E: Write> Printer<O, E> {
    fn print(&mut self, line: &Line) -> io::Result<()> {
        let time = (self.clock)();
        match line {
            Line::Report(text) => {
                writeln!(self.out, "{time} {text}")?;
                self.out.flush()
            }
            Line::Error(text) => {
                // Standard error is not worth stopping the daemon over.
                let _ = writeln!(self.err, "{time} {text}");
                let _ = self.err.flush();
                Ok(())
            }
        }
    }
}

/// One round: each account in turn, skipping one whose lock another sync holds, stopping early
/// once a stop is requested. One account's failure never stops the others.
pub fn round<O: Write, E: Write>(
    accounts: &[Account],
    data_dir: &Path,
    stop: &Stop,
    reports: &mut Reports,
    printer: &mut Printer<O, E>,
    mut sync: impl FnMut(&Account) -> Synced,
) -> io::Result<()> {
    reports.retain(accounts);
    for account in accounts {
        if stop.requested() {
            break;
        }
        let synced = match lock::try_account(data_dir, &account.name) {
            // `neomail sync` has it: it reports, and the next round catches up.
            Ok(None) => continue,
            Ok(Some(_held)) => sync(account),
            Err(e) => Synced {
                report: Err(e),
                warnings: Vec::new(),
            },
        };
        for warning in &synced.warnings {
            if let Some(line) = reports.warning(&account.name, warning) {
                printer.print(&line)?;
            }
        }
        if let Some(line) = reports.line(&account.name, &synced.report) {
            printer.print(&line)?;
        }
    }
    Ok(())
}

/// The config of the next round: the file read again, or the last good one when it cannot be
/// read, with the reason reported once per distinct error.
fn reload(path: &Path, last: &mut Config, reported: &mut Option<String>) -> Option<Line> {
    match Config::load(path) {
        Ok(config) => {
            *last = config;
            *reported = None;
            None
        }
        Err(e) => {
            let message = format!("{e:#}");
            if reported.as_ref() == Some(&message) {
                return None;
            }
            *reported = Some(message.clone());
            Some(Line::Error(format!(
                "neomail: {message}; keeping the last good config"
            )))
        }
    }
}

fn local_time() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// `neomail daemon`.
pub fn run(config_path: PathBuf, interval: Interval) -> Result<ExitCode> {
    let config = match Config::load(&config_path) {
        Ok(config) if !config.accounts.is_empty() => config,
        // No file or no accounts: the start lines, like bare `neomail`.
        Ok(_) => return Ok(no_accounts()),
        Err(_) if !config::exists(&config_path) => return Ok(no_accounts()),
        Err(e) => return Err(crate::usage(format!("{e:#}"))),
    };
    let data_dir = config::data_dir()?;
    crate::create_private_dir(&data_dir)?;
    let _daemon = lock::daemon(&data_dir)?;
    let mut cache = crate::open_cache(true)?.context("the cache could not be created")?;
    let stop = Stop::on_signals()?;
    let runtime = crate::account::runtime()?;
    let count = config.accounts.len();
    eprintln!(
        "neomail daemon: syncing {count} account{} every {interval}; Ctrl-C stops",
        if count == 1 { "" } else { "s" }
    );
    let mut syncer = Syncer::new(
        &runtime,
        nm_core::sync::DEFAULT_LIMIT,
        output::ProgressLine::new(false),
    )
    .unattended(PASSWORD_COMMAND_TIMEOUT);
    let mut config = config;
    let mut config_error = None;
    let mut reports = Reports::default();
    let mut printer = Printer {
        out: io::stdout(),
        err: io::stderr(),
        clock: local_time,
    };
    let mut failed: io::Result<()> = Ok(());
    run_rounds(
        &runtime,
        interval.duration(),
        &stop,
        SystemTime::now,
        || {
            let mut printed = Ok(());
            if let Some(line) = reload(&config_path, &mut config, &mut config_error) {
                printed = printer.print(&line);
            }
            // Next to the reports' retain in `round`: a removed account's tokens go too.
            syncer.retain(&config.accounts);
            let printed = printed.and_then(|()| {
                round(
                    &config.accounts,
                    &data_dir,
                    &stop,
                    &mut reports,
                    &mut printer,
                    |account| syncer.sync(account, &mut cache),
                )
            });
            if let Err(e) = printed {
                // Standard output is gone (a closed pipe): nothing more can be reported.
                failed = Err(e);
                stop.requested.store(true, Ordering::SeqCst);
            }
        },
    );
    failed?;
    Ok(ExitCode::SUCCESS)
}

fn no_accounts() -> ExitCode {
    eprint!("{}", crate::start_text());
    ExitCode::from(crate::EXIT_USAGE)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use anyhow::bail;

    use super::*;

    #[test]
    fn interval_is_digits_and_a_unit_from_30s_to_24h() {
        let ok = |s: &str| {
            let i = parse_interval(s).unwrap();
            (i.duration().as_secs(), i.to_string())
        };
        assert_eq!(ok("30s"), (30, "30 seconds".to_owned()));
        assert_eq!(ok("5m"), (300, "5 minutes".to_owned()));
        assert_eq!(ok("1m"), (60, "1 minute".to_owned()));
        assert_eq!(ok("1h"), (3600, "1 hour".to_owned()));
        assert_eq!(ok("24h"), (86400, "24 hours".to_owned()));
        assert_eq!(ok("0090s"), (90, "90 seconds".to_owned()));
        assert_eq!(ok(DEFAULT_INTERVAL), (300, "5 minutes".to_owned()));
        for bad in [
            "",
            "5",
            "m",
            "5x",
            "-5m",
            "+5m",
            "5 m",
            " 5m",
            "5M",
            "1.5h",
            "5mm",
            "٥m",
            "29s",
            "0m",
            "25h",
            "1441m",
            "99999999999999999999999h",
        ] {
            assert!(parse_interval(bad).is_err(), "{bad:?}");
        }
    }

    fn paused_runtime() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
    }

    /// A wall clock that keeps time with the paused runtime clock from `t0`, plus what `jump`
    /// adds after `elapsed` on the runtime clock. Read inside the runtime only.
    fn wall_clock(t0: Instant, jump: impl Fn(Duration) -> Duration) -> impl Fn() -> SystemTime {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        move || {
            let elapsed = Instant::now() - t0;
            base + elapsed + jump(elapsed)
        }
    }

    #[test]
    fn rounds_start_at_once_then_every_interval_and_never_pile_up() {
        let runtime = paused_runtime();
        let stop = Stop::new().unwrap();
        let t0 = now(&runtime);
        let interval = Duration::from_secs(300);
        // How long each round takes, in seconds: the second runs past two intervals.
        let lengths = [10, 700, 20, 0];
        let starts = RefCell::new(Vec::new());
        let wall = wall_clock(t0, |_| Duration::ZERO);
        run_rounds(&runtime, interval, &stop, wall, || {
            let mut starts = starts.borrow_mut();
            starts.push((now(&runtime) - t0).as_secs());
            let length = Duration::from_secs(lengths[starts.len() - 1]);
            runtime.block_on(async { tokio::time::sleep(length).await });
            if starts.len() == lengths.len() {
                stop.request();
            }
        });
        // 0, 300, then 1000 when the long round ends (not 600 and 900 as well), then 1300.
        assert_eq!(*starts.borrow(), [0, 300, 1000, 1300]);
    }

    /// The runtime's clock stops while the machine is suspended; the wall clock does not. Here
    /// the machine sleeps for two hours at 45 s: after the resume (still 45 s on the runtime
    /// clock), the next look at the wall clock, at 60 s, starts the overdue round.
    #[test]
    fn a_round_starts_soon_after_a_resume_rather_than_at_the_end_of_the_interval() {
        let runtime = paused_runtime();
        let stop = Stop::new().unwrap();
        let t0 = now(&runtime);
        let suspended = |elapsed: Duration| {
            if elapsed >= Duration::from_secs(45) {
                Duration::from_secs(2 * 3600)
            } else {
                Duration::ZERO
            }
        };
        let starts = RefCell::new(Vec::new());
        let interval = Duration::from_secs(3600);
        run_rounds(&runtime, interval, &stop, wall_clock(t0, suspended), || {
            let mut starts = starts.borrow_mut();
            starts.push((now(&runtime) - t0).as_secs());
            if starts.len() == 2 {
                stop.request();
            }
        });
        assert_eq!(*starts.borrow(), [0, 60]);
    }

    #[test]
    fn a_round_is_due_on_either_clock_or_when_the_wall_clock_goes_back() {
        let at = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let interval = Duration::from_secs(300);
        let due = next_round((at, wall), interval, at);
        let after = |s| Duration::from_secs(s);
        assert!(!due.passed(at, wall));
        assert!(!due.passed(at + after(299), wall + after(299)));
        assert!(due.passed(at + after(300), wall), "the runtime clock");
        assert!(due.passed(at, wall + after(300)), "the wall clock");
        assert!(due.passed(at, wall - after(1)), "the wall clock set back");
        // A round that ran past the interval: the next one is due at once, on both clocks.
        let late = next_round((at, wall), interval, at + after(700));
        assert!(late.passed(at + after(700), wall + after(700)));
    }

    #[test]
    fn a_stop_during_a_round_ends_the_daemon_after_it() {
        let runtime = paused_runtime();
        let stop = Stop::new().unwrap();
        let mut rounds = 0;
        run_rounds(
            &runtime,
            Duration::from_secs(300),
            &stop,
            SystemTime::now,
            || {
                rounds += 1;
                stop.request();
            },
        );
        assert_eq!(rounds, 1);

        let stop = Stop::new().unwrap();
        stop.request();
        let mut rounds = 0;
        let interval = Duration::from_secs(300);
        run_rounds(&runtime, interval, &stop, SystemTime::now, || rounds += 1);
        assert_eq!(rounds, 0);
    }

    /// On the real clock, since paused time skips ahead without polling the socket; nothing
    /// waits for the deadline, an hour away.
    #[test]
    fn a_stop_wakes_a_wait_before_its_deadline() {
        let runtime = crate::account::runtime().unwrap();
        let stop = Stop::new().unwrap();
        let started = std::time::Instant::now();
        let woke = runtime.block_on(async {
            let hour = Duration::from_secs(3600);
            let due = next_round((Instant::now(), SystemTime::now()), hour, Instant::now());
            let waiting = stop.wait_until(due, &SystemTime::now);
            let stopping = async {
                tokio::task::yield_now().await;
                stop.request();
            };
            // Fails rather than hangs when the socket never wakes the wait.
            tokio::time::timeout(Duration::from_secs(60), async {
                tokio::join!(waiting, stopping).0
            })
            .await
        });
        assert_eq!(woke, Ok(Wake::Stop));
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    fn accounts(names: &[&str]) -> Vec<Account> {
        let text: String = names
            .iter()
            .map(|n| {
                format!(
                    "[[accounts]]\nname = \"{n}\"\nusername = \"you@example.com\"\n\
                     password_command = \"true\"\nimap = {{ host = \"127.0.0.1\" }}\n"
                )
            })
            .collect();
        Config::parse(&text).unwrap().accounts
    }

    fn printer() -> Printer<Vec<u8>, Vec<u8>> {
        Printer {
            out: Vec::new(),
            err: Vec::new(),
            clock: || "12:00:00".to_owned(),
        }
    }

    /// A sync without warnings.
    fn quiet(
        mut sync: impl FnMut(&Account) -> Result<SyncReport>,
    ) -> impl FnMut(&Account) -> Synced {
        move |a| Synced {
            report: sync(a),
            warnings: Vec::new(),
        }
    }

    fn report(new: usize) -> SyncReport {
        SyncReport {
            new,
            ..SyncReport::default()
        }
    }

    #[test]
    fn one_failing_account_never_stops_the_others_and_errors_are_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Stop::new().unwrap();
        let mut reports = Reports::default();
        let mut p = printer();
        let all = accounts(&["work", "home", "club"]);
        // Per round: what "work" does; "home" fails the same way every round; "club" gets mail
        // in the first round only.
        let work: [Result<SyncReport, &str>; 5] = [
            Err("connection refused"),
            Err("connection refused"),
            Err("timed out"),
            Ok(report(0)),
            Ok(report(0)),
        ];
        let mut synced = Vec::new();
        for (n, work) in work.into_iter().enumerate() {
            round(
                &all,
                dir.path(),
                &stop,
                &mut reports,
                &mut p,
                quiet(|a| {
                    synced.push(a.name.clone());
                    match a.name.as_str() {
                        "work" => work.map_err(|e| anyhow::anyhow!(e)),
                        "home" => bail!("password command failed"),
                        _ => Ok(report(usize::from(n == 0) * 2)),
                    }
                }),
            )
            .unwrap();
        }
        assert_eq!(synced.len(), 15, "every account, every round: {synced:?}");
        assert_eq!(
            String::from_utf8(p.err).unwrap(),
            "12:00:00 neomail: work: connection refused\n\
             12:00:00 neomail: home: password command failed\n\
             12:00:00 neomail: work: timed out\n"
        );
        assert_eq!(
            String::from_utf8(p.out).unwrap(),
            "12:00:00 club: 2 new, 0 updated, 0 removed\n\
             12:00:00 work: 0 new, 0 updated, 0 removed\n",
            "the recovery is reported once, quiet rounds are not"
        );
    }

    #[test]
    fn a_removed_account_s_error_is_forgotten() {
        let mut reports = Reports::default();
        let failed: Result<SyncReport> = Err(anyhow::anyhow!("refused"));
        assert!(reports.line("work", &failed).is_some());
        assert!(reports.line("work", &failed).is_none());
        reports.retain(&accounts(&["home"]));
        assert!(reports.line("work", &failed).is_some());
    }

    fn missing(missing: usize) -> SyncReport {
        SyncReport {
            missing,
            ..SyncReport::default()
        }
    }

    #[test]
    fn the_first_success_is_reported_then_only_changes() {
        let mut reports = Reports::default();
        let mut lines = |r: SyncReport| reports.line("work", &Ok(r)).is_some();
        assert!(lines(report(0)), "the first round shows the daemon working");
        assert!(!lines(report(0)));
        assert!(lines(report(1)));
        assert!(!lines(report(0)));
    }

    /// nm-core retries a message without a body at every sync, so the same count comes back
    /// every round: it is reported when it changes, not every time.
    #[test]
    fn messages_without_a_body_are_reported_when_their_number_changes() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Stop::new().unwrap();
        let mut reports = Reports::default();
        let mut p = printer();
        let all = accounts(&["work"]);
        let mut rounds = |r: SyncReport, p: &mut Printer<Vec<u8>, Vec<u8>>| {
            round(&all, dir.path(), &stop, &mut reports, p, quiet(|_| Ok(r))).unwrap();
            String::from_utf8(std::mem::take(&mut p.out)).unwrap()
        };
        let line =
            "12:00:00 work: 0 new, 0 updated, 0 removed, 1 without a body (retried next sync)\n";
        assert_eq!(rounds(missing(1), &mut p), line);
        assert_eq!(rounds(missing(1), &mut p), "");
        let with_new = SyncReport {
            new: 1,
            ..missing(1)
        };
        assert!(!rounds(with_new, &mut p).is_empty(), "with another change");
        assert_eq!(rounds(missing(1), &mut p), "");
        assert!(!rounds(missing(2), &mut p).is_empty());
        assert!(!rounds(missing(0), &mut p).is_empty());
        assert_eq!(rounds(missing(0), &mut p), "");
    }

    #[test]
    fn a_warning_is_printed_with_the_time_unless_it_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Stop::new().unwrap();
        let mut reports = Reports::default();
        let mut p = printer();
        let all = accounts(&["work"]);
        for warning in ["warning: not saved", "warning: not saved", "warning: other"] {
            round(&all, dir.path(), &stop, &mut reports, &mut p, |_| Synced {
                report: Ok(report(0)),
                warnings: vec![warning.to_owned()],
            })
            .unwrap();
        }
        assert_eq!(
            String::from_utf8(p.err).unwrap(),
            "12:00:00 neomail: work: warning: not saved\n\
             12:00:00 neomail: work: warning: other\n"
        );
    }

    #[test]
    fn a_locked_account_is_skipped_for_the_round() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Stop::new().unwrap();
        let mut reports = Reports::default();
        let mut p = printer();
        let held = lock::try_account(dir.path(), "work").unwrap().unwrap();
        let mut synced = Vec::new();
        let all = accounts(&["work", "home"]);
        round(
            &all,
            dir.path(),
            &stop,
            &mut reports,
            &mut p,
            quiet(|a| {
                assert!(
                    lock::try_account(dir.path(), &a.name).unwrap().is_none(),
                    "held while syncing"
                );
                synced.push(a.name.clone());
                Ok(report(0))
            }),
        )
        .unwrap();
        assert_eq!(synced, ["home"]);
        assert_eq!(
            String::from_utf8(std::mem::take(&mut p.out)).unwrap(),
            "12:00:00 home: 0 new, 0 updated, 0 removed\n",
            "skipping is quiet"
        );
        assert!(p.err.is_empty());
        drop(held);
        round(
            &all,
            dir.path(),
            &stop,
            &mut reports,
            &mut p,
            quiet(|a| {
                synced.push(a.name.clone());
                Ok(report(0))
            }),
        )
        .unwrap();
        assert_eq!(synced, ["home", "work", "home"]);
    }

    #[test]
    fn a_stop_lets_the_account_being_synced_finish_and_skips_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let stop = Stop::new().unwrap();
        let mut p = printer();
        let mut synced = Vec::new();
        round(
            &accounts(&["work", "home"]),
            dir.path(),
            &stop,
            &mut Reports::default(),
            &mut p,
            quiet(|a| {
                stop.request();
                synced.push(a.name.clone());
                Ok(report(1))
            }),
        )
        .unwrap();
        assert_eq!(synced, ["work"]);
        assert_eq!(
            String::from_utf8(p.out).unwrap(),
            "12:00:00 work: 1 new, 0 updated, 0 removed\n"
        );
    }

    #[test]
    fn an_invalid_config_is_reported_once_and_the_last_good_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let good = "[[accounts]]\nname = \"work\"\nusername = \"you@example.com\"\n\
                    password_command = \"true\"\nimap = { host = \"127.0.0.1\" }\n";
        std::fs::write(&path, good).unwrap();
        let mut config = Config::load(&path).unwrap();
        let mut reported = None;
        std::fs::write(&path, "[[accounts]\n").unwrap();
        let line = reload(&path, &mut config, &mut reported).unwrap();
        let Line::Error(text) = line else {
            panic!("{line:?}")
        };
        assert!(text.ends_with("; keeping the last good config"), "{text}");
        assert!(reload(&path, &mut config, &mut reported).is_none());
        assert_eq!(config.accounts.len(), 1);
        // Added accounts take effect at the next round.
        std::fs::write(&path, format!("{good}{}", good.replace("work", "home"))).unwrap();
        assert!(reload(&path, &mut config, &mut reported).is_none());
        assert_eq!(config.accounts.len(), 2);
        std::fs::remove_file(&path).unwrap();
        assert!(reload(&path, &mut config, &mut reported).is_some());
        assert_eq!(config.accounts.len(), 2);
    }
}
