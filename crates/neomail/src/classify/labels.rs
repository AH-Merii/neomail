//! Groups as Outlook categories (DEC-2026-10-13-categories): after `neomail classify` stores its
//! results, and after each daemon wake classifies, every classified Outlook message whose
//! effective group differs from the category neomail last wrote gets the new one. What is still
//! to write comes from the cache ([`Cache::labels_due`]), so the backlog from before 10c, failed
//! writes and groups that change with age or a new threshold need no queue.
//!
//! [`write_labels`] owns the cap, the lock and the per-account error rules, so the CLI and the
//! daemon share them; [`CategoryWriter`] is the seam to Graph ([`GraphWriter`]; a fake in the
//! tests). IMAP accounts are left alone: Gmail and Proton labels are slice 10d's. Nothing printed
//! names a subject, a sender or a user's category.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use nm_core::{Cache, LabelPrefix, MessageId, RemoteId};
use nm_graph::{CONCURRENT_FETCHES, CategoryEdit, CategoryResult, GraphError};
use tokio::runtime::Runtime;

use crate::config::{Account, ClassifySettings, Config, Server};
use crate::graph;
use crate::lock;
use crate::style::Colors;
use crate::sync::{Credential, Logins};

/// The most categories one `neomail classify` run writes per account: its message cap.
pub const PER_RUN: usize = super::DEFAULT_LIMIT as usize;
/// The most one daemon wake writes per account: its request cap (DEC-2026-10-13-daemon-classify).
pub const PER_WAKE: usize = super::daemon::PER_WAKE;

/// Writes categories to an account's server: [`GraphWriter`], or a fake in the tests.
pub(crate) trait CategoryWriter {
    type Lock;
    /// The account's lock, as every write takes it; `None` when someone else holds it and the
    /// account is skipped this time.
    fn lock(&mut self, account: &str) -> Result<Option<Self::Lock>>;
    /// Whether the account's sign-in lets neomail change mail (Mail.ReadWrite). Signs in.
    fn may_write(&mut self, account: &Account) -> Result<bool>;
    /// Sets the categories of `edits`, at most [`CONCURRENT_FETCHES`] of them. At `deadline`
    /// the writes still in flight are dropped, and their messages get no answer.
    fn write(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        edits: &[CategoryEdit],
        deadline: Option<Instant>,
    ) -> Result<Vec<(RemoteId, CategoryResult)>>;
    /// The sign-in's warnings since the last call, as (account, warning).
    fn warnings(&mut self) -> Vec<(String, String)>;
}

/// Why an account wrote nothing more.
#[derive(Debug)]
pub(crate) enum Off {
    /// The sign-in grants Mail.Read only: no request was sent.
    ReadOnly,
    /// Another neomail holds the account's lock.
    Locked,
    /// The login, the cache, Graph refusing the token or the grant (401, 403), or Graph
    /// throttling after its retries or out of reach: the rest wait for next time.
    Failed(anyhow::Error),
}

/// One account's pass.
#[derive(Debug, Default)]
pub(crate) struct AccountOutcome {
    pub account: String,
    pub written: usize,
    /// Messages whose write failed: still due, so tried again next time.
    pub failed: usize,
    /// Messages left for next time: past the cap, or not reached.
    pub waiting: usize,
    pub off: Option<Off>,
}

/// What a pass writes: the settings' prefix and rules, at most `cap` per account, only the
/// messages `ids` when given. `stop` ends the pass once the writes in flight are done; at
/// `deadline`, as `clock` tells it, no write starts and the ones in flight are dropped, so the
/// daemon's next round starts on time.
pub(crate) struct Pass<'a> {
    pub settings: &'a ClassifySettings,
    pub ids: Option<&'a [MessageId]>,
    pub cap: usize,
    pub stop: &'a AtomicBool,
    pub deadline: Option<Instant>,
    pub clock: &'a dyn Fn() -> Instant,
    pub now: DateTime<Utc>,
}

