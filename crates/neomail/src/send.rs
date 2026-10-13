//! `neomail send`, `reply` and `forward`: a Markdown body becomes a message
//! ([`nm_core::compose`]), shown and confirmed, then sent from one account: over SMTP for IMAP
//! accounts ([`nm_imap::smtp`]), through Microsoft Graph's sendMail for Outlook.
//!
//! - Nothing is sent without a yes: asked on a terminal, `--yes` from a script; otherwise a usage
//!   error (exit 2) and no connection. `--dry-run` prints the message and connects to nothing.
//! - Exit 0 means the server accepted the message. A copy in Sent that could not be stored is a
//!   warning, not a failure, so a retry never sends it twice. Exit 1 means it was not sent, except
//!   when the server never answered the whole message (or Graph failed with a 5xx): that error
//!   says it may have been sent.
//! - One identity per account: its `address` (else its login) and `display_name`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use nm_core::MessageId;
use nm_core::compose::{self, ComposeError, Draft, Mailbox, Outgoing};
use nm_core::parse::ParsedMessage;
use serde::Serialize;

use crate::config::{self, Account, Config, Login, Outbound, Server};
use crate::keyring::Keyring;
use crate::output::{self, Mode};
use crate::secret::{self, Input};
use crate::sync::{Credential, Logins};
use crate::usage;

pub const SEND_EXAMPLES: &str = "\
Examples:
  neomail send --to ada@example.com --subject Hello --body note.md
                                    Send note.md, asking first
  printf 'Hi **Ada**\\n' | neomail send --from work --to ada@example.com --subject Hi \\
      --body - --yes --json         From a script: the body on standard input, no question
  neomail send --to ada@example.com --subject Hi --body note.md --dry-run
                                    Print the message that would be sent; send nothing

The body is Markdown: it is sent as HTML rendered from it, with the Markdown itself as the plain
text part (--text-only sends the plain text alone). HTML typed into the Markdown is sent as text,
never as HTML. When standard input is a terminal, neomail shows who the message goes to and asks.
When it is not (for example a body piped to --body -), or with --no-input, pass --yes, or nothing
is sent and it exits 2. --dry-run prints the message, with the summary (Bcc included) on standard
error, and sends nothing. Exit 1 when the question was answered no, or the server refused the
message or a recipient, or the login failed: nothing was sent to anyone then. Exit 1 too when the
connection was lost or timed out after the whole message went out, or Microsoft Graph failed with
a 5xx: then the error says it may have been sent, so check Sent before sending it again. Exit 0
means the server accepted it; a copy in Sent that could not be stored is a warning. An Outlook
account added before neomail could send needs `neomail account login NAME` once. Attachments are
not supported yet.";

pub const REPLY_EXAMPLES: &str = "\
Examples:
  neomail reply 42 --body answer.md Reply to the sender of message 42
  neomail reply 42 --all --body answer.md
                                    Reply to the sender and everyone in To and Cc
  printf 'Thanks!\\n' | neomail reply 42 --body - --yes
                                    Reply from a script

ID is the first column of `neomail list`. The reply goes to the original's Reply-To, else its
From (its To when the original is yours); --all adds its To and Cc, never you. If any of those
addresses cannot be sent to yet (one that is not ASCII), nothing is sent and it exits 1; no one
is left out quietly. It is sent from the account the original is in unless --from says
otherwise, with the subject `Re: ` and the original quoted below your text, and threads with it
(In-Reply-To and References). Asking, --yes and exit codes are as for `neomail send`.";

pub const FORWARD_EXAMPLES: &str = "\
Examples:
  neomail forward 42 --to ada@example.com
                                    Forward message 42, asking first
  neomail forward 42 --to ada@example.com --body note.md
                                    With a note of yours above it
  neomail forward 42 --to ada@example.com --yes --json
                                    From a script

