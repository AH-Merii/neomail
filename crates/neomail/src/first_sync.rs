//! The question after an account's first `neomail sync`: classify its last year now?
//! (DEC-2026-10-14-first-sync). Mail goes to the classifier only after a yes, and a yes runs
//! what `neomail classify --account NAME` runs, its consent question included
//! (DEC-2026-10-14-classify-when-asked). It is asked once per account, only on a terminal and
//! only when a key is stored; the answer is kept in the data directory.

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use nm_core::{Cache, GroupRules, SyncReport};

use crate::classify::{self, Preview, shell_quote};
use crate::config::{self, Config};
use crate::keyring::{self, Keyring};
use crate::secret::Input;
use crate::style::Colors;

/// The file in the data directory that keeps each account's answer, a JSON object of account
/// names and `"yes"` or `"no"`. Not in the cache, so deleting `cache.sqlite3` keeps it.
pub const ANSWERS_FILE: &str = "classify-first-sync.json";

/// `neomail sync --help`'s closing text.
pub const SYNC_HELP: &str = "\
After an account's first sync, neomail asks once, on a terminal, whether to classify that \
account's mail from the last year, showing how many messages, requests and input tokens that \
takes and what the rest would take. Nothing is sent without a yes; a yes runs `neomail classify --account NAME`, which asks \
for consent before the first request to a backend. The answer is kept in classify-first-sync.json \
in the data directory and the question is not asked again for that account. Off a terminal or \
with --no-input nothing is asked and nothing is classified. Without a jev key one line says how \
to set it up.";

/// Whether `report` is from an account's first sync: one that starts, or finishes, an INBOX
/// synced from empty. A rebuild after a UIDVALIDITY change is not one, since its mail was asked
/// about before if ever; a later sync that finishes an interrupted rebuild still is
/// ([`SyncReport::initial`] stays set until a sync completes), as is every sync until the first
/// one completes. Asking again then is harmless: nothing is sent without a yes.
fn is_first(report: &SyncReport) -> bool {
    report.initial && !report.reset
}

/// The accounts in `synced`, with their reports in order, whose sync [`is_first`].
pub fn firsts(synced: &[(String, SyncReport)]) -> Vec<String> {
    synced
        .iter()
        .filter(|(_, report)| is_first(report))
        .map(|(account, _)| account.clone())
        .collect()
}

/// Whether the question may be asked: standard input is a terminal, `--no-input` does not
/// forbid it, and standard error, where the question goes, is a terminal too.
fn can_ask(input: Input, stderr_terminal: bool) -> bool {
    input.can_prompt() && stderr_terminal
}

/// Whether classifying can start, and what the question shows.
pub(crate) enum Setup {
    Ready {
        host: String,
        rules: GroupRules,
        /// Whether classifying also writes groups to the server (`classify.labels`).
        labels: bool,
        preview: Preview,
    },
    /// No key for the backend: nothing is asked.
    NoKey,
}

/// What [`offer`] did.
#[derive(Debug, PartialEq)]
pub(crate) enum Offered {
    /// Answered before, or nothing from the window waits: nothing said.
    Quiet,
    /// One line said how to classify or how to set it up; nothing was asked.
    Hinted,
    /// Asked; a no, or no answer.
    Declined,
    /// Asked; a yes, and the classify run's exit code.
    Classified(ExitCode),
}

/// The account's earlier answer: `Some(true)` for a yes.
pub(crate) fn answered(data_dir: &Path, account: &str) -> Result<Option<bool>> {
    Ok(read_answers(data_dir)?.get(account).map(|a| a == "yes"))
}