impl<'a> Pass<'a> {
    /// `neomail classify`'s pass: at most [`PER_RUN`] per account, no deadline.
    pub(crate) fn run(
        settings: &'a ClassifySettings,
        ids: Option<&'a [MessageId]>,
        stop: &'a AtomicBool,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            settings,
            ids,
            cap: PER_RUN,
            stop,
            deadline: None,
            clock: &Instant::now,
            now,
        }
    }

    /// A daemon wake's pass: at most [`PER_WAKE`] per account, ending `budget` from now.
    pub(crate) fn wake(
        settings: &'a ClassifySettings,
        stop: &'a AtomicBool,
        budget: Duration,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            settings,
            ids: None,
            cap: PER_WAKE,
            stop,
            deadline: Some(Instant::now() + budget),
            clock: &Instant::now,
            now,
        }
    }

    fn over(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.deadline.is_some_and(|d| (self.clock)() >= d)
    }
}

/// Whether a write's error ends the account's pass: the token or the grant is wrong for every
/// message (401, 403), or Graph still throttles after its retries or cannot be reached, and every
/// next request would wait as long again.
fn ends_pass(e: &GraphError) -> bool {
    matches!(
        e,
        GraphError::Unauthorized
            | GraphError::Forbidden
            | GraphError::Throttled { .. }
            | GraphError::Transport(_)
    )
}

/// Writes the due categories of each Outlook account in `accounts`. With `labels = false` it
/// writes nothing. An account with nothing due is not signed in to. One account's failure never
/// stops the others, and nothing here fails the caller: classifying, syncing and notifying go on.
pub(crate) fn write_labels<W: CategoryWriter>(
    cache: &mut Cache,
    accounts: &[&Account],
    pass: &Pass<'_>,
    writer: &mut W,
) -> Vec<AccountOutcome> {
    if !pass.settings.labels {
        return Vec::new();
    }
    let mut outcomes = Vec::new();
    for account in accounts {
        if !matches!(account.server, Server::Graph { .. }) {
            continue;
        }
        let mut outcome = AccountOutcome {
            account: account.name.clone(),
            ..AccountOutcome::default()
        };
        if let Err(e) = write_account(cache, account, pass, writer, &mut outcome) {
            outcome.off = Some(Off::Failed(e));
        }
        if outcome.written + outcome.failed + outcome.waiting > 0 || outcome.off.is_some() {
            outcomes.push(outcome);
        }
    }
    outcomes
}

fn write_account<W: CategoryWriter>(
    cache: &mut Cache,
    account: &Account,
    pass: &Pass<'_>,
    writer: &mut W,
    outcome: &mut AccountOutcome,
) -> Result<()> {
    let settings = pass.settings;
    let (due, more) = cache.labels_due(
        Some(&account.name),
        pass.ids,
        &settings.rules,
        &settings.label_prefix,
        pass.now,
        pass.cap,
    )?;
    if due.is_empty() {
        return Ok(());
    }
    // Until a write is tried, everything due waits.
    outcome.waiting = due.len() + more;
    if pass.over() {
        return Ok(());
    }
    let Some(_held) = writer.lock(&account.name)? else {
        outcome.off = Some(Off::Locked);
        return Ok(());
    };
    if !writer.may_write(account)? {
        outcome.off = Some(Off::ReadOnly);
        return Ok(());
    }
    for chunk in due.chunks(CONCURRENT_FETCHES) {
        if pass.over() {
            break;
        }
        let edits: Vec<CategoryEdit> = chunk
            .iter()
            .map(|d| CategoryEdit {
                id: d.remote_id.clone(),
                want: d.want.clone(),
                last: d.last.clone(),
            })
            .collect();
        let results = writer.write(account, &settings.label_prefix, &edits, pass.deadline)?;
        outcome.waiting -= chunk.len();
        let mut ended: Option<GraphError> = None;
        let mut handled = 0;
        for (id, result) in results {
            let Some(d) = chunk.iter().find(|d| d.remote_id == id) else {
                continue;
            };
            match result {
                CategoryResult::Set | CategoryResult::Already => {
                    handled += 1;
                    cache.store_label(&account.name, &d.remote_id, &d.want, pass.now)?;
                    if matches!(result, CategoryResult::Set) {
                        outcome.written += 1;
                    }
                }
                // The next sync drops it from the cache.
                CategoryResult::Gone => handled += 1,
                // A refusal says more than throttling: it needs a new sign-in.
                CategoryResult::Failed(e) if ends_pass(&e) => {
                    let refusal = matches!(e, GraphError::Unauthorized | GraphError::Forbidden);
                    if ended.is_none() || refusal {
                        ended = Some(e);
                    }
                }
                CategoryResult::Failed(_) => {
                    handled += 1;
                    outcome.failed += 1;
                }
            }
        }
        // The messages that ended the pass, and the ones that got no answer, wait too.
        outcome.waiting += chunk.len() - handled;
        if let Some(e) = ended {
            // The writer signs in afresh next time, so there is nothing to run again.
            if matches!(e, GraphError::Unauthorized) {
                anyhow::bail!("access token refused; signed in again next time");
            }
            return Err(graph::explain_write(e, &account.name));
        }
    }
    Ok(())
}