The original's From, Date, Subject and To, then its text, go below your note, with the subject
`Fwd: `. Its attachments are not forwarded yet: neomail warns when it had any. It is sent from
the account the original is in unless --from says otherwise. Asking, --yes and exit codes are as
for `neomail send`.";

#[derive(clap::Args)]
pub struct SendArgs {
    /// Recipients: an address or `Name <address>`; repeat the flag or separate them with commas
    #[arg(long, value_name = "ADDR", required = true)]
    to: Vec<String>,
    /// The subject
    #[arg(long, value_name = "TEXT")]
    subject: String,
    /// The body: a Markdown file, or - for standard input
    #[arg(long, value_name = "PATH")]
    body: PathBuf,
    #[command(flatten)]
    common: Common,
}

#[derive(clap::Args)]
pub struct ReplyArgs {
    /// The message to answer: its id, the first column of `neomail list`
    #[arg(value_name = "ID", value_parser = clap::value_parser!(i64).range(1..))]
    id: i64,
    /// Reply to everyone: also the original's To and Cc, except you
    #[arg(long)]
    all: bool,
    /// The body: a Markdown file, or - for standard input
    #[arg(long, value_name = "PATH")]
    body: PathBuf,
    #[command(flatten)]
    common: Common,
}

#[derive(clap::Args)]
pub struct ForwardArgs {
    /// The message to forward: its id, the first column of `neomail list`
    #[arg(value_name = "ID", value_parser = clap::value_parser!(i64).range(1..))]
    id: i64,
    /// Recipients: an address or `Name <address>`; repeat the flag or separate them with commas
    #[arg(long, value_name = "ADDR", required = true)]
    to: Vec<String>,
    /// A note above the forwarded message: a Markdown file, or - for standard input
    #[arg(long, value_name = "PATH")]
    body: Option<PathBuf>,
    #[command(flatten)]
    common: Common,
}

/// The flags `send`, `reply` and `forward` share.
#[derive(clap::Args)]
pub struct Common {
    /// The account to send from, by name or address [default: the original's, else the only one]
    #[arg(long, value_name = "ACCOUNT")]
    from: Option<String>,
    /// Copy to these addresses; repeat the flag or separate them with commas
    #[arg(long, value_name = "ADDR")]
    cc: Vec<String>,
    /// Blind copy: these addresses get the message, but no header names them
    #[arg(long, value_name = "ADDR")]
    bcc: Vec<String>,
    /// Send the Markdown as plain text only, without the HTML part
    #[arg(long)]
    text_only: bool,
    /// Print the message that would be sent, and send nothing: no connection is made
    #[arg(long, conflicts_with_all = ["json", "jsonl", "plain"])]
    dry_run: bool,
    /// Send without asking (for scripts)
    #[arg(long)]
    yes: bool,
    /// Print what was sent as a JSON object (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
    #[arg(long, conflicts_with = "jsonl")]
    json: bool,
    /// Print what was sent as one JSON line
    #[arg(long, conflicts_with = "plain")]
    jsonl: bool,
    /// Print the Message-ID alone, as in a pipe, even on a terminal
    #[arg(long, conflicts_with = "json")]
    plain: bool,
}

/// Where the commands read their settings and the cache, and how they may ask and print.
pub struct Env<'a> {
    pub config: Option<PathBuf>,
    pub input: Input,
    /// Standard output is a terminal.
    pub terminal: bool,
    pub out: &'a mut dyn Write,
}

/// `neomail send`.
pub fn send(args: SendArgs, cx: Env<'_>) -> Result<ExitCode> {
    let settings = Config::load(&config::config_path(cx.config.clone())?)?;
    let account = pick(&settings, args.common.from.as_deref(), None)?;
    let me = account.identity()?;
    if args.subject.chars().any(char::is_control) {
        return Err(usage(
            "--subject: it holds a line break or a control character",
        ));
    }
    let to = mailboxes("--to", &args.to)?;
    if to.is_empty() {
        return Err(usage("--to: no address given"));
    }
    let markdown = read_body(&args.body)?;
    let draft = Draft {
        from: me,
        to,
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: args.subject,
        markdown,
        text_only: false,
        in_reply_to: None,
        references: Vec::new(),
    };
    deliver(account, draft, &args.common, cx)
}

