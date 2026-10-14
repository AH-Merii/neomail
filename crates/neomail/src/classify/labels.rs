//! Groups on the server, and confident spam moved to Junk: after `neomail classify` stores its
//! results, and after each daemon wake classifies, every classified message whose effective group
//! differs from the one neomail last wrote gets the new one: an Outlook category
//! (DEC-2026-10-13-categories), or a Gmail or Proton label (DEC-2026-10-14-labels-junk). Spam at
//! or above `classify.junk_at` then moves to the account's Junk folder, at most once. What is
//! still to do comes from the cache ([`Cache::labels_due`]), so the backlog, failed writes and
//! groups that change with age or a new threshold need no queue.
//!
//! [`write_labels`] owns the cap, the lock and the per-account error rules, so the CLI and the
//! daemon share them; [`LabelWriter`] is the seam to the servers ([`ServerWriter`]; a fake in
//! the tests). Other IMAP servers get no labels, only the Junk move, so they are signed in to
//! only when spam is due one. Nothing printed names a subject, a sender or a label or category of
//! the user's.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use nm_core::{
    Cache, Destination, DueFor, Folder, LabelDue, LabelPrefix, MessageId, MessageKey, Moved,
    RemoteId, Uid, UidValidity,
};
use nm_graph::{CONCURRENT_FETCHES, CategoryEdit, CategoryResult, GraphError};
use nm_imap::labels::{LabelEdit, LabelKind, LabelsWritten, NoJunk, Takes};
use tokio::runtime::Runtime;

use crate::config::{Account, ClassifySettings, Config, Server};
use crate::graph;
use crate::lock;
use crate::style::Colors;
use crate::sync::{Credential, Logins};

/// The most messages one `neomail classify` run writes per account: its message cap.
pub const PER_RUN: usize = super::DEFAULT_LIMIT as usize;
/// The most one daemon wake writes per account: its request cap (DEC-2026-10-13-daemon-classify).
pub const PER_WAKE: usize = super::daemon::PER_WAKE;
/// The messages one IMAP session writes: a stop or a deadline is checked between batches.
pub const IMAP_BATCH: usize = 50;

/// Writes groups to an account's server: [`ServerWriter`], or a fake in the tests.
pub(crate) trait LabelWriter {
    type Lock;
    /// The account's lock, as every write takes it; `None` when someone else holds it and the
    /// account is skipped this time.
    fn lock(&mut self, account: &str) -> Result<Option<Self::Lock>>;
    /// Whether a Graph account's sign-in lets neomail change mail (Mail.ReadWrite). Signs in.
    fn may_write(&mut self, account: &Account) -> Result<bool>;
    /// Sets the categories of `edits` on a Graph account, at most [`CONCURRENT_FETCHES`] of
    /// them. At `deadline` the writes still in flight are dropped, and their messages get no
    /// answer.
    fn write(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        edits: &[CategoryEdit],
        deadline: Option<Instant>,
    ) -> Result<Vec<(RemoteId, CategoryResult)>>;
    /// Moves the messages `ids` of a Graph account to its Junk folder (`junkemail`). `None`
    /// when cut off at `deadline`.
    fn move_junk(
        &mut self,
        account: &Account,
        ids: &[RemoteId],
        deadline: Option<Instant>,
    ) -> Result<Option<Moved<RemoteId>>>;
    /// What an IMAP account's server takes: labels, the Junk move. Signs in.
    fn probe(&mut self, account: &Account) -> Result<Takes>;
    /// Writes one batch on an IMAP account ([`nm_imap::labels::write_labels`]). `None` when
    /// cut off at `deadline`.
    fn write_imap(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        uid_validity: UidValidity,
        edits: &[LabelEdit<'_>],
        deadline: Option<Instant>,
    ) -> Result<Option<LabelsWritten>>;
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
    /// The login, the cache, a server refusing a command, Graph refusing the token or the grant
    /// (401, 403), or Graph throttling after its retries or out of reach: the rest wait for next
    /// time.
    Failed(anyhow::Error),
}

/// One account's pass.
#[derive(Debug, Default)]
pub(crate) struct AccountOutcome {
    pub account: String,
    /// Over IMAP: its groups are labels, not categories.
    pub imap: bool,
    /// Its groups are due on its server: asked for, and a server that may take them. Without
    /// them only spam is due, a move to Junk.
    pub labels: bool,
    pub written: usize,
    /// Messages moved to Junk.
    pub junked: usize,
    /// Messages whose write or move failed: still due, so tried again next time.
    pub failed: usize,
    /// Messages left for next time: past the cap, or not reached.
    pub waiting: usize,
    pub off: Option<Off>,
    /// What the account cannot do, said as a warning: no Junk move, earlier labels that stay.
    pub notes: Vec<String>,
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