/// `neomail classify`'s pass, after its results are stored: the Outlook accounts in its scope
/// (`account`, or every one; only the messages `ids` when given), at most [`PER_RUN`] each,
/// waiting for an account's lock. Prints its lines on standard error and returns whether it
/// failed, for the exit code.
pub(crate) fn cli_pass(
    cache: &mut Cache,
    config: Option<&Config>,
    account: Option<&str>,
    ids: &[MessageId],
    colors: Colors,
) -> Result<bool> {
    let Some(config) = config else {
        return Ok(false);
    };
    let accounts: Vec<&Account> = config
        .accounts
        .iter()
        .filter(|a| matches!(a.server, Server::Graph { .. }))
        .filter(|a| account.is_none_or(|name| a.name == name))
        .collect();
    if accounts.is_empty() || !config.classify.labels {
        return Ok(false);
    }
    let runtime = crate::account::runtime()?;
    let data_dir = crate::config::data_dir()?;
    let mut writer = GraphWriter::waiting(&runtime, &data_dir);
    let stop = AtomicBool::new(false);
    let ids = (!ids.is_empty()).then_some(ids);
    let pass = Pass::run(&config.classify, ids, &stop, Utc::now());
    let outcomes = write_labels(cache, &accounts, &pass, &mut writer);
    for (account, warning) in writer.warnings() {
        eprintln!("neomail: {account}: {warning}");
    }
    for o in &outcomes {
        if let Some(off) = off_line(o) {
            let label = match o.off {
                Some(Off::Failed(_)) => colors.stderr.error("neomail"),
                _ => "neomail".to_owned(),
            };
            eprintln!("{label}: {off}");
        }
        if let Some(line) = summary(o, "run") {
            eprintln!("neomail: {line}");
        }
    }
    Ok(failed(&outcomes))
}

/// The line that counts an account's writes, `None` when no write was answered: nothing was
/// tried (a held lock, a stop, a spent budget), or every one carried its category already. `next`
/// is when the rest is tried: "run" or "wake".
pub(crate) fn summary(o: &AccountOutcome, next: &str) -> Option<String> {
    if o.written + o.failed == 0 {
        return None;
    }
    let mut line = format!(
        "{}: {} categor{} written",
        o.account,
        o.written,
        if o.written == 1 { "y" } else { "ies" }
    );
    if o.failed > 0 {
        line.push_str(&format!(", {} failed (tried again next {next})", o.failed));
    }
    if o.waiting > 0 {
        let verb = if o.waiting == 1 { "waits" } else { "wait" };
        line.push_str(&format!(", {} {verb} for the next {next}", o.waiting));
    }
    Some(line)
}

