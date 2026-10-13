//! `neomail classify` and `neomail key`: choose the cached messages to classify, ask once per
//! backend host before anything is sent, run the requests four at a time, store each result as
//! it arrives, and estimate what is left. The wire format is jev.rs's; what is sent is built by
//! redact.rs from nm-redact's [`Outgoing`](nm_redact::Outgoing) alone.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeDelta, Utc};
use nm_core::{Cache, Choice, Classification, Group, GroupRules, MessageId};
use serde::Serialize;

use crate::config::{self, ClassifySettings};
use crate::http::{self, Https};
use crate::jev::{self, Answer, Verdict};
use crate::keyring::{self, Keyring};
use crate::oauth::Secret;
use crate::output::{self, Mode};
use crate::secret::{self, Input};
use crate::style::Colors;
use crate::{EXIT_FAILURE, IdArg, ListingFormat, redact, usage};

/// `--days`' default: the last year (DEC-2026-10-11-first-run).
pub const DEFAULT_DAYS: u32 = 365;
/// `--limit`'s default: the newest 1,000 messages (DEC-2026-10-11-first-run).
pub const DEFAULT_LIMIT: u32 = 1000;
/// Requests in flight at once, one connection each.
const CONCURRENCY: usize = 4;
/// TypeSafe's price per million input tokens; output tokens are free
/// (<https://docs.typesafe.ai/models.md>).
const USD_PER_MILLION_INPUT: f64 = 0.042;
/// The host the price applies to.
const TYPESAFE_HOST: &str = "api.typesafe.ai";
/// The file in the data directory listing the hosts the user agreed to send to, one per line.
/// Not in the cache, so deleting `cache.sqlite3` keeps it; not in the config, which is the user's.
const CONSENT_FILE: &str = "classify-consent";
/// How many messages the estimate reads when nothing else gives it a size.
const SAMPLE: usize = 50;
const TYPESAFE_PRIVACY: &str = "https://www.typesafe.ai/legal/privacy-policy";
const TYPESAFE_TERMS: &str = "https://www.typesafe.ai/legal/mca";

#[derive(clap::Args)]
pub struct Args {
    /// Message ids, the first column of `neomail list`, or - to read them from standard input; without ids, the cached messages that have no group yet
    #[arg(value_name = "ID", value_parser = crate::parse_id_arg)]
    pub ids: Vec<IdArg>,
    /// Classify only this account's messages
    #[arg(long, value_name = "NAME", conflicts_with = "ids")]
    pub account: Option<String>,
    /// Only messages received in the last N days
    #[arg(long, value_name = "N", default_value_t = DEFAULT_DAYS, value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "ids")]
    pub days: u32,
    /// At most N messages, the newest first
    #[arg(long, value_name = "N", default_value_t = DEFAULT_LIMIT, value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "ids")]
    pub limit: u32,
    /// Every cached message without a group, whatever its age and however many
    #[arg(long, conflicts_with = "ids")]
    pub all: bool,
    /// Ask again for messages that already have a group
    #[arg(long)]
    pub again: bool,
    /// Print what a run would send and cost, and send nothing
    #[arg(long)]
    pub estimate: bool,
    /// Consent to sending to the backend without being asked (remembered, as a typed yes is)
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub format: ListingFormat,
}

#[derive(clap::Subcommand)]
pub enum KeyCommand {
    /// Store the key: typed without echo on a terminal, else one line of standard input
    #[command(after_help = KEY_EXAMPLES)]
    Set {
        /// The backend whose key this is [default: classify.url, else https://api.typesafe.ai]
        #[arg(long, value_name = "URL")]
        backend: Option<String>,
    },
    /// Delete the stored key
    Delete {
        /// The backend whose key this is [default: classify.url, else https://api.typesafe.ai]
        #[arg(long, value_name = "URL")]
        backend: Option<String>,
    },
    /// Say where the key comes from (the OS keyring, JEV_API_KEY, or nowhere), never the key
    Status {
        /// The backend whose key this is [default: classify.url, else https://api.typesafe.ai]
        #[arg(long, value_name = "URL")]
        backend: Option<String>,
    },
}

/// `neomail key`'s and `neomail key set`'s examples.
pub const KEY_EXAMPLES: &str = "\
Examples:
  neomail key set                   Type the key; it is not echoed
  pass show jev | neomail key set
                                    Store a key read from another store
  neomail key set --backend http://127.0.0.1:8080
                                    Store the key for a local System One backend
  neomail key status                Say where the key comes from, never the key
  neomail key delete                Delete the stored key

The key goes in the OS keyring under the service neomail-jev, one entry per backend host.
JEV_API_KEY is used when the keyring has none.";