/// `neomail reply ID [--all]`.
pub fn reply(args: ReplyArgs, cx: Env<'_>) -> Result<ExitCode> {
    let settings = Config::load(&config::config_path(cx.config.clone())?)?;
    let (stored_account, original) = cached(args.id, cx.config.clone())?;
    let account = pick(
        &settings,
        args.common.from.as_deref(),
        Some(&stored_account),
    )?;
    let me = account.identity()?;
    let markdown = read_body(&args.body)?;
    let draft = compose::reply(&original, args.all, &me, &markdown).map_err(|e| match e {
        ComposeError::NoRecipients => anyhow!(
            "message {} has no one to reply to but you; use `neomail send --to ADDR` instead",
            args.id
        ),
        e => anyhow!("message {} cannot be answered: {e}", args.id),
    })?;
    deliver(account, draft, &args.common, cx)
}

/// `neomail forward ID --to ADDR`.
pub fn forward(args: ForwardArgs, cx: Env<'_>) -> Result<ExitCode> {
    let settings = Config::load(&config::config_path(cx.config.clone())?)?;
    let (stored_account, original) = cached(args.id, cx.config.clone())?;
    let account = pick(
        &settings,
        args.common.from.as_deref(),
        Some(&stored_account),
    )?;
    let me = account.identity()?;
    let to = mailboxes("--to", &args.to)?;
    if to.is_empty() {
        return Err(usage("--to: no address given"));
    }
    let markdown = match &args.body {
        Some(path) => read_body(path)?,
        None => String::new(),
    };
    let attachments = original.attachments.len();
    if attachments > 0 {
        let s = if attachments == 1 { "" } else { "s" };
        eprintln!(
            "neomail: warning: message {} has {attachments} attachment{s}, which neomail cannot \
             forward yet; only its text is forwarded",
            args.id
        );
    }
    let draft = compose::forward(&original, &me, to, &markdown);
    deliver(account, draft, &args.common, cx)
}

/// The cached message `id`: its account's name and the message, parsed.
fn cached(id: i64, config: Option<PathBuf>) -> Result<(String, ParsedMessage)> {
    let Some(cache) = crate::open_cache(false)? else {
        bail!("{}", crate::no_cache_hint(config));
    };
    let Some(stored) = cache.get(MessageId::new(id))? else {
        bail!("no cached message {id}; `neomail list` shows the ids");
    };
    let original = nm_core::parse::parse(&stored.raw)
        .with_context(|| format!("message {id} cannot be read"))?;
    Ok((stored.account, original))
}

/// The account to send from: `--from` (a name, else an address), else the original's account,
/// else the only one.
fn pick<'c>(
    settings: &'c Config,
    from: Option<&str>,
    original: Option<&str>,
) -> Result<&'c Account> {
    let names = || {
        settings
            .accounts
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let accounts = &settings.accounts;
    if accounts.is_empty() {
        bail!("no accounts yet; {}", config::ADD_HINT);
    }
    if let Some(from) = from {
        let by_name = accounts.iter().find(|a| a.name == from);
        let by_address = || {
            accounts
                .iter()
                .find(|a| a.identity().is_ok_and(|me| me.is(from)))
        };
        return by_name.or_else(by_address).ok_or_else(|| {
            usage(format!(
                "--from {from:?}: no account has that name or address; the accounts are {}",
                names()
            ))
        });
    }
    if let Some(name) = original {
        return accounts.iter().find(|a| a.name == name).ok_or_else(|| {
            anyhow!(
                "the message is in the account {name:?}, which is no longer in the config; pass \
                 --from with one of {}",
                names()
            )
        });
    }
    match accounts.as_slice() {
        [only] => Ok(only),
        _ => Err(usage(format!(
            "--from: which account should send it? Pass --from with one of {}",
            names()
        ))),
    }
}