/// Why the account wrote nothing more, `None` for a lock someone else holds (the next time
/// catches up, as for a sync).
pub(crate) fn off_line(o: &AccountOutcome) -> Option<String> {
    let account = &o.account;
    match o.off.as_ref()? {
        Off::ReadOnly => Some(format!(
            "{account}: groups are not written to Outlook: run `neomail account login {account}` \
             to let neomail change mail"
        )),
        Off::Locked => None,
        Off::Failed(e) => Some(format!("{account}: categories not written: {e:#}")),
    }
}

/// Whether the pass failed for `neomail classify`'s exit code: a write or an account failed. A
/// Mail.Read sign-in is a warning, not a failure.
pub(crate) fn failed(outcomes: &[AccountOutcome]) -> bool {
    outcomes
        .iter()
        .any(|o| o.failed > 0 || matches!(o.off, Some(Off::Failed(_))))
}

/// The daemon's lines about categories, each said once per state as the sync's are: the counts
/// when the wake wrote some or the failed and waiting counts changed; why an account is off when
/// that differs from what was last said, and again only after the account wrote without it; the
/// sign-in's warnings when they differ from the account's last one. A wake that tried nothing
/// (a held lock, a stop, a spent budget) changes none of that.
#[derive(Default)]
pub(crate) struct Reports {
    last_off: HashMap<String, String>,
    /// The (failed, waiting) counts last printed, for the accounts that still have some due.
    last_counts: HashMap<String, (usize, usize)>,
    last_warning: HashMap<String, String>,
}

impl Reports {
    pub(crate) fn lines(&mut self, outcomes: &[AccountOutcome]) -> Vec<crate::daemon::Line> {
        use crate::daemon::Line;
        let mut lines = Vec::new();
        // An account with nothing due has everything written: its counts start afresh.
        self.last_counts
            .retain(|account, _| outcomes.iter().any(|o| &o.account == account));
        for o in outcomes {
            let off = off_line(o);
            if let Some(off) = off.as_ref()
                && self.last_off.get(&o.account) != Some(off)
            {
                lines.push(Line::Error(format!("neomail: {off}")));
                self.last_off.insert(o.account.clone(), off.clone());
            }
            let counts = (o.failed, o.waiting);
            let news = o.written > 0 || self.last_counts.get(&o.account) != Some(&counts);
            if let Some(line) = summary(o, "wake").filter(|_| news) {
                lines.push(Line::Report(line));
                self.last_counts.insert(o.account.clone(), counts);
                if off.is_none() {
                    self.last_off.remove(&o.account);
                }
            }
        }
        lines
    }

    /// A warning from an account's sign-in, unless it repeats the account's last one.
    pub(crate) fn warning(&mut self, account: &str, warning: &str) -> Option<crate::daemon::Line> {
        if self.last_warning.get(account).is_some_and(|w| w == warning) {
            return None;
        }
        self.last_warning
            .insert(account.to_owned(), warning.to_owned());
        Some(crate::daemon::Line::Error(format!(
            "neomail: {account}: {warning}"
        )))
    }
}

/// A daemon wake's category step, after it classified: writes what is due within `budget` and
/// returns the lines to print. Each wake that synced runs it, as what is due changes with age.
pub(crate) fn daemon_wake<W: CategoryWriter>(
    cache: &mut Cache,
    accounts: &[&Account],
    pass: &Pass<'_>,
    writer: &mut W,
    reports: &mut Reports,
) -> Vec<crate::daemon::Line> {
    let outcomes = write_labels(cache, accounts, pass, writer);
    let mut lines: Vec<_> = writer
        .warnings()
        .into_iter()
        .filter_map(|(account, warning)| reports.warning(&account, &warning))
        .collect();
    lines.extend(reports.lines(&outcomes));
    lines
}