    /// What the settings ask for: labels unless `labels = false`, Junk moves unless
    /// `junk = false`.
    fn wanted(&self) -> DueFor {
        DueFor {
            labels: self.settings.labels,
            junk: self.settings.rules.junk_at.is_some(),
        }
    }

    /// The due messages of `account` asked for by `for_`: at most the cap, and how many more.
    fn due(
        &self,
        cache: &Cache,
        account: &Account,
        for_: DueFor,
    ) -> Result<(Vec<LabelDue>, usize)> {
        let settings = self.settings;
        Ok(cache.labels_due(
            Some(&account.name),
            self.ids,
            &settings.rules,
            &settings.label_prefix,
            for_,
            self.now,
            self.cap,
        )?)
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

/// Writes what is due on each account in `accounts`: categories on Outlook, labels on Gmail and
/// Proton, Junk moves on all of them. With `labels = false` and `junk = false` it writes nothing.
/// An account with nothing due is not signed in to. One account's failure never stops the
/// others, and nothing here fails the caller: classifying, syncing and notifying go on.
pub(crate) fn write_labels<W: LabelWriter>(
    cache: &mut Cache,
    accounts: &[&Account],
    pass: &Pass<'_>,
    writer: &mut W,
) -> Vec<AccountOutcome> {
    let wanted = pass.wanted();
    if !wanted.labels && !wanted.junk {
        return Vec::new();
    }
    let mut outcomes = Vec::new();
    for account in accounts {
        let imap = match &account.server {
            Server::Imap(imap) => Some(imap),
            Server::Graph { .. } => None,
        };
        let mut outcome = AccountOutcome {
            account: account.name.clone(),
            imap: imap.is_some(),
            labels: wanted.labels && imap.is_none_or(nm_imap::labels::may_take_labels),
            ..AccountOutcome::default()
        };
        let wanted = DueFor {
            labels: outcome.labels,
            junk: wanted.junk,
        };
        let result = if !wanted.labels && !wanted.junk {
            Ok(())
        } else if imap.is_some() {
            write_imap_account(cache, account, pass, wanted, writer, &mut outcome)
        } else {
            write_graph_account(cache, account, pass, wanted, writer, &mut outcome)
        };
        if let Err(e) = result {
            outcome.off = Some(Off::Failed(e));
        }
        let counts = outcome.written + outcome.junked + outcome.failed + outcome.waiting;
        if counts > 0 || outcome.off.is_some() || !outcome.notes.is_empty() {
            outcomes.push(outcome);
        }
    }
    outcomes
}

/// Graph's id for a due message: only Graph messages carry one.
fn graph_id(d: &LabelDue) -> Option<&RemoteId> {
    match &d.key {
        MessageKey::Remote(id) => Some(id),
        MessageKey::Uid { .. } => None,
    }
}

/// An Outlook account: categories in chunks of [`CONCURRENT_FETCHES`], then each chunk's spam
/// moved to `junkemail` once its category is written (or was not due).
fn write_graph_account<W: LabelWriter>(
    cache: &mut Cache,
    account: &Account,
    pass: &Pass<'_>,
    wanted: DueFor,
    writer: &mut W,
    outcome: &mut AccountOutcome,
) -> Result<()> {
    let settings = pass.settings;
    let (due, more) = pass.due(cache, account, wanted)?;
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
        let chunk: Vec<&LabelDue> = chunk.iter().filter(|d| graph_id(d).is_some()).collect();
        // The messages whose category is written or was not due; the ones done with.
        let mut ready: Vec<&LabelDue> = chunk.iter().copied().filter(|d| !d.label).collect();
        let mut done = 0;
        let mut ended: Option<GraphError> = None;
        let edits: Vec<CategoryEdit> = chunk
            .iter()
            .filter(|d| d.label)
            .filter_map(|d| {
                Some(CategoryEdit {
                    id: graph_id(d)?.clone(),
                    want: d.want.clone(),
                    last: d.last.clone(),
                })
            })
            .collect();
        if !edits.is_empty() {
            let results = writer.write(account, &settings.label_prefix, &edits, pass.deadline)?;
            for (id, result) in results {
                let Some(d) = chunk.iter().copied().find(|d| graph_id(d) == Some(&id)) else {
                    continue;
                };
                match result {
                    CategoryResult::Set | CategoryResult::Already => {
                        cache.store_label(&account.name, d.item(), &d.want, pass.now)?;
                        if matches!(result, CategoryResult::Set) {
                            outcome.written += 1;
                        }
                        ready.push(d);
                    }
                    // The next sync drops it from the cache.
                    CategoryResult::Gone => done += 1,
                    // A refusal says more than throttling: it needs a new sign-in.
                    CategoryResult::Failed(e) if ends_pass(&e) => {
                        let refusal = matches!(e, GraphError::Unauthorized | GraphError::Forbidden);
                        if ended.is_none() || refusal {
                            ended = Some(e);
                        }
                    }
                    CategoryResult::Failed(_) => {
                        done += 1;
                        outcome.failed += 1;
                    }
                }
            }
        }
        done += ready.iter().filter(|d| !d.junk).count();
        let junk: Vec<&LabelDue> = ready.into_iter().filter(|d| d.junk).collect();
        if ended.is_none() && !junk.is_empty() && !pass.over() {
            done += move_graph_junk(cache, account, pass, writer, outcome, &junk)?;
        }
        // The messages that ended the pass, and the ones that got no answer, wait.
        outcome.waiting -= done;
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

/// Moves the Outlook messages `junk` to `junkemail`, each move recorded before it is sent.
/// Returns how many are done with: moved, gone, or not confirmed (failed, and tried again); none
/// when the moves were cut off at the deadline, as some may have been sent.
fn move_graph_junk<W: LabelWriter>(
    cache: &mut Cache,
    account: &Account,
    pass: &Pass<'_>,
    writer: &mut W,
    outcome: &mut AccountOutcome,
    junk: &[&LabelDue],
) -> Result<usize> {
    for d in junk {
        cache.junk_attempt(&account.name, &d.msg_key, d.id, pass.now)?;
    }
    let ids: Vec<RemoteId> = junk.iter().filter_map(|d| graph_id(d).cloned()).collect();
    let Some(moved) = writer.move_junk(account, &ids, pass.deadline)? else {
        return Ok(0);
    };
    let mut left = Vec::new();
    for d in junk {
        let Some(id) = graph_id(d) else { continue };
        if moved.moved.contains(id) {
            cache.junk_moved(&account.name, &d.msg_key, pass.now)?;
            left.push(d.id);
            outcome.junked += 1;
        } else if moved.gone.contains(id) {
            // It left the INBOX already: never moved now.
            cache.junk_moved(&account.name, &d.msg_key, pass.now)?;
        } else {
            outcome.failed += 1;
        }
    }
    // A confirmed move takes the message out of the cache, as archiving does.
    cache.remove(&left)?;
    Ok(junk.len())
}

/// An IMAP account: what its server takes first, then batches of [`IMAP_BATCH`], each in one
/// session: labels, then spam to Junk.
fn write_imap_account<W: LabelWriter>(
    cache: &mut Cache,
    account: &Account,
    pass: &Pass<'_>,
    wanted: DueFor,
    writer: &mut W,
    outcome: &mut AccountOutcome,
) -> Result<()> {
    let (due, more) = pass.due(cache, account, wanted)?;
    if due.is_empty() {
        return Ok(());
    }
    outcome.waiting = due.len() + more;
    if pass.over() {
        return Ok(());
    }
    let Some(_held) = writer.lock(&account.name)? else {
        outcome.off = Some(Off::Locked);
        return Ok(());
    };
    let takes = writer.probe(account)?;
    let can = DueFor {
        labels: wanted.labels && takes.labels != LabelKind::None,
        junk: wanted.junk && takes.no_junk.is_none(),
    };
    if let Some(why) = takes.no_junk
        && wanted.junk
        && due.iter().any(|d| d.junk)
    {
        outcome.notes.push(no_junk_note(&account.name, why));
    }
    // Again for what the server takes, so what it cannot do never fills the cap.
    let (due, more) = if can == wanted {
        (due, more)
    } else if can.labels || can.junk {
        pass.due(cache, account, can)?
    } else {
        (Vec::new(), 0)
    };
    outcome.waiting = due.len() + more;
    let mut kept = 0;
    for chunk in due.chunks(IMAP_BATCH) {
        if pass.over() {
            break;
        }
        // The cache holds INBOX only: one mailbox, so one UIDVALIDITY.
        let keyed: Vec<(&LabelDue, Uid, UidValidity)> = chunk
            .iter()
            .filter_map(|d| match d.key {
                MessageKey::Uid { uid_validity, uid } => Some((d, uid, uid_validity)),
                MessageKey::Remote(_) => None,
            })
            .collect();
        let Some(&(_, _, validity)) = keyed.first() else {
            continue;
        };
        let batch: Vec<(&LabelDue, Uid)> = keyed
            .into_iter()
            .filter(|&(_, _, v)| v == validity)
            .map(|(d, uid, _)| (d, uid))
            .collect();
        // Recorded before it is sent: a message is moved at most once.
        for (d, _) in batch.iter().filter(|(d, _)| d.junk) {
            cache.junk_attempt(&account.name, &d.msg_key, d.id, pass.now)?;
        }
        let edits: Vec<LabelEdit<'_>> = batch
            .iter()
            .map(|&(d, uid)| LabelEdit {
                uid,
                want: d.label.then_some(d.want.as_str()),
                last: d.last.as_deref(),
                message_id: (!d.msg_key.starts_with("sha256:")).then_some(d.msg_key.as_str()),
                junk: d.junk,
            })
            .collect();
        let prefix = &pass.settings.label_prefix;
        let Some(written) = writer.write_imap(account, prefix, validity, &edits, pass.deadline)?
        else {
            // Cut off at the deadline: all of it waits.
            break;
        };
        outcome.waiting -= batch.len();
        let mut moved = Vec::new();
        for (i, &(d, _)) in batch.iter().enumerate() {
            let labelled = written.labelled.iter().find(|&&(l, _)| l == i);
            if let Some(&(_, changed)) = labelled {
                cache.store_label(&account.name, d.item(), &d.want, pass.now)?;
                outcome.written += usize::from(changed);
            }
            kept += usize::from(written.kept.contains(&i));
            if written.gone.contains(&i) {
                if d.junk {
                    // It left the INBOX already: never moved now.
                    cache.junk_moved(&account.name, &d.msg_key, pass.now)?;
                }
                continue;
            }
            let mut missed = d.label && labelled.is_none();
            if d.junk {
                if written.moved.contains(&i) {
                    cache.junk_moved(&account.name, &d.msg_key, pass.now)?;
                    moved.push(d.id);
                    outcome.junked += 1;
                } else if written.unmoved.contains(&i) {
                    // Sent and not confirmed: the record stays, and the move is tried again.
                    missed = true;
                } else {
                    // Never sent, as its label was not written: due again as it was.
                    cache.junk_not_sent(&account.name, &d.msg_key)?;
                }
            }
            outcome.failed += usize::from(missed);
        }
        // A confirmed move takes the message out of the cache, as archiving does.
        cache.remove(&moved)?;
    }
    if kept > 0 {
        outcome.notes.push(format!(
            "{}: {kept} earlier label{} stay{} beside the new one: removing a Proton label \
             needs UIDPLUS and a Message-ID",
            account.name,
            if kept == 1 { "" } else { "s" },
            if kept == 1 { "s" } else { "" },
        ));
    }
    Ok(())
}

/// Why spam stays in the INBOX of `account`.
fn no_junk_note(account: &str, why: NoJunk) -> String {
    let reason = match why {
        NoJunk::NoMove => "the server does not support IMAP MOVE (RFC 6851)",
        NoJunk::NoFolder => {
            "the server has no Junk folder: no mailbox has the \\Junk attribute (RFC 6154)"
        }
    };
    format!("{account}: spam is not moved to Junk: {reason}")
}

/// `neomail classify`'s pass, after its results are stored: the accounts in its scope
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
        .filter(|a| account.is_none_or(|name| a.name == name))
        .collect();
    let settings = &config.classify;
    if accounts.is_empty() || (!settings.labels && settings.rules.junk_at.is_none()) {
        return Ok(false);
    }
    let runtime = crate::account::runtime()?;
    let data_dir = crate::config::data_dir()?;
    let mut writer = ServerWriter::waiting(&runtime, &data_dir);
    let stop = AtomicBool::new(false);
    let ids = (!ids.is_empty()).then_some(ids);
    let pass = Pass::run(settings, ids, &stop, Utc::now());
    let outcomes = write_labels(cache, &accounts, &pass, &mut writer);
    for (account, warning) in writer.warnings() {
        eprintln!("neomail: {account}: {warning}");
    }
    for o in &outcomes {
        for note in &o.notes {
            eprintln!("neomail: {note}");
        }
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

/// What an account's groups are on its server, for the lines.
fn kind(o: &AccountOutcome) -> (&'static str, &'static str) {
    if o.imap {
        ("label", "labels")
    } else {
        ("category", "categories")
    }
}

/// The line that counts an account's writes and moves, `None` when none was answered: nothing
/// was tried (a held lock, a stop, a spent budget), or every message carried its group already.
/// `next` is when the rest is tried: "run" or "wake".
pub(crate) fn summary(o: &AccountOutcome, next: &str) -> Option<String> {
    if o.written + o.failed + o.junked == 0 {
        return None;
    }
    let (one, many) = kind(o);
    let mut parts = Vec::new();
    // A pass that only moved spam says so alone.
    if o.written > 0 || (o.junked == 0 && o.labels) {
        let noun = if o.written == 1 { one } else { many };
        parts.push(format!("{} {noun} written", o.written));
    }
    if o.junked > 0 || !o.labels {
        parts.push(format!("{} moved to Junk", o.junked));
    }
    let mut line = format!("{}: {}", o.account, parts.join(", "));
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
        Off::Failed(e) if o.labels => Some(format!("{account}: {} not written: {e:#}", kind(o).1)),
        Off::Failed(e) => Some(format!("{account}: spam not moved to Junk: {e:#}")),
    }
}

/// Whether the pass failed for `neomail classify`'s exit code: a write, a move or an account
/// failed. A Mail.Read sign-in, a server without Junk and an earlier label that stays are
/// warnings, not failures.
pub(crate) fn failed(outcomes: &[AccountOutcome]) -> bool {
    outcomes
        .iter()
        .any(|o| o.failed > 0 || matches!(o.off, Some(Off::Failed(_))))
}

/// The daemon's lines about groups on the server, each said once per state as the sync's are:
/// the counts when the wake wrote or moved some or the failed and waiting counts changed; why an
/// account is off, or what it cannot do, when that differs from what was last said, and again
/// only after the account wrote without it; the sign-in's warnings when they differ from the
/// account's last one. A wake that tried nothing (a held lock, a stop, a spent budget) changes
/// none of that.
#[derive(Default)]
pub(crate) struct Reports {
    last_off: HashMap<String, String>,
    /// The (failed, waiting) counts last printed, for the accounts that still have some due.
    last_counts: HashMap<String, (usize, usize)>,
    last_warning: HashMap<String, String>,
    last_notes: HashMap<String, Vec<String>>,
}

impl Reports {
    pub(crate) fn lines(&mut self, outcomes: &[AccountOutcome]) -> Vec<crate::daemon::Line> {
        use crate::daemon::Line;
        let mut lines = Vec::new();
        // An account with nothing due has everything written: its counts start afresh.
        self.last_counts
            .retain(|account, _| outcomes.iter().any(|o| &o.account == account));
        for o in outcomes {
            if !o.notes.is_empty() && self.last_notes.get(&o.account) != Some(&o.notes) {
                for note in &o.notes {
                    lines.push(Line::Error(format!("neomail: {note}")));
                }
                self.last_notes.insert(o.account.clone(), o.notes.clone());
            }
            let off = off_line(o);
            if let Some(off) = off.as_ref()
                && self.last_off.get(&o.account) != Some(off)
            {
                lines.push(Line::Error(format!("neomail: {off}")));
                self.last_off.insert(o.account.clone(), off.clone());
            }
            let counts = (o.failed, o.waiting);
            let news =
                o.written + o.junked > 0 || self.last_counts.get(&o.account) != Some(&counts);
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

/// A daemon wake's step for groups on the server, after it classified: writes and moves what is
/// due within `budget` and returns the lines to print. Each wake that synced runs it, as what is
/// due changes with age.
pub(crate) fn daemon_wake<W: LabelWriter>(
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

/// The real writer: Graph and IMAP through the logins `neomail sync` uses, under the account
/// locks. `wait` waits for a held lock (`neomail classify`); otherwise the account is skipped
/// (the daemon, which must not stall its round). The sign-in's warnings are kept for the caller
/// to print, so the daemon prints them with its time and once per state. What an IMAP server
/// takes is kept from the last session with it, so the daemon asks once.
pub(crate) struct ServerWriter<'a> {
    runtime: &'a Runtime,
    data_dir: &'a Path,
    logins: Logins,
    wait: bool,
    warnings: Vec<(String, String)>,
    takes: HashMap<String, Takes>,
}

impl<'a> ServerWriter<'a> {
    /// `neomail classify`'s: waits for an account's lock.
    pub(crate) fn waiting(runtime: &'a Runtime, data_dir: &'a Path) -> Self {
        Self::new(runtime, data_dir, true)
    }

    /// The daemon's: skips an account whose lock is held, so its round never stalls. Its own
    /// logins: a token is refreshed only while something is due.
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
            takes: HashMap::new(),
        }
    }

    /// Keeps the login's warnings; after a failure the token may be what failed, so the next
    /// write signs in afresh and asks the server again what it takes.
    fn finish(&mut self, account: &Account, warnings: Vec<String>, failed: bool) {
        for warning in warnings {
            self.warnings.push((account.name.clone(), warning));
        }
        if failed {
            self.logins.forget(&account.name);
            self.takes.remove(&account.name);
        }
    }

    /// Runs `write` with the IMAP account's server and login.
    fn imap<T>(
        &mut self,
        account: &Account,
        write: impl FnOnce(&Runtime, &nm_imap::ImapConfig, nm_imap::Login<'_>) -> Result<T>,
    ) -> Result<T> {
        let Server::Imap(imap) = &account.server else {
            anyhow::bail!("{} reads Microsoft Graph, not IMAP", account.name);
        };
        let runtime = self.runtime;
        let mut warnings = Vec::new();
        let result = self
            .logins
            .credential(runtime, account, &mut warnings)
            .and_then(|credential| write(runtime, imap, credential.imap()));
        self.finish(account, warnings, result.is_err());
        result
    }
}

/// `fut` cut off at `deadline`, when there is one: `None` then.
async fn until<T>(deadline: Option<Instant>, fut: impl Future<Output = T>) -> Option<T> {
    match deadline {
        Some(at) => tokio::time::timeout_at(at.into(), fut).await.ok(),
        None => Some(fut.await),
    }
}

impl LabelWriter for ServerWriter<'_> {
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
                // A write cut off is tried again next time: it reads the categories afresh.
                until(deadline, graph::set_categories(token, prefix, edits))
                    .await
                    .unwrap_or_else(|| Ok(Vec::new()))
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

    fn move_junk(
        &mut self,
        account: &Account,
        ids: &[RemoteId],
        deadline: Option<Instant>,
    ) -> Result<Option<Moved<RemoteId>>> {
        let runtime = self.runtime;
        let mut warnings = Vec::new();
        let to = Destination::Role(Folder::Junk);
        let result = match self.logins.credential(runtime, account, &mut warnings) {
            Ok(Credential::AccessToken(token)) => runtime.block_on(async {
                until(deadline, graph::move_to(&account.name, token, ids, &to))
                    .await
                    .transpose()
            }),
            Ok(Credential::Password(_)) => Err(not_oauth()),
            Err(e) => Err(e),
        };
        self.finish(account, warnings, result.is_err());
        result
    }

    fn probe(&mut self, account: &Account) -> Result<Takes> {
        if let Some(takes) = self.takes.get(&account.name) {
            return Ok(*takes);
        }
        let takes = self.imap(account, |runtime, imap, login| {
            Ok(runtime.block_on(nm_imap::labels::probe(imap, login))?)
        })?;
        self.takes.insert(account.name.clone(), takes);
        Ok(takes)
    }

    fn write_imap(
        &mut self,
        account: &Account,
        prefix: &LabelPrefix,
        uid_validity: UidValidity,
        edits: &[LabelEdit<'_>],
        deadline: Option<Instant>,
    ) -> Result<Option<LabelsWritten>> {
        let written = self.imap(account, |runtime, imap, login| {
            runtime.block_on(async {
                let write = nm_imap::labels::write_labels(imap, login, uid_validity, prefix, edits);
                Ok(until(deadline, write).await.transpose()?)
            })
        })?;
        if let Some(takes) = written.as_ref().and_then(|w| w.takes) {
            self.takes.insert(account.name.clone(), takes);
        }
        Ok(written)
    }

    fn warnings(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.warnings)
    }
}

fn not_oauth() -> anyhow::Error {
    anyhow::anyhow!("a Microsoft Graph account signs in with OAuth, not a password")
}

#[cfg(test)]
#[path = "labels/tests.rs"]
mod tests;