/// The addresses of one flag, checked; a bad one is a usage error naming the flag.
fn mailboxes(flag: &str, values: &[String]) -> Result<Vec<Mailbox>> {
    values
        .iter()
        .flat_map(|v| split_addresses(v))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| Mailbox::parse(v).map_err(|e| usage(format!("{flag}: {e}"))))
        .collect()
}

/// One flag's value split at its commas, except those inside double quotes or `<...>`, so
/// `"Doe, Jane" <jane@example.com>, bob@example.com` is two addresses.
fn split_addresses(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut angle, mut escaped) = (0, false, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' if !quoted && !angle => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

/// The Markdown body: a file, or standard input for `-`. It must be UTF-8.
fn read_body(path: &Path) -> Result<String> {
    let (bytes, what) = if path == Path::new("-") {
        let mut bytes = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut bytes)
            .context("cannot read the body from standard input")?;
        (bytes, "standard input".to_owned())
    } else {
        let bytes =
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        (bytes, path.display().to_string())
    };
    String::from_utf8(bytes).map_err(|_| anyhow!("the body in {what} is not UTF-8 text"))
}

/// Where the copy in Sent is: kept by the server itself (Gmail, Bridge, Graph), appended by
/// neomail, or missing because the append failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum SentCopy {
    Server,
    Appended,
    Failed,
}

/// An address as every JSON output has it (docs/json.md).
#[derive(Serialize)]
struct Address<'a> {
    name: Option<&'a str>,
    address: &'a str,
}

impl<'a> From<&'a Mailbox> for Address<'a> {
    fn from(m: &'a Mailbox) -> Self {
        Self {
            name: m.name(),
            address: m.address(),
        }
    }
}

/// What `--json` prints for a sent message (docs/json.md).
#[derive(Serialize)]
struct Sent<'a> {
    account: &'a str,
    from: Address<'a>,
    to: Vec<Address<'a>>,
    cc: Vec<Address<'a>>,
    bcc: Vec<Address<'a>>,
    subject: &'a str,
    message_id: &'a str,
    in_reply_to: Option<&'a str>,
    sent_copy: SentCopy,
}

/// Adds `--cc` and `--bcc` to the draft, builds it, then prints it (`--dry-run`) or confirms and
/// sends it.
fn deliver(account: &Account, mut draft: Draft, common: &Common, cx: Env<'_>) -> Result<ExitCode> {
    for (flag, values) in [("--cc", &common.cc), ("--bcc", &common.bcc)] {
        for m in mailboxes(flag, values)? {
            let listed = draft
                .to
                .iter()
                .chain(&draft.cc)
                .chain(&draft.bcc)
                .any(|o| o.is(m.address()));
            if !listed {
                match flag {
                    "--cc" => draft.cc.push(m),
                    _ => draft.bcc.push(m),
                }
            }
        }
    }
    draft.text_only = common.text_only;
    let outgoing = draft
        .build(Utc::now())
        .map_err(|e| anyhow!("the message cannot be built: {e}"))?;
    // Both read the config alone, so a dry run refuses what the real send would refuse.
    let outbound = account.outbound()?;
    if outbound == Outbound::Graph && !outgoing.bcc.is_empty() {
        bail!(
            "account {} sends through Microsoft Graph, which neomail cannot give Bcc recipients \
             yet; nothing was sent; send without --bcc, or from another account",
            account.name
        );
    }
    if common.dry_run {
        // The summary goes to standard error: it is the only place Bcc shows, as no header
        // names it.
        eprint!("{}", summary(account, &draft));
        cx.out.write_all(&outgoing.mime)?;
        return Ok(ExitCode::SUCCESS);
    }
    if outbound == Outbound::Graph {
        refuse_a_grant_without_send(account)?;
    }
    if !confirm(account, &draft, common.yes, cx.input)? {
        eprintln!("neomail: nothing was sent");
        return Ok(ExitCode::from(crate::EXIT_FAILURE));
    }
    let sent_copy = transmit(account, &outbound, &outgoing)?;
    let sent = Sent {
        account: &account.name,
        from: Address::from(&draft.from),
        to: draft.to.iter().map(Address::from).collect(),
        cc: draft.cc.iter().map(Address::from).collect(),
        bcc: draft.bcc.iter().map(Address::from).collect(),
        subject: &draft.subject,
        message_id: &outgoing.message_id,
        in_reply_to: draft.in_reply_to.as_deref(),
        sent_copy,
    };
    let mode = Mode::choose(common.plain, common.json, common.jsonl, cx.terminal);
    write_sent(cx.out, mode, &sent, &outgoing)?;
    Ok(ExitCode::SUCCESS)
}