/// The real writer: Graph through the logins `neomail sync` uses, under the account locks.
/// `wait` waits for a held lock (`neomail classify`); otherwise the account is skipped (the
/// daemon, which must not stall its round). The sign-in's warnings are kept for the caller to
/// print, so the daemon prints them with its time and once per state.
pub(crate) struct GraphWriter<'a> {
    runtime: &'a Runtime,
    data_dir: &'a Path,
    logins: Logins,
    wait: bool,
    warnings: Vec<(String, String)>,
}

impl<'a> GraphWriter<'a> {
    /// `neomail classify`'s: waits for an account's lock.
    pub(crate) fn waiting(runtime: &'a Runtime, data_dir: &'a Path) -> Self {
        Self::new(runtime, data_dir, true)
    }

    /// The daemon's: skips an account whose lock is held, so its round never stalls. Its own
    /// logins: a token is refreshed only while some category is due.
    pub(crate) fn skipping(runtime: &'a Runtime, data_dir: &'a Path) -> Self {
        Self::new(runtime, data_dir, false)
    }

    fn new(runtime: &'a Runtime, data_dir: &'a Path, wait: bool) -> Self {
        Self {
            runtime,
            data_dir,
            logins: Logins::default(),
            wait,
            warnings: Vec::new(),
        }
    }
}

impl CategoryWriter for GraphWriter<'_> {
    type Lock = lock::Held;

    fn lock(&mut self, account: &str) -> Result<Option<lock::Held>> {
        if self.wait {
            lock::wait_for_account(self.data_dir, account, || {
                eprintln!("neomail: waiting for neomail daemon to finish syncing {account}");
            })
            .map(Some)
        } else {
            lock::try_account(self.data_dir, account)
        }
    }

    fn may_write(&mut self, account: &Account) -> Result<bool> {
        let mut warnings = Vec::new();
        let result = match self.logins.credential(self.runtime, account, &mut warnings) {
            Ok(Credential::AccessToken(token)) => Ok(graph::may_write_categories(token)),
            Ok(Credential::Password(_)) => Err(not_oauth()),
            Err(e) => Err(e),
        };
        self.finish(account, warnings, result.is_err());
        result
    }

    fn write(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        edits: &[CategoryEdit],
        deadline: Option<Instant>,
    ) -> Result<Vec<(RemoteId, CategoryResult)>> {
        let runtime = self.runtime;
        let mut warnings = Vec::new();
        let result = match self.logins.credential(runtime, account, &mut warnings) {
            Ok(Credential::AccessToken(token)) => runtime.block_on(async {
                let writes = graph::set_categories(token, prefix, edits);
                match deadline {
                    // A write cut off is tried again next time: it reads the categories afresh.
                    Some(at) => tokio::time::timeout_at(at.into(), writes)
                        .await
                        .unwrap_or_else(|_| Ok(Vec::new())),
                    None => writes.await,
                }
            }),
            Ok(Credential::Password(_)) => Err(not_oauth()),
            Err(e) => Err(e),
        };
        let refused = result.as_ref().map_or(true, |results| {
            results
                .iter()
                .any(|(_, r)| matches!(r, CategoryResult::Failed(GraphError::Unauthorized)))
        });
        self.finish(account, warnings, refused);
        result
    }

    fn warnings(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.warnings)
    }
}

impl GraphWriter<'_> {
    /// Keeps the login's warnings; after a failure the token may be what failed, so the next
    /// write signs in afresh.
    fn finish(&mut self, account: &Account, warnings: Vec<String>, failed: bool) {
        for warning in warnings {
            self.warnings.push((account.name.clone(), warning));
        }
        if failed {
            self.logins.forget(&account.name);
        }
    }
}

fn not_oauth() -> anyhow::Error {
    anyhow::anyhow!("a Microsoft Graph account signs in with OAuth, not a password")
}

#[cfg(test)]
#[path = "labels/tests.rs"]
mod tests;