fn read_answers(data_dir: &Path) -> Result<BTreeMap<String, String>> {
    let path = data_dir.join(ANSWERS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Keeps `account`'s answer, replacing the file whole so that a stopped write leaves the old one.
fn remember(data_dir: &Path, account: &str, yes: bool) -> Result<()> {
    let mut answers = read_answers(data_dir)?;
    let answer = if yes { "yes" } else { "no" };
    answers.insert(account.to_owned(), answer.to_owned());
    crate::create_private_dir(data_dir)?;
    let path = data_dir.join(ANSWERS_FILE);
    let partial = data_dir.join(format!("{ANSWERS_FILE}.partial"));
    let text = serde_json::to_string_pretty(&answers)? + "\n";
    // Private, as the consent file is: only the user reads which accounts said yes.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&partial)
        .with_context(|| format!("cannot create {}", partial.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", partial.display()))?;
    drop(file);
    std::fs::rename(&partial, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// `neomail classify --account NAME`.
fn classify_command(account: &str) -> String {
    format!("neomail classify --account {}", shell_quote(account))
}

/// The question's text, without the prompt.
fn question(
    account: &str,
    host: &str,
    rules: &GroupRules,
    labels: bool,
    preview: &Preview,
    kept: &Path,
) -> String {
    let mut text = format!(
        "{account}'s first sync is done. neomail can sort its mail into groups with jev at {host}:\n\
         \x20 the last {} days: {}\n",
        classify::DEFAULT_DAYS,
        preview.send
    );
    if let Some((rest, command)) = &preview.rest {
        text.push_str(&format!(
            "  the rest (older, or past the {}-message cap): {rest};\n    `{command}` sends it later\n",
            classify::thousands(u64::from(classify::DEFAULT_LIMIT))
        ));
    }
    if let Some(basis) = preview.tokens_from {
        text.push_str(&format!("  tokens from: {basis}\n"));
    }
    text.push_str(&format!(
        "Each message gets one group. Spam is grouped as spam; any other answer below a\n\
         confidence of {} lists as other, and a follow-up older than {} days as follow-up-old.\n",
        rules.other_below, rules.follow_up_old_days,
    ));
    // What a yes changes on the server, as `neomail classify` does it (docs/labels.md).
    let writes =
        labels.then_some("writes each message's group to its server as a label or category");
    let moves = rules
        .junk_at
        .map(|at| format!("moves spam at a confidence of {at} or more to Junk"));
    match (writes, moves) {
        (Some(w), Some(m)) => text.push_str(&format!("Classifying also {w}\nand {m}.\n")),
        (Some(w), None) => text.push_str(&format!("Classifying also {w}.\n")),
        (None, Some(m)) => text.push_str(&format!("Classifying also {m}.\n")),
        (None, None) => {}
    }
    text.push_str(&format!(
        "Nothing is sent without a yes. The answer is kept in\n  {}\n\
         and not asked again for {account}.\n",
        kept.display()
    ));
    text
}

/// The first-sync question for `account`. `setup` says whether classifying can start; `ask`
/// shows a text and a prompt and returns the answer; `classify` runs `neomail classify
/// --account NAME`, asking its consent question through the `ask` it is given. Lines go to `err`.
#[allow(clippy::type_complexity, reason = "classify gets the ask")]
pub(crate) fn offer(
    account: &str,
    data_dir: &Path,
    can_ask: bool,
    setup: &mut dyn FnMut() -> Result<Setup>,
    ask: &mut dyn FnMut(&str, &str) -> Result<String>,
    classify: &mut dyn FnMut(&mut dyn FnMut(&str, &str) -> Result<String>) -> Result<ExitCode>,
    err: &mut dyn Write,
) -> Result<Offered> {
    if answered(data_dir, account)?.is_some() {
        return Ok(Offered::Quiet);
    }
    let command = classify_command(account);
    if !can_ask {
        writeln!(
            err,
            "neomail: {account}: nothing was classified; `{command}` sorts the last year's mail \
             into groups (`--estimate` first shows what it would cost)"
        )?;
        return Ok(Offered::Hinted);
    }
    let (host, rules, labels, preview) = match setup()? {
        Setup::Ready {
            host,
            rules,
            labels,
            preview,
        } => (host, rules, labels, preview),
        Setup::NoKey => {
            writeln!(
                err,
                "neomail: {account}: to sort its mail into groups, store a jev key with \
                 `neomail key set` (or set JEV_API_KEY), then run `{command}`"
            )?;
            return Ok(Offered::Hinted);
        }
    };
    if preview.messages == 0 {
        return Ok(Offered::Quiet);
    }
    let kept = data_dir.join(ANSWERS_FILE);
    let text = question(account, &host, &rules, labels, &preview, &kept);
    let prompt = format!("Classify {account}'s last year now? [y/N] ");
    // Standard input closed is no answer, which is a no.
    let yes =
        ask(&text, &prompt).is_ok_and(|a| matches!(a.to_ascii_lowercase().as_str(), "y" | "yes"));
    remember(data_dir, account, yes)?;
    if !yes {
        writeln!(
            err,
            "neomail: nothing was classified; run `{command}` when you want to"
        )?;
        return Ok(Offered::Declined);
    }
    Ok(Offered::Classified(classify(ask)?))
}

/// `neomail classify`'s arguments alone, parsed as the command line would give them.
#[derive(Parser)]
#[command(name = "classify")]
struct ClassifyLine {
    #[command(flatten)]
    args: classify::Args,
}

/// The arguments of `neomail classify --account NAME`, with its defaults.
fn classify_args(account: &str) -> Result<classify::Args> {
    Ok(ClassifyLine::try_parse_from(["classify", "--account", account])?.args)
}

/// [`Setup`] from the config, the keyring (or `JEV_API_KEY`) and the cache.
fn setup(config: &Config, account: &str) -> Result<Setup> {
    let settings = &config.classify;
    let host = settings.host()?;
    // The keyring before any runtime starts: zbus runs one of its own (keyring.rs).
    let store = Keyring::open_for_jev().ok();
    let key = keyring::jev_key(store.as_ref(), &host, std::env::var(keyring::JEV_ENV).ok());
    drop(store);
    if key.is_none() {
        return Ok(Setup::NoKey);
    }
    let mut cache: Cache = crate::open_cache(false)?.context("the cache is missing")?;
    let owners = crate::redact::owner_addresses(config);
    let args = classify_args(account)?;
    let preview = classify::preview(&mut cache, &args, settings, &owners, chrono::Utc::now())?;
    Ok(Setup::Ready {
        host,
        rules: settings.rules,
        labels: settings.labels,
        preview,
    })
}

/// Shows `text` on standard error and asks `prompt` on the terminal.
fn ask_terminal(text: &str, prompt: &str) -> Result<String> {
    eprint!("{text}");
    crate::secret::prompt_line(prompt)
}

/// After `neomail sync` printed its lines: the question for each account in `firsts` (from
/// [`firsts`]), in turn. Returns whether something failed: a classify run, or the
/// question itself, whose error is one line naming the account.
#[allow(
    clippy::too_many_arguments,
    reason = "classify's arguments and the accounts"
)]
pub fn after_sync(
    config: &Config,
    config_path: Option<PathBuf>,
    firsts: &[String],
    input: Input,
    terminal: bool,
    colors: Colors,
    out: &mut dyn Write,
) -> Result<bool> {
    if firsts.is_empty() {
        return Ok(false);
    }
    let data_dir = config::data_dir()?;
    // The question goes to standard error: it must be seen as well as answered.
    let can_ask = can_ask(input, io::stderr().is_terminal());
    let mut failed = false;
    for account in firsts {
        let mut classify = |ask: &mut dyn FnMut(&str, &str) -> Result<String>| {
            let args = classify_args(account)?;
            let path = config_path.clone();
            classify::run_with(&args, Vec::new(), path, input, terminal, colors, out, ask)
        };
        let offered = offer(
            account,
            &data_dir,
            can_ask,
            &mut || setup(config, account),
            &mut ask_terminal,
            &mut classify,
            &mut io::stderr(),
        );
        out.flush()?;
        match offered {
            Ok(Offered::Classified(code)) if code != ExitCode::SUCCESS => failed = true,
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {account}: {e:#}", colors.stderr.error("neomail"));
                failed = true;
            }
        }
    }
    Ok(failed)
}

#[cfg(test)]
mod tests;