/// Refuses, before the question, an Outlook sign-in that does not let neomail send: one from
/// before neomail asked for Mail.Send. Only the keyring entry is read, so no connection is made.
/// A keyring that cannot be read is left to the send, which reports it; `graph::send` checks the
/// grant again.
fn refuse_a_grant_without_send(account: &Account) -> Result<()> {
    if !matches!(account.login, Login::OAuth(_)) {
        return Ok(());
    }
    let Ok(keyring) = Keyring::open_for_oauth() else {
        return Ok(());
    };
    if let Ok(Some(entry)) = keyring.get_oauth(&account.name)
        && !entry.grant().can_send()
    {
        bail!(
            "this account's sign-in does not let neomail send mail; nothing was sent; run \
             `neomail account login {}` once to let neomail send",
            account.name
        );
    }
    Ok(())
}

/// A yes to send: `--yes`, or asked on a terminal. Without either, a usage error.
fn confirm(account: &Account, draft: &Draft, yes: bool, input: Input) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !input.can_prompt() {
        return Err(usage(format!(
            "nothing was sent: {}; pass --yes to send without asking",
            input.why_not()
        )));
    }
    eprint!("{}", summary(account, draft));
    let answer = secret::prompt_line("Send? [y/N] ").context("nothing was sent")?;
    Ok(said_yes(&answer))
}

/// Whether the answer to `Send? [y/N]` is a yes: `y` or `yes` in any case. Anything else, an empty
/// answer included, is a no.
fn said_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// The context of an error met while sending. Its chain may hold a socket's broken pipe, which
/// must never pass for the quiet exit of a closed standard output.
#[derive(Debug)]
pub struct Sending(pub(crate) String);