/// `neomail key set|delete|status`.
pub fn key(
    command: &KeyCommand,
    config: Option<PathBuf>,
    input: Input,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let (KeyCommand::Set { backend }
    | KeyCommand::Delete { backend }
    | KeyCommand::Status { backend }) = command;
    let (url, host) = match backend {
        Some(url) => {
            let url = url.trim().trim_end_matches('/').to_owned();
            // A value given on the command line that cannot be used is bad usage, as for clap's.
            let host = http::url_host(&url).map_err(|e| usage(format!("--backend: {e:#}")))?;
            (url, host)
        }
        None => {
            let settings = settings(config)?;
            let host = settings.host().context("classify.url")?;
            (settings.url, host)
        }
    };
    match command {
        KeyCommand::Set { .. } => {
            // Before the key is typed: no one should type it only to learn it cannot be stored.
            let store = Keyring::open_for_jev()?;
            let key = match secret::read_secret(&format!("jev key for {host}: "), "key", input) {
                Err(_) if input.no_input && !input.terminal => {
                    return Err(usage(
                        "--no-input: pipe the key on standard input; nothing was stored",
                    ));
                }
                key => key?,
            };
            // As an empty line is (secret.rs): a blank entry would shadow JEV_API_KEY.
            let key = key.trim();
            if key.is_empty() {
                bail!("the key is empty; nothing was stored");
            }
            store.set_jev_key(&host, &Secret::new(key.to_owned()))?;
            writeln!(
                out,
                "Stored the jev key for {host} in {} (service {:?}, entry {host:?}).",
                store.name(),
                keyring::JEV_SERVICE
            )?;
        }
        KeyCommand::Delete { .. } => {
            let store = Keyring::open_for_jev()?;
            let (done, prep) = if store.delete_jev_key(&host)? {
                ("Deleted the", "from")
            } else {
                ("There was no", "in")
            };
            writeln!(out, "{done} jev key for {host} {prep} {}.", store.name())?;
        }
        KeyCommand::Status { .. } => {
            let store = Keyring::open_for_jev().ok();
            let env = std::env::var(keyring::JEV_ENV).ok();
            match keyring::jev_key(store.as_ref(), &host, env) {
                Some((_, source)) => {
                    writeln!(out, "{host}: the key comes from {}", source.describe())?;
                }
                None => {
                    // `key set` alone stores the key for classify.url, not for this backend.
                    let hint = match backend {
                        Some(_) => format!(
                            "no jev key: run `neomail key set --backend {}` or set JEV_API_KEY",
                            shell_quote(&url)
                        ),
                        None => keyring::NO_JEV_KEY.to_owned(),
                    };
                    writeln!(out, "{host}: {hint}")?;
                    return Ok(ExitCode::from(EXIT_FAILURE));
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The `[classify]` settings: the config's, or the defaults without a config file. A config the
/// user named must load, as for `classify`, so that both agree on the backend.
fn settings(config: Option<PathBuf>) -> Result<ClassifySettings> {
    Ok(config::load_optional(config, true)?
        .map(|c| c.classify)
        .unwrap_or_default())
}

/// Whether the user agreed to send to `host`.
fn consented(data_dir: &Path, host: &str) -> Result<bool> {
    let path = data_dir.join(CONSENT_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text.lines().any(|line| line.trim() == host)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Remembers that the user agreed to send to `host`.
fn remember_consent(data_dir: &Path, host: &str) -> Result<()> {
    crate::create_private_dir(data_dir)?;
    let path = data_dir.join(CONSENT_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    writeln!(file, "{host}").with_context(|| format!("cannot write {}", path.display()))
}

/// What is said before the first request to `host`.
fn consent_text(host: &str, messages: usize) -> String {
    let keeps = if host == TYPESAFE_HOST || host.ends_with(".typesafe.ai") {
        format!(
            "TypeSafe keeps what it receives as its terms say:\n  {TYPESAFE_PRIVACY}\n  \
             {TYPESAFE_TERMS}"
        )
    } else {
        format!("{host} keeps what it receives as its own terms say.")
    };
    format!(
        "neomail classify sends one request per message to {host}, {messages} now, tried again\n\
         up to twice on a rate limit or a server error. Each holds:\n\
         \x20 - the subject and the body's first 1500 characters, with the part of each email\n\
         \x20   address before the @, links (their host stays), phone, card and IBAN numbers,\n\
         \x20   amounts, one-time codes, runs of 4 or more digits and the names in the headers\n\
         \x20   masked; shorter numbers, postal addresses and names that are in no header are\n\
         \x20   sent as written;\n\
         \x20 - the sender's domain (a personal domain identifies its owner);\n\
         \x20 - whether the sender is a role address (orders@, noreply@ ...), and whether the mail\n\
         \x20   came through a mailing list, was automated, or is a reply.\n\
         Never the display names from the headers or the part of the sender's address before\n\
         the @; a name in the body that is in no header is sent as written.\n\
         {keeps}\n\
         `neomail redact ID` shows exactly what is sent for a message.\n"
    )
}

/// Asks once per host, unless `yes`; a yes is remembered. Without a terminal or with
/// `--no-input`, and no earlier yes, it is a usage error and nothing is sent.
fn ensure_consent(
    data_dir: &Path,
    host: &str,
    yes: bool,
    input: Input,
    messages: usize,
) -> Result<bool> {
    if consented(data_dir, host)? {
        return Ok(true);
    }
    if yes {
        remember_consent(data_dir, host)?;
        return Ok(true);
    }
    if !input.can_prompt() {
        return Err(usage(format!(
            "nothing was sent: {host} needs your consent once; run `neomail classify` on a \
             terminal, or pass --yes ({})",
            input.why_not()
        )));
    }
    eprint!("{}", consent_text(host, messages));
    let answer = secret::prompt_line(&format!("Send to {host}? [y/N] "))?;
    if matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes") {
        remember_consent(data_dir, host)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// One message to ask about: where its result goes, and the request, built before any is sent.
struct Job {
    id: MessageId,
    account: String,
    msg_key: String,
    received: Option<DateTime<Utc>>,
    /// The original subject, for the terminal's line; never sent.
    subject: String,
    request: Vec<u8>,
}

/// A message given by id that already has a result: printed, not asked again.
struct Known {
    id: MessageId,
    account: String,
    received: Option<DateTime<Utc>>,
    subject: String,
    result: Classification,
}

/// What a run will do.
#[derive(Default)]
struct Plan {
    jobs: Vec<Job>,
    known: Vec<Known>,
    /// Ids that are unknown or unreadable, with the reason.
    failures: Vec<String>,
    /// Messages without a result left outside the window or the cap, newest first.
    left: Vec<MessageId>,
}

/// What [`plan`] needs to read a message.
struct Reader<'a> {
    owners: &'a [&'a str],
    model: &'a str,
}

impl Reader<'_> {
    /// The message's key and request, or why it cannot be read. Records the key of a row cached
    /// before schema 5, so the next run finds its result without reading it.
    fn read(&self, cache: &mut Cache, id: MessageId) -> Result<std::result::Result<Job, String>> {
        let Some(stored) = cache.get(id)? else {
            return Ok(Err(format!(
                "no cached message with id {id}; see `neomail list`"
            )));
        };
        let Ok(parsed) = nm_core::parse::parse(&stored.raw) else {
            return Ok(Err(format!(
                "message {id} cannot be read; see `neomail show --raw {id}`"
            )));
        };
        let msg_key = nm_core::parse::msg_key(parsed.headers.message_id.as_deref(), &stored.raw);
        cache.set_msg_key(id, &msg_key)?;
        let outgoing = redact::outgoing(&parsed, self.owners);
        Ok(Ok(Job {
            id,
            account: stored.account,
            msg_key,
            received: stored.received.or(parsed.date),
            subject: parsed.subject.unwrap_or_default(),
            request: jev::request_body(self.model, &outgoing),
        }))
    }
}

/// Chooses the messages: the ids given, else the ones without a result received in the last
/// `days`, newest first, at most `limit` (`--all`: every one).
fn plan(
    cache: &mut Cache,
    args: &Args,
    ids: &[i64],
    reader: &Reader<'_>,
    now: DateTime<Utc>,
) -> Result<Plan> {
    let mut plan = Plan::default();
    // Each message once per account: a repeated id, or a copy of one message in two mailboxes,
    // is one request.
    let mut seen = std::collections::HashSet::new();
    if !ids.is_empty() {
        for &id in ids {
            match reader.read(cache, MessageId::new(id))? {
                Err(why) => plan.failures.push(why),
                Ok(job) if !seen.insert((job.account.clone(), job.msg_key.clone())) => {}
                Ok(job) => match cache.classification(&job.account, &job.msg_key)? {
                    Some(result) if !args.again => plan.known.push(Known {
                        id: job.id,
                        account: job.account,
                        received: job.received,
                        subject: job.subject,
                        result,
                    }),
                    _ => plan.jobs.push(job),
                },
            }
        }
        return Ok(plan);
    }
    let cutoff = now - TimeDelta::days(i64::from(args.days));
    let cap = if args.all {
        usize::MAX
    } else {
        usize::try_from(args.limit).unwrap_or(usize::MAX)
    };
    for pending in cache.pending(args.account.as_deref(), args.again)? {
        let in_window = args.all || pending.received.is_some_and(|r| r >= cutoff);
        if !in_window || plan.jobs.len() >= cap {
            // A row cached before schema 5 has no key yet; reading each would cost a parse.
            let first = pending
                .msg_key
                .is_none_or(|key| seen.insert((pending.account, key)));
            if first {
                plan.left.push(pending.id);
            }
            continue;
        }
        let Ok(job) = reader.read(cache, pending.id)? else {
            // Unreadable mail is never sent; `neomail show --raw` tells why.
            continue;
        };
        // A row cached before schema 5 learns its key only now: it may have a result already.
        if pending.msg_key.is_none()
            && !args.again
            && cache.classification(&job.account, &job.msg_key)?.is_some()
        {
            continue;
        }
        if seen.insert((job.account.clone(), job.msg_key.clone())) {
            plan.jobs.push(job);
        }
    }
    Ok(plan)
}

/// Input tokens per request, and how that was worked out.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PerRequest {
    tokens: f64,
    /// `true`: the average `input_tokens` of stored results; `false`: the request's characters
    /// divided by 4.
    measured: bool,
}

impl PerRequest {
    fn basis(self) -> &'static str {
        if self.measured {
            "the average input_tokens of the results stored so far"
        } else {
            "the requests' characters divided by 4"
        }
    }
}

/// Tokens per request: measured when results report it, else estimated from `requests`.
fn per_request(measured: Option<f64>, requests: &[&[u8]]) -> Option<PerRequest> {
    if let Some(tokens) = measured.filter(|t| t.is_finite() && *t > 0.0) {
        return Some(PerRequest {
            tokens,
            measured: true,
        });
    }
    if requests.is_empty() {
        return None;
    }
    let chars: usize = requests
        .iter()
        .map(|r| String::from_utf8_lossy(r).chars().count())
        .sum();
    Some(PerRequest {
        tokens: chars as f64 / 4.0 / requests.len() as f64,
        measured: false,
    })
}

/// What a number of requests would cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
struct Estimate {
    messages: usize,
    requests: usize,
    input_tokens: u64,
    /// In US dollars; `None` for a backend other than TypeSafe's, whose price is unknown.
    cost_usd: Option<f64>,
}

fn estimate(messages: usize, per: Option<PerRequest>, priced: bool) -> Estimate {
    let tokens = per.map_or(0.0, |p| p.tokens * messages as f64).round();
    // Rounded and non-negative, so the cast is exact up to 2^53.
    let input_tokens = tokens.max(0.0) as u64;
    Estimate {
        messages,
        requests: messages,
        input_tokens,
        cost_usd: priced.then(|| tokens / 1_000_000.0 * USD_PER_MILLION_INPUT),
    }
}

/// What following [`take_all_command`] would send, `messages` requests' worth: it is quoted
/// whole, never only the part this run left out.
fn take_all(args: &Args, messages: usize, per: Option<PerRequest>, priced: bool) -> String {
    let what = if args.again {
        "ask about every message again"
    } else {
        "send every message without a group"
    };
    format!(
        "`{}` would {what}: {}",
        take_all_command(args),
        describe(&estimate(messages, per, priced))
    )
}

/// The command that takes what this run left out: the same selection without the window
/// or the cap, so following it never widens the run to other accounts. It sends every message
/// of the selection that has no result yet, those this run failed or never asked included;
/// with `--again`, every message of the selection again.
fn take_all_command(args: &Args) -> String {
    let mut command = String::from("neomail classify --all");
    if let Some(account) = args.account.as_deref() {
        command.push_str(" --account ");
        command.push_str(&shell_quote(account));
    }
    if args.again {
        command.push_str(" --again");
    }
    command
}

/// `word` as one shell word: as it is when it holds only safe characters, else in single quotes.
fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `12 messages, 12 requests, about 4,200 input tokens, about $0.0002`.
fn describe(e: &Estimate) -> String {
    let mut text = format!(
        "{} message{}, {} request{}, about {} input tokens",
        thousands(e.messages as u64),
        if e.messages == 1 { "" } else { "s" },
        thousands(e.requests as u64),
        if e.requests == 1 { "" } else { "s" },
        thousands(e.input_tokens)
    );
    if let Some(cost) = e.cost_usd {
        let digits = if cost >= 0.01 || cost == 0.0 { 2 } else { 4 };
        text.push_str(&format!(
            ", about ${cost:.digits$} at ${USD_PER_MILLION_INPUT} per million input tokens"
        ));
    }
    text
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `neomail classify`.
pub fn run(
    args: &Args,
    ids: Vec<i64>,
    config: Option<PathBuf>,
    input: Input,
    terminal: bool,
    colors: Colors,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let mode = args.format.mode(terminal);
    // The owner's addresses are masked whole: a config the user named must load.
    let loaded = config::load_optional(config.clone(), true)?;
    let settings = loaded
        .as_ref()
        .map(|c| c.classify.clone())
        .unwrap_or_default();
    let owners = loaded
        .as_ref()
        .map(redact::owner_addresses)
        .unwrap_or_default();
    let Some(mut cache) = crate::open_cache(false)? else {
        bail!("{}", crate::no_cache_hint(config));
    };
    if let Some(name) = args.account.as_deref() {
        crate::check_cached_account(&cache, name)?;
    }
    let host = settings.host()?;
    let now = Utc::now();
    let reader = Reader {
        owners: &owners,
        model: &settings.model,
    };
    let plan = plan(&mut cache, args, &ids, &reader, now)?;
    let per = per_request_of(&mut cache, &plan, &reader)?;
    let priced = host == TYPESAFE_HOST;
    let todo = estimate(plan.jobs.len(), per, priced);
    let left = estimate(plan.left.len(), per, priced);
    if args.estimate {
        // Nothing is sent, so the command would send this run's messages too.
        let take = take_all(args, todo.messages + left.messages, per, priced);
        write_estimate(out, mode, &todo, &left, per, &take)?;
        return finish_failures(&plan.failures, colors, false);
    }
    let asked = plan.jobs.len();
    let mut printer = Printer::new(mode, out, colors);
    for known in &plan.known {
        printer.result(
            known.id,
            &known.account,
            &known.subject,
            known.received,
            &known.result,
            &settings.rules,
            now,
        )?;
    }
    let mut outcome = Outcome::default();
    if !plan.jobs.is_empty() {
        // The keyring before any runtime: zbus runs one of its own (keyring.rs).
        let store = Keyring::open_for_jev().ok();
        let key = keyring::jev_key(store.as_ref(), &host, std::env::var(keyring::JEV_ENV).ok());
        drop(store);
        let Some((key, _)) = key else {
            printer.finish()?;
            bail!("{}", keyring::NO_JEV_KEY);
        };
        let data_dir = config::data_dir()?;
        if !ensure_consent(&data_dir, &host, args.yes, input, plan.jobs.len())? {
            printer.finish()?;
            eprintln!("neomail: nothing was sent");
            return Ok(ExitCode::from(EXIT_FAILURE));
        }
        outcome = match send(&settings, &key, plan.jobs, &mut cache, &mut printer, now) {
            Ok(outcome) => outcome,
            Err(e) => {
                // --json still prints the results that were stored. After a failed print this
                // fails too, and the first error is the one to report (a closed pipe is quiet).
                let _ = printer.finish();
                return Err(e);
            }
        };
    } else if plan.known.is_empty() && plan.failures.is_empty() {
        eprintln!("neomail: nothing to classify");
    }
    printer.finish()?;
    if outcome.classified > 0 || outcome.failed > 0 {
        eprintln!(
            "neomail: classified {}{}",
            outcome.classified,
            if outcome.failed > 0 {
                format!(", {} failed", outcome.failed)
            } else {
                String::new()
            }
        );
    }
    if let Some(why) = &outcome.stopped {
        eprintln!(
            "{}: the run stopped: {why}; the {} result{} received were stored",
            colors.stderr.error("neomail"),
            outcome.classified,
            if outcome.classified == 1 { "" } else { "s" }
        );
    }
    if !plan.left.is_empty() {
        // This run's messages that have no result yet: they failed, or the run stopped first.
        let unfinished = asked - outcome.classified;
        let (sends, also) = match (args.again, unfinished) {
            // --again asks about every message again, this run's included.
            (true, _) => (asked + plan.left.len(), String::new()),
            (false, 0) => (plan.left.len(), String::new()),
            (false, n) => (
                plan.left.len() + n,
                format!(
                    ", and {n} that failed or {} not asked",
                    if n == 1 { "was" } else { "were" }
                ),
            ),
        };
        eprintln!(
            "neomail: left outside the {}-day window or the {}-message cap: {} message{}{also}; \
             {}{}",
            args.days,
            thousands(u64::from(args.limit)),
            thousands(plan.left.len() as u64),
            if plan.left.len() == 1 { "" } else { "s" },
            take_all(args, sends, per, priced),
            per.map_or(String::new(), |p| format!(" (tokens from {})", p.basis())),
        );
    }
    let failed = outcome.failed > 0 || outcome.stopped.is_some();
    finish_failures(&plan.failures, colors, failed)
}

/// Reports the ids that could not be read; exit 1 when there were any or `failed`.
fn finish_failures(failures: &[String], colors: Colors, failed: bool) -> Result<ExitCode> {
    for why in failures {
        eprintln!("{}: {why}", colors.stderr.error("neomail"));
    }
    Ok(if failed || !failures.is_empty() {
        ExitCode::from(EXIT_FAILURE)
    } else {
        ExitCode::SUCCESS
    })
}

/// Tokens per request: measured from stored results, else from this run's requests, else from
/// a sample of the messages left out.
fn per_request_of(
    cache: &mut Cache,
    plan: &Plan,
    reader: &Reader<'_>,
) -> Result<Option<PerRequest>> {
    let measured = cache.average_input_tokens()?;
    let mut requests: Vec<Vec<u8>> = Vec::new();
    if measured.is_none() && plan.jobs.is_empty() {
        for &id in plan.left.iter().take(SAMPLE) {
            if let Ok(job) = reader.read(cache, id)? {
                requests.push(job.request);
            }
        }
    }
    let views: Vec<&[u8]> = if plan.jobs.is_empty() {
        requests.iter().map(Vec::as_slice).collect()
    } else {
        plan.jobs.iter().map(|j| j.request.as_slice()).collect()
    };
    Ok(per_request(measured, &views))
}

/// `--estimate`'s output: lines for people and pipes, one object for scripts.
fn write_estimate(
    out: &mut dyn Write,
    mode: Mode,
    todo: &Estimate,
    left: &Estimate,
    per: Option<PerRequest>,
    take_all: &str,
) -> Result<()> {
    #[derive(Serialize)]
    struct Shown<'a> {
        #[serde(flatten)]
        todo: &'a Estimate,
        /// `measured` (stored results) or `characters` (characters / 4).
        tokens_from: Option<&'static str>,
        left: &'a Estimate,
    }
    let shown = Shown {
        todo,
        tokens_from: per.map(|p| if p.measured { "measured" } else { "characters" }),
        left,
    };
    match mode {
        Mode::Json => output::write_json(out, &shown)?,
        Mode::Jsonl => output::write_jsonl(out, [&shown])?,
        Mode::Human | Mode::Plain => {
            writeln!(out, "would send: {}", describe(todo))?;
            if left.messages > 0 {
                writeln!(
                    out,
                    "left outside the window or the cap: {}",
                    describe(left)
                )?;
                writeln!(out, "{take_all}")?;
            }
            if let Some(p) = per {
                writeln!(out, "tokens from: {}", p.basis())?;
            }
            // A note, not a result: stdout carries only the estimate.
            eprintln!("neomail: nothing was sent");
        }
    }
    Ok(())
}

/// One result for scripts: jev's answer and the group it lists under, or why it failed.
#[derive(Serialize)]
struct ResultItem<'a> {
    id: MessageId,
    account: &'a str,
    group: Option<&'static str>,
    choice: Option<&'static str>,
    confidence: Option<f64>,
    #[serde(serialize_with = "serialize_probabilities")]
    probabilities: &'a [(Choice, f64)],
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// The probabilities as one object in the options' fixed order; `null` when there are none.
fn serialize_probabilities<S: serde::Serializer>(
    p: &&[(Choice, f64)],
    s: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    if p.is_empty() {
        return s.serialize_none();
    }
    let mut map = s.serialize_map(Some(p.len()))?;
    for (choice, probability) in p.iter() {
        map.serialize_entry(choice.name(), probability)?;
    }
    map.end()
}

/// Prints results as they arrive: a line each on a terminal, TSV in a pipe, JSON Lines as they
/// come, or one JSON array at the end. Progress goes to standard error on a terminal.
struct Printer<'a> {
    mode: Mode,
    out: &'a mut dyn Write,
    colors: Colors,
    /// `--json`'s items, each serialized as it arrives so their keys keep the struct's order.
    array: Vec<String>,
    progress: bool,
    shown: bool,
}

impl<'a> Printer<'a> {
    fn new(mode: Mode, out: &'a mut dyn Write, colors: Colors) -> Self {
        Self {
            mode,
            out,
            colors,
            array: Vec::new(),
            progress: io::stderr().is_terminal(),
            shown: false,
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one result's fields")]
    fn result(
        &mut self,
        id: MessageId,
        account: &str,
        subject: &str,
        received: Option<DateTime<Utc>>,
        result: &Classification,
        rules: &GroupRules,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let group = rules.effective(result.choice, result.confidence, received, now);
        let item = ResultItem {
            id,
            account,
            group: Some(group.name()),
            choice: Some(result.choice.name()),
            confidence: Some(result.confidence),
            probabilities: &result.probabilities,
            model: Some(&result.model),
            error: None,
        };
        self.write(&item, group, subject)
    }

    fn failed(&mut self, id: MessageId, account: &str, why: &str) -> Result<()> {
        let item = ResultItem {
            id,
            account,
            group: None,
            choice: None,
            confidence: None,
            probabilities: &[],
            model: None,
            error: Some(why),
        };
        match self.mode {
            Mode::Json | Mode::Jsonl => self.write(&item, Group::Other, ""),
            Mode::Human | Mode::Plain => {
                self.clear();
                eprintln!(
                    "{}: message {id} ({}): {why}",
                    self.colors.stderr.error("neomail"),
                    output::one_line(account)
                );
                Ok(())
            }
        }
    }

    fn write(&mut self, item: &ResultItem<'_>, group: Group, subject: &str) -> Result<()> {
        self.clear();
        let confidence = format!("{:.2}", item.confidence.unwrap_or(0.0));
        match self.mode {
            Mode::Json => self.array.push(serde_json::to_string_pretty(item)?),
            Mode::Jsonl => output::write_jsonl(&mut *self.out, [item])?,
            Mode::Plain => {
                let paint = self.colors.plain;
                writeln!(
                    self.out,
                    "{}\t{}\t{}\t{confidence}",
                    paint.id(&item.id.to_string()),
                    paint.account(&output::one_line(item.account)),
                    group.name()
                )?;
            }
            Mode::Human => {
                let paint = self.colors.stdout;
                writeln!(
                    self.out,
                    "{} {}  {}  {confidence}  {}",
                    paint.id(&item.id.to_string()),
                    paint.account(&output::one_line(item.account)),
                    paint.strong(&format!("{:<17}", group.name())),
                    output::one_line(subject)
                )?;
            }
        }
        self.out.flush()?;
        Ok(())
    }

    fn progress(&mut self, done: usize, total: usize) {
        if self.progress && self.mode == Mode::Human {
            eprint!("\r\x1b[2Kneomail: classifying {done}/{total}");
            let _ = io::stderr().flush();
            self.shown = true;
        }
    }

    fn clear(&mut self) {
        if self.shown {
            eprint!("\r\x1b[2K");
            self.shown = false;
        }
    }

    fn finish(&mut self) -> Result<()> {
        self.clear();
        if self.mode == Mode::Json {
            // As `output::write_json` prints an array; a string in JSON holds no raw newline,
            // so indenting each line indents the item.
            if self.array.is_empty() {
                writeln!(self.out, "[]")?;
            } else {
                writeln!(self.out, "[")?;
                let last = self.array.len() - 1;
                for (n, item) in self.array.iter().enumerate() {
                    for (k, line) in item.lines().enumerate() {
                        if k > 0 {
                            writeln!(self.out)?;
                        }
                        write!(self.out, "  {line}")?;
                    }
                    writeln!(self.out, "{}", if n == last { "" } else { "," })?;
                }
                writeln!(self.out, "]")?;
            }
        }
        self.out.flush()?;
        Ok(())
    }
}

/// How a run went.
#[derive(Debug, Default)]
struct Outcome {
    classified: usize,
    failed: usize,
    /// Why the run stopped early, if it did.
    stopped: Option<String>,
}

/// What the workers share. The requests are built; the workers only send them.
struct Shared {
    http: Https,
    endpoint: String,
    authorization: String,
    requests: Vec<Vec<u8>>,
    queue: Mutex<VecDeque<usize>>,
    stop: AtomicBool,
}

enum Event {
    Answered(usize, Answer),
    Failed(usize, String),
    Stop(String),
    /// A message failed in a way every other message would too, so the run stops: it ran out
    /// of tries on a retry status (a rate limit or an outage that outlasts one message's tries
    /// would cost every other message as many requests), or a 2xx answer is not jev's.
    GaveUp(usize, String),
    /// The run had already stopped, so this message was left unasked.
    Stopped,
}

/// Sends the requests, [`CONCURRENCY`] at a time, storing each result as soon as it arrives.
fn send(
    settings: &ClassifySettings,
    key: &Secret,
    mut jobs: Vec<Job>,
    cache: &mut Cache,
    printer: &mut Printer<'_>,
    now: DateTime<Utc>,
) -> Result<Outcome> {
    let total = jobs.len();
    let shared = Arc::new(Shared {
        http: Https::new()?,
        endpoint: jev::endpoint(&settings.url),
        authorization: format!("Bearer {}", key.expose()),
        requests: jobs
            .iter_mut()
            .map(|j| std::mem::take(&mut j.request))
            .collect(),
        queue: Mutex::new((0..total).collect()),
        stop: AtomicBool::new(false),
    });
    let runtime = crate::account::runtime()?;
    runtime.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..CONCURRENCY.min(total) {
            tokio::spawn(worker(shared.clone(), tx.clone()));
        }
        drop(tx);
        let mut outcome = Outcome::default();
        // The first error storing or printing a result. It stops the run, but the answers
        // already on their way are still stored: each cost a request.
        let mut error: Option<anyhow::Error> = None;
        let stop = &shared.stop;
        printer.progress(0, total);
        while let Some(event) = rx.recv().await {
            // Once printing failed (a closed pipe), nothing more is printed.
            let print = error.is_none();
            match event {
                Event::Answered(i, answer) => {
                    let job = &jobs[i];
                    let result = Classification {
                        choice: answer.choice,
                        confidence: answer.confidence,
                        probabilities: answer.probabilities,
                        model: answer.model,
                        request_version: jev::REQUEST_VERSION,
                        input_tokens: answer.input_tokens,
                        classified_at: Utc::now(),
                    };
                    let stored = cache.store_classification(&job.account, &job.msg_key, &result);
                    if let Err(e) = stored {
                        keep(&mut error, stop, Err(e.into()));
                        continue;
                    }
                    outcome.classified += 1;
                    if print {
                        let printed = printer.result(
                            job.id,
                            &job.account,
                            &job.subject,
                            job.received,
                            &result,
                            &settings.rules,
                            now,
                        );
                        keep(&mut error, stop, printed);
                    }
                }
                Event::Failed(i, why) => {
                    outcome.failed += 1;
                    if print {
                        let printed = printer.failed(jobs[i].id, &jobs[i].account, &why);
                        keep(&mut error, stop, printed);
                    }
                }
                Event::Stop(why) => {
                    outcome.stopped.get_or_insert(why);
                }
                Event::GaveUp(i, why) => {
                    outcome.failed += 1;
                    if print {
                        let printed = printer.failed(jobs[i].id, &jobs[i].account, &why);
                        keep(&mut error, stop, printed);
                    }
                    outcome.stopped.get_or_insert(why);
                }
                Event::Stopped => {}
            }
            if error.is_none() {
                printer.progress(outcome.classified + outcome.failed, total);
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok(outcome),
        }
    })
}

/// Keeps the first error storing or printing a result, and stops the run on it.
fn keep(error: &mut Option<anyhow::Error>, stop: &AtomicBool, done: Result<()>) {
    if let Err(e) = done {
        stop.store(true, Ordering::Relaxed);
        error.get_or_insert(e);
    }
}

/// Takes requests off the queue until it is empty or the run stops.
async fn worker(shared: Arc<Shared>, tx: tokio::sync::mpsc::UnboundedSender<Event>) {
    while !shared.stop.load(Ordering::Relaxed) {
        let next = shared.queue.lock().ok().and_then(|mut q| q.pop_front());
        let Some(i) = next else { break };
        let event = ask(&shared, i).await;
        if matches!(event, Event::Stop(_) | Event::GaveUp(..)) {
            shared.stop.store(true, Ordering::Relaxed);
        }
        if tx.send(event).is_err() {
            break;
        }
    }
}

/// One message's request, tried up to [`jev::MAX_TRIES`] times while jev says to retry.
async fn ask(shared: &Shared, i: usize) -> Event {
    let headers = [
        ("Authorization", shared.authorization.as_str()),
        ("Accept", "application/json"),
    ];
    let mut attempt = 1;
    loop {
        let sent = shared
            .http
            .post_json(
                &shared.endpoint,
                &headers,
                &shared.requests[i],
                jev::MAX_ANSWER,
            )
            .await;
        let response = match sent {
            Ok(response) => response,
            // The backend cannot be reached: every other request would fail the same way.
            Err(e) => return Event::Stop(format!("{e:#}")),
        };
        let status = response.status;
        match jev::verdict(
            status,
            response.retry_after,
            &response.body,
            attempt,
            jitter(),
        ) {
            Verdict::Answer => {
                return match jev::parse_answer(&response.body) {
                    Ok(answer) => Event::Answered(i, answer),
                    // Most likely classify.url names some other service: every other message
                    // would be sent to it for nothing.
                    Err(e) => Event::GaveUp(i, format!("the backend's answer is not jev's: {e}")),
                };
            }
            Verdict::Retry(wait) if attempt < jev::MAX_TRIES => {
                tokio::time::sleep(wait).await;
                // Another worker may have stopped the run while this one waited.
                if shared.stop.load(Ordering::Relaxed) {
                    return Event::Stopped;
                }
                attempt += 1;
            }
            Verdict::Retry(_) => {
                return Event::GaveUp(
                    i,
                    format!("jev answered {status} {attempt} times; try again later"),
                );
            }
            Verdict::Skip => {
                return Event::Failed(i, "jev refused the request as invalid (422)".to_owned());
            }
            Verdict::StopKey => {
                return Event::Stop(format!(
                    "jev refused the key ({status}): {}",
                    keyring::NO_JEV_KEY.trim_start_matches("no jev key: ")
                ));
            }
            Verdict::Stop { status, body } => {
                return Event::Stop(format!("jev answered {status}: {body}"));
            }
        }
    }
}

/// A number from 0 to 1 to spread retries, from the system's random source.
fn jitter() -> f64 {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 4];
    match ring::rand::SystemRandom::new().fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX),
        Err(_) => 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_name_in_the_hint_is_one_shell_word() {
        assert_eq!(shell_quote("work.mail@acme"), "work.mail@acme");
        assert_eq!(shell_quote("my work"), "'my work'");
        assert_eq!(shell_quote("bob's"), "'bob'\\''s'");
        assert_eq!(shell_quote("$(x)"), "'$(x)'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn the_estimate_prices_input_tokens_on_typesafe_only() {
        let per = Some(PerRequest {
            tokens: 500.0,
            measured: true,
        });
        let e = estimate(2_000, per, true);
        assert_eq!(
            (e.messages, e.requests, e.input_tokens),
            (2_000, 2_000, 1_000_000)
        );
        assert!((e.cost_usd.unwrap() - 0.042).abs() < 1e-12);
        assert_eq!(
            describe(&e),
            "2,000 messages, 2,000 requests, about 1,000,000 input tokens, about $0.04 at $0.042 \
             per million input tokens"
        );
        assert_eq!(estimate(2_000, per, false).cost_usd, None);
        assert_eq!(estimate(0, None, true).input_tokens, 0);
    }

    #[test]
    fn tokens_are_measured_when_results_report_them_else_characters_over_4() {
        // 400 and 800 characters (1,600 bytes): 150 tokens a request.
        let wide = "é".repeat(800);
        let requests: [&[u8]; 2] = [&[b'x'; 400], wide.as_bytes()];
        let guessed = per_request(None, &requests).unwrap();
        assert_eq!((guessed.tokens, guessed.measured), (150.0, false));
        let measured = per_request(Some(320.0), &requests).unwrap();
        assert_eq!((measured.tokens, measured.measured), (320.0, true));
        assert_eq!(per_request(None, &[]), None);
    }

    #[test]
    fn consent_is_remembered_per_host() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let input = Input {
            terminal: false,
            no_input: false,
        };
        let refused = ensure_consent(&data, "api.typesafe.ai", false, input, 3);
        assert!(refused.unwrap_err().is::<crate::UsageError>());
        assert!(ensure_consent(&data, "api.typesafe.ai", true, input, 3).unwrap());
        assert!(ensure_consent(&data, "api.typesafe.ai", false, input, 3).unwrap());
        assert!(ensure_consent(&data, "127.0.0.1", false, input, 3).is_err());
    }

    #[test]
    fn the_consent_text_names_the_host_what_is_sent_and_the_terms() {
        let text = consent_text("api.typesafe.ai", 12);
        // Line breaks fall where the width needs them.
        let flat = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for part in [
            "to api.typesafe.ai, 12 now",
            "tried again up to twice on a rate limit or a server error",
            "the sender's domain",
            "never the display names from the headers or the part of the sender's address before \
             the @; a name in the body that is in no header is sent as written",
            "the part of each email address before the @",
            "links (their host stays)",
            "phone, card and IBAN numbers, amounts, one-time codes, runs of 4 or more digits",
            "shorter numbers, postal addresses and names that are in no header are sent as written",
            TYPESAFE_PRIVACY,
            TYPESAFE_TERMS,
            "`neomail redact ID`",
        ] {
            assert!(flat.contains(&part.to_lowercase()), "{part}: {text}");
        }
        // A name in the body that no header holds is sent.
        assert!(!flat.contains("never the sender's name"), "{text}");
        assert!(!consent_text("127.0.0.1", 1).contains(TYPESAFE_TERMS));
    }
}