impl std::fmt::Display for Sending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the question shows: who it is from and to, the subject, what it answers and the body's
/// size.
fn summary(account: &Account, draft: &Draft) -> String {
    let join = |list: &[Mailbox]| {
        list.iter()
            .map(Mailbox::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!("From:     {} (account {})\n", draft.from, account.name);
    for (label, list) in [("To:", &draft.to), ("Cc:", &draft.cc), ("Bcc:", &draft.bcc)] {
        if !list.is_empty() {
            text.push_str(&format!("{label:<10}{}\n", join(list)));
        }
    }
    text.push_str(&format!("Subject:  {}\n", draft.subject));
    if let Some(id) = &draft.in_reply_to {
        text.push_str(&format!("Replying to <{id}>\n"));
    }
    let lines = draft.markdown.lines().count();
    let s = if lines == 1 { "" } else { "s" };
    let how = if draft.text_only {
        "as plain text"
    } else {
        "as HTML and plain text"
    };
    text.push_str(&format!(
        "Body:     {lines} line{s} of Markdown, sent {how}\n"
    ));
    text
}

/// Sends `outgoing` from `account`, then stores the copy in Sent when the server keeps none.
fn transmit(account: &Account, outbound: &Outbound, outgoing: &Outgoing) -> Result<SentCopy> {
    let name = &account.name;
    let runtime = crate::account::runtime()?;
    let mut logins = Logins::default();
    let mut warnings = Vec::new();
    // Outside `block_on`: the keyring starts a runtime of its own.
    let credential = logins.credential(&runtime, account, &mut warnings);
    for warning in warnings {
        eprintln!("neomail: {name}: {warning}");
    }
    let credential = credential.with_context(|| Sending(format!("{name}: nothing was sent")))?;
    match (outbound, &credential) {
        (Outbound::Graph, Credential::AccessToken(token)) => {
            runtime
                .block_on(crate::graph::send(name, token, outgoing))
                .with_context(|| Sending(format!("sending from {name}")))?;
            Ok(SentCopy::Server)
        }
        (Outbound::Graph, Credential::Password(_)) => {
            bail!("a Microsoft Graph account signs in with OAuth, not a password")
        }
        (Outbound::Smtp { config, save_sent }, credential) => {
            runtime
                .block_on(nm_imap::smtp::submit(config, credential.imap(), outgoing))
                .with_context(|| Sending(format!("sending from {name}")))?;
            if !save_sent {
                return Ok(SentCopy::Server);
            }
            let Server::Imap(imap) = &account.server else {
                unreachable!("only an IMAP account sends over SMTP")
            };
            match runtime.block_on(nm_imap::append_sent(
                imap,
                credential.imap(),
                &outgoing.mime,
            )) {
                Ok(()) => Ok(SentCopy::Appended),
                Err(e) => {
                    eprintln!(
                        "neomail: {name}: warning: the message was sent, but its copy could not \
                         be stored in Sent: {:#}",
                        anyhow::Error::new(e)
                    );
                    Ok(SentCopy::Failed)
                }
            }
        }
    }
}

/// One line on a terminal, the Message-ID in a pipe, or one JSON object.
fn write_sent(out: &mut dyn Write, mode: Mode, sent: &Sent<'_>, outgoing: &Outgoing) -> Result<()> {
    match mode {
        Mode::Json => output::write_json(out, sent)?,
        Mode::Jsonl => {
            serde_json::to_writer(&mut *out, sent)?;
            writeln!(out)?;
        }
        Mode::Plain => writeln!(out, "{}", sent.message_id)?,
        Mode::Human => writeln!(
            out,
            "sent from {} to {} (<{}>)",
            sent.account,
            outgoing.envelope_to.join(", "),
            sent.message_id
        )?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_or_yes_in_any_case_is_a_yes() {
        for yes in ["y", "Y", "yes", "YES", "Yes", " y "] {
            assert!(said_yes(yes), "{yes:?}");
        }
        for no in [
            "",
            "n",
            "N",
            "no",
            "ye",
            "yes please",
            "sure",
            "yy",
            "\u{0}y",
        ] {
            assert!(!said_yes(no), "{no:?}");
        }
    }

    #[test]
    fn the_question_shows_every_recipient_the_subject_and_what_it_answers() {
        let config = Config::parse(
            "[[accounts]]\nname = \"work\"\nusername = \"me@example.com\"\n\
             password_command = \"true\"\n[accounts.imap]\nhost = \"imap.example.com\"\n",
        )
        .unwrap();
        let m = |a: &str| Mailbox::parse(a).unwrap();
        let draft = Draft {
            from: m("Me <me@example.com>"),
            to: vec![m("ada@example.com")],
            cc: vec![m("Cy <cy@example.com>")],
            bcc: vec![m("eve@example.com")],
            subject: "Re: Plans".to_owned(),
            markdown: "Yes.\n\n> Shall we?\n".to_owned(),
            text_only: false,
            in_reply_to: Some("three@example.com".to_owned()),
            references: Vec::new(),
        };
        assert_eq!(
            summary(&config.accounts[0], &draft),
            "From:     Me <me@example.com> (account work)\n\
             To:       ada@example.com\n\
             Cc:       Cy <cy@example.com>\n\
             Bcc:      eve@example.com\n\
             Subject:  Re: Plans\n\
             Replying to <three@example.com>\n\
             Body:     3 lines of Markdown, sent as HTML and plain text\n"
        );
    }
}
