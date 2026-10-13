//! `neomail`: the neomail command line: the INBOX (sync, list, show, redact, mark, archive, delete),
//! passwords in the OS keyring, Gmail and Outlook through the user's own OAuth client (Outlook read
//! through Microsoft Graph), and setup (`account add`, `account list`, `config path|check`).

mod account;
mod browser;
mod classify;
mod config;
#[cfg(unix)]
mod daemon;
mod folders;
mod graph;
mod help;
mod http;
mod jev;
mod keyring;
mod lock;
#[cfg(unix)]
mod notify;
mod oauth;
mod output;
mod pager;
mod push;
mod redact;
mod secret;
mod send;
mod setup;
mod style;
mod sync;
mod triage;
mod wizard;

use std::ffi::OsString;
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use nm_core::{Cache, MessageId};
use nm_imap::{CertPin, Security};

use crate::config::{Account, Config, Login, PasswordSource};
use crate::keyring::Keyring;
use crate::output::Mode;
use crate::secret::{Input, Password};
use crate::style::{ColorWhen, Colors};

/// Exit status for a failure other than bad usage (clap exits with 2 for that).
const EXIT_FAILURE: u8 = 1;
/// Exit status for bad usage, the same as clap's.
const EXIT_USAGE: u8 = 2;
const CACHE_FILE: &str = "cache.sqlite3";

const TOP_EXAMPLES: &str = "\
Examples:
  neomail account add               Pick a provider from a menu and add an account
  neomail account add proton        Add Proton Mail through Proton Mail Bridge
  neomail sync && neomail list      Fetch new mail, then list it
  neomail show 42                   Read message 42
  neomail list | fzf -m | neomail show -
                                    Pick messages with fzf, then read them
  neomail redact 42                 See message 42 as a classifier would, personal data masked
  neomail key set                   Store your jev key in the OS keyring
  neomail classify                  Sort the last year's cached mail into groups with jev
  neomail list --group follow-up    List the mail that needs a reply
  neomail mark read 42 43           Mark messages 42 and 43 read, on the server too
  neomail archive 42                Move message 42 to its account's archive
  neomail folder list               List each account's folders
  neomail move Receipts 42          Move message 42 to the folder Receipts
  neomail send --to ada@example.com --subject Hi --body note.md
                                    Send note.md, written in Markdown, after asking
  neomail reply 42 --body answer.md Reply to message 42
  neomail config check              Check the config file

Guide: https://github.com/AH-Merii/neomail#try-it";

const REDACT_EXAMPLES: &str = "\
Examples:
  neomail redact 42                 Print message 42's subject and body excerpt, masked
  neomail redact 42 57              Print two messages, separated by a `── message 57 ──` line
  neomail list | fzf -m | neomail redact -
                                    Pick messages with fzf, then print them masked
  neomail redact --json 42          Print the text and the mask counts as JSON

This is what `neomail classify` sends: the subject, the body's first 1500 characters after
masking, and one line of signals (the sender's domain, whether the address is a role such as
orders@, and whether the mail came through a mailing list, was automated or is a reply). Email
addresses, links, phone, card and IBAN numbers, amounts, one-time codes, runs of 4 or more
digits and the names in the headers become placeholders such as <email>@acme.example or <name>;
postal addresses and names in no header are not masked. The owner's addresses (the config's
logins) are masked whole: without the default config there are none, and a --config or
NEOMAIL_CONFIG file that is missing is an error. It reads the cache only: nothing is sent or
changed. Exit 1 when any id was unknown or unreadable. What is masked and what is not:
https://github.com/AH-Merii/neomail/blob/main/docs/privacy.md";

const LIST_EXAMPLES: &str = "\
Examples:
  neomail list                      The 50 newest messages of every account
  neomail list --account work --limit all
                                    Every cached message of one account
  neomail list --group follow-up    The mail that needs a reply, once `neomail classify` ran
  neomail list --group unclassified --plain | cut -f1 | neomail classify -
                                    Classify the listed messages that have no group yet; as
                                    standard input holds the ids, nothing can be asked: the
                                    backend's consent must be given once on a terminal first

Groups: main, follow-up, follow-up-old, recruitment, receipts-invoices, orders-shipping,
newsletters, promotions, notifications, spam, other. A follow-up older than
classify.follow_up_old_days (90) lists as follow-up-old, and an answer below
classify.other_below (0.4) confidence as other.";

const CLASSIFY_EXAMPLES: &str = "\
Examples:
  neomail key set                   Store your jev key first (or set JEV_API_KEY)
  neomail classify --estimate       See what a run would send and cost; nothing is sent
  neomail classify                  Classify the last 365 days, newest first, at most 1,000
  neomail classify --all            Classify every cached message without a group
  neomail classify 42 57            Classify two messages
  neomail classify --again 42       Ask again for a message that already has a group
  neomail classify --yes --json     In a script: consent given, results as JSON

Each message costs one request to classify.url (https://api.typesafe.ai by default), tried again
up to twice on a rate limit or a server error, with its redacted subject and body excerpt, the
sender's domain and four signals: exactly what `neomail redact ID` shows. Before the first
request to a host, neomail says what will be sent and asks once; --yes consents for a script.
`neomail daemon` classifies new mail in the background only once `daemon = true` under [classify]
in the config switches it on; that consent then covers it too. A result is stored as soon as it
arrives and is never asked for again unless --again: a stopped run resumes where it left off.
Then each classified Outlook message gets its group as a category (classify.label_prefix, nm- by
default; classify.labels = false turns it off), up to 1,000 per account, keeping your own
categories. Exit 1 when a message failed, a category write or an account's sign-in for it
failed, or the run stopped. What is sent and kept:
https://github.com/AH-Merii/neomail/blob/main/docs/privacy.md";

const MARK_EXAMPLES: &str = "\
Examples:
  neomail mark read 42              Mark message 42 read
  neomail mark unread 42 57         Mark two messages unread
  neomail list | fzf -m | neomail mark read -
                                    Pick messages with fzf, then mark them read
  neomail mark read --json 42       Print the marked messages as JSON

The change is made on the server first, then in the cache, so `neomail list` shows it at once.
It needs the server: nothing is queued. Exit 1 when any id was unknown, gone or not confirmed.
An Outlook account added before neomail could change mail needs `neomail account login NAME`
once.";

const ARCHIVE_EXAMPLES: &str = "\
Examples:
  neomail archive 42                Archive message 42
  neomail archive 42 57             Archive two messages
  neomail list | fzf -m | neomail archive -
                                    Pick messages with fzf, then archive them
  neomail archive --json 42         Print the archived messages and their folder as JSON

Each message moves from the INBOX to its account's archive: the mailbox the server marks
\\Archive, else \\All (Gmail's All Mail); on Outlook, the folder Outlook's own Archive uses.
The server moves it first, then it leaves the cache, so `neomail list` no longer shows it. It
needs the server: nothing is queued. Exit 1 when any id was unknown, gone or not confirmed, or
its account has no archive. An Outlook account added before neomail could change mail needs
`neomail account login NAME` once.";

const DELETE_EXAMPLES: &str = "\
Examples:
  neomail delete 42                 Move message 42 to the trash
  neomail delete 42 57              Move two messages to the trash
  neomail list | fzf -m | neomail delete -
                                    Pick messages with fzf, then move them to the trash
  neomail delete --json 42          Print the moved messages and their folder as JSON

Nothing is deleted for good: each message moves from the INBOX to its account's trash (the
mailbox the server marks \\Trash; on Outlook, Deleted Items), and neomail never empties it. The
server moves it first, then it leaves the cache, so `neomail list` no longer shows it. It needs
the server: nothing is queued. Exit 1 when any id was unknown, gone or not confirmed, or its
account has no trash. An Outlook account added before neomail could change mail needs
`neomail account login NAME` once.";

const MOVE_EXAMPLES: &str = "\
Examples:
  neomail folder list --account proton
                                    Find the folder's name
  neomail move Folders/Receipts 42  Move message 42 to Folders/Receipts
  neomail list | fzf -m | neomail move Receipts -
                                    Pick messages with fzf, then move them to Receipts
  neomail move Inbox/Receipts 42    On Outlook: move message 42 to Receipts inside the Inbox
  neomail move --json Receipts 42   Print the moved messages and their folder as JSON

FOLDER is a name exactly as `neomail folder list` prints it, in each message's own account
(INBOX in any case is refused: the messages are in it). neomail never creates a folder: an
account without one of that name moves none of its messages. Each message moves from the INBOX;
the server moves it first, then it leaves the cache, so `neomail list` no longer shows it. It
needs the server: nothing is queued. Exit 1 when any id was unknown, gone or not confirmed, or
its account has no such folder. On Outlook a folder's name is its path of display names.";

const FOLDER_LIST_EXAMPLES: &str = "\
Examples:
  neomail folder list               List every account's folders
  neomail folder list --account gmail
                                    List one account's folders
  neomail folder list --plain | cut -f1
                                    Print only the names `neomail move` takes

Each folder's name, account and role (inbox, archive, trash, junk, sent, drafts, all, flagged,
or none), read from the server: it needs the server. A folder that cannot hold messages is left
out. Exit 1 when any account failed; the others are still printed. On Outlook a folder's name is
its path of display names from the top of the mailbox, such as Inbox/Receipts.";

const ADD_EXAMPLES: &str = "\
Examples:
  neomail account add
  neomail account add proton
  neomail account add gmail-app-password --username you@example.com
  neomail account add imap --name work --username you@example.com --host imap.example.com
  pass show mail/work | neomail account add imap --no-input --name work \\
      --username you@example.com --host imap.example.com
  neomail account add gmail --email you@example.com \\
      --client-id 1234-abc.apps.googleusercontent.com
  neomail account add gmail --project my-project-42
  neomail account add outlook --account-type work --email you@example.com \\
      --client-id 00000000-0000-0000-0000-000000000000 --device-code

Secrets are never flags: they are prompted for without echo, read as one line of standard input,
or come from --password-command.

gmail-app-password goes one step at a time on a terminal: whether the account can have an app
password, 2-Step Verification, creating the app password, then the name and address (type < to
go back). The keyring and the config are checked before the app password is asked, spaces in a
pasted one are removed, and the IMAP login and an SMTP login that sends nothing are checked
before anything is written. Off a terminal or with --no-input it skips the advice steps and needs
--username:
https://github.com/AH-Merii/neomail/blob/main/docs/gmail-app-password.md

gmail and outlook go one step at a time on a terminal too, from no Google Cloud project or no
Entra app to a signed-in client, each step with why it exists and the page to open. outlook shows
only the steps for the account type and the sign-in method. The keyring and the config are
checked before the client secret is asked; the client ID (and Google's secret) is checked with
Google or Microsoft before the sign-in; Gmail's IMAP and SMTP logins and Outlook's Graph mailbox
are checked after it, before anything is written (--no-check skips those two checks), except a
sign-in left in the keyring under that name with no account in the config: it is revoked (Google)
and deleted right before the new sign-in, since revoking it afterwards would end the new one too,
and nothing replaces it if a later step fails. A failure names the step to change. Off a terminal or with --no-input they need --email and --client-id:
https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md
https://github.com/AH-Merii/neomail/blob/main/docs/outlook.md";

#[derive(Parser)]
#[command(
    name = "neomail",
    version,
    about = "neomail: a terminal email client",
    after_help = TOP_EXAMPLES,
    styles = help::styles()
)]
struct Cli {
    /// Config file [env: NEOMAIL_CONFIG] [default: $XDG_CONFIG_HOME/neomail/config.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Never prompt, even on a terminal; a value that would be asked for is a usage error [env: NEOMAIL_NO_INPUT=1]
    #[arg(long, global = true)]
    no_input: bool,
    /// When to colour output [default: auto; NO_COLOR, CLICOLOR_FORCE and CLICOLOR are followed]
    #[arg(long, global = true, value_name = "WHEN", value_enum)]
    color: Option<ColorWhen>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// How a listing prints: human on a terminal, plain TSV off one.
#[derive(clap::Args)]
#[group(multiple = false)]
struct ListingFormat {
    /// Print TSV: one record per line, key field first (id or name), no header (the default when stdout is not a terminal)
    #[arg(long)]
    plain: bool,
    /// Print a JSON array (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
    #[arg(long)]
    json: bool,
    /// Print JSON Lines: one object per line, the same objects as --json
    #[arg(long)]
    jsonl: bool,
}

impl ListingFormat {
    fn mode(&self, terminal: bool) -> Mode {
        Mode::choose(self.plain, self.json, self.jsonl, terminal)
    }
}

#[derive(clap::Args)]
#[group(multiple = false)]
struct ShowFormat {
    /// Print JSON: one object for one id, an array for several or - (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
    #[arg(long)]
    json: bool,
    /// Print JSON Lines: one object per message
    #[arg(long)]
    jsonl: bool,
    /// Print the RFC 822 source of exactly one message
    #[arg(long)]
    raw: bool,
}

/// How `redact` prints: the text on a terminal or off one, or JSON.
#[derive(clap::Args)]
#[group(multiple = false)]
struct RedactFormat {
    /// Print the text without styling (the default when stdout is not a terminal)
    #[arg(long)]
    plain: bool,
    /// Print JSON: one object for one id, an array for several or - (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
    #[arg(long)]
    json: bool,
    /// Print JSON Lines: one object per message
    #[arg(long)]
    jsonl: bool,
}

/// `--limit`: a positive number, or `all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Limit {
    All,
    Count(NonZeroU32),
}

impl Limit {
    fn get(self) -> NonZeroU32 {
        match self {
            Self::All => NonZeroU32::MAX,
            Self::Count(n) => n,
        }
    }
}

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Count(n) => write!(f, "{n}"),
        }
    }
}

fn parse_limit(s: &str) -> Result<Limit, String> {
    if s == "all" {
        return Ok(Limit::All);
    }
    s.parse()
        .map(Limit::Count)
        .map_err(|_| "expected a positive number or `all`".to_owned())
}

/// An id argument of `show`: a message id, or `-` for ids on standard input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdArg {
    Id(i64),
    Stdin,
}

fn parse_id_arg(s: &str) -> Result<IdArg, String> {
    if s == "-" {
        return Ok(IdArg::Stdin);
    }
    s.parse()
        .map(IdArg::Id)
        .map_err(|_| "expected a message id (the first column of `neomail list`) or -".to_owned())
}

#[derive(Subcommand)]
enum Command {
    /// Fetch new mail into the local cache, read-only: nothing changes on the server
    Sync {
        /// Sync only this account
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// How many of the newest messages the first sync takes per account, or all (later syncs fetch all new mail)
        #[arg(long, value_name = "N|all", value_parser = parse_limit, default_value_t = Limit::Count(nm_core::sync::DEFAULT_LIMIT))]
        limit: Limit,
    },
    /// Keep every account's cache current: sync them all now and then every interval, until Ctrl-C or SIGTERM
    #[cfg(unix)]
    #[command(
        after_help = "Classifying in the background is off until `daemon = true` under [classify] in the config \
switches it on; the config is read again at each round (every --interval), so switching it takes effect \
from the next round. While it is off, the daemon sends nothing \
to the classifier and writes no groups. Once on, it classifies new mail as it arrives, with the request \
`neomail classify` sends, once `neomail classify` has your consent for the backend and there is a key \
(`neomail key set` or JEV_API_KEY); it never asks. It sends at most 100 messages each time it syncs (a \
round, a push sync or an Outlook check); the rest wait for the next time it syncs. After classifying, it \
writes each Outlook message's group as a category; `classify.labels = false` turns that off.\n\n\
Guide: https://github.com/AH-Merii/neomail/blob/main/docs/daemon.md"
    )]
    Daemon {
        /// Time between the starts of two rounds: digits followed by s, m or h, from 30s to 24h
        #[arg(long, value_name = "DURATION", default_value = daemon::DEFAULT_INTERVAL, value_parser = daemon::parse_interval)]
        interval: daemon::Interval,
        /// Do not show desktop notifications of new mail
        #[arg(long)]
        no_notify: bool,
    },
    /// List cached messages, newest first ('*' marks unread)
    #[command(after_help = LIST_EXAMPLES)]
    List {
        /// List only this account
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// The most messages to list, or all
        #[arg(long, value_name = "N|all", value_parser = parse_limit, default_value_t = Limit::Count(NonZeroU32::MIN.saturating_add(49)))]
        limit: Limit,
        /// List only this group's messages (`neomail classify` sorts them), or unclassified
        #[arg(long, value_name = "GROUP", value_parser = parse_group)]
        group: Option<nm_core::GroupFilter>,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Show cached messages by the ids from `neomail list`, through a pager on a terminal
    Show {
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: ShowFormat,
        /// Print directly instead of through the pager ($NEOMAIL_PAGER, $PAGER or less)
        #[arg(long)]
        no_pager: bool,
    },
    /// Show the text neomail would send to a classifier for cached messages, with personal data masked
    #[command(after_help = REDACT_EXAMPLES)]
    Redact {
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: RedactFormat,
    },
    /// Sort cached mail into groups with the classifier jev: the last year's, newest first, at most 1,000
    #[command(after_help = CLASSIFY_EXAMPLES)]
    Classify(classify::Args),
    /// Store, delete or check the classifier's API key, kept in the OS keyring
    #[command(after_help = classify::KEY_EXAMPLES)]
    Key {
        #[command(subcommand)]
        command: classify::KeyCommand,
    },
    /// Mark messages read or unread, on the server and in the cache, by the ids from `neomail list`
    #[command(after_help = MARK_EXAMPLES)]
    Mark {
        /// The state to put the messages in
        #[arg(value_enum)]
        state: MarkState,
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Archive messages: move them from the INBOX to the account's archive, by the ids from `neomail list`
    #[command(after_help = ARCHIVE_EXAMPLES)]
    Archive {
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Move messages from the INBOX to the account's trash, never deleting them for good, by the ids from `neomail list`
    #[command(after_help = DELETE_EXAMPLES)]
    Delete {
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Move messages from the INBOX to a folder of their account, by its name and the ids from `neomail list`
    #[command(after_help = MOVE_EXAMPLES)]
    Move {
        /// The folder's name, as `neomail folder list` prints it
        #[arg(value_name = "FOLDER", value_parser = parse_folder_name)]
        folder: nm_core::FolderName,
        /// Message ids, the first column of `neomail list`, or - to read them from standard input, one per line
        #[arg(value_name = "ID", required = true, value_parser = parse_id_arg)]
        ids: Vec<IdArg>,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Send a message written in Markdown from one of your accounts, after asking
    #[command(after_help = send::SEND_EXAMPLES, mut_arg("from", send::from_for_send))]
    Send(send::SendArgs),
    /// Reply to a cached message, by its id from `neomail list`, after asking
    #[command(after_help = send::REPLY_EXAMPLES)]
    Reply(send::ReplyArgs),
    /// Forward a cached message, by its id from `neomail list`, after asking
    #[command(after_help = send::FORWARD_EXAMPLES)]
    Forward(send::ForwardArgs),
    /// List each account's folders, the names `neomail move` takes
    Folder {
        #[command(subcommand)]
        command: FolderCommand,
    },
    /// Print a shell completion script to standard output
    Completions {
        /// The shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Add, list and sign in accounts, and manage passwords in the OS keyring
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Show where the config file is, or check it
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum MarkState {
    /// Read (the \Seen flag set)
    Read,
    /// Unread (the \Seen flag cleared)
    Unread,
}

impl MarkState {
    fn action(self) -> nm_core::Action {
        match self {
            Self::Read => nm_core::Action::MarkRead,
            Self::Unread => nm_core::Action::MarkUnread,
        }
    }
}

#[derive(Subcommand)]
enum FolderCommand {
    /// List each account's folders from its server: name, account and role
    #[command(after_help = FOLDER_LIST_EXAMPLES)]
    List {
        /// List only this account's folders
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        #[command(flatten)]
        format: ListingFormat,
    },
}

/// `list --group`: a group's name, or `unclassified`.
fn parse_group(s: &str) -> Result<nm_core::GroupFilter, String> {
    if s == "unclassified" {
        return Ok(nm_core::GroupFilter::Unclassified);
    }
    s.parse().map(nm_core::GroupFilter::Is).map_err(|_| {
        let names: Vec<_> = nm_core::Group::ALL.iter().map(|g| g.name()).collect();
        format!("expected one of {}, unclassified", names.join(", "))
    })
}

fn parse_folder_name(s: &str) -> Result<nm_core::FolderName, String> {
    nm_core::FolderName::new(s).ok_or_else(|| {
        "expected a folder name (the first column of `neomail folder list`)".to_owned()
    })
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the config file's path, whether or not it exists
    Path,
    /// Parse the config file offline; errors are PATH:LINE:COL: message, never quoting a value
    Check,
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Add an account: a provider menu on a terminal, flags for everything else
    #[command(after_help = ADD_EXAMPLES)]
    Add(Box<AddCommand>),
    /// List the configured accounts (reads only the config: no keyring, no network)
    List {
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Sign an OAuth account in again, replacing its saved sign-in
    Login {
        /// The account's name in the config
        #[arg(value_name = "NAME")]
        name: String,
        /// Print the sign-in URL without opening a browser
        #[arg(long)]
        no_browser: bool,
        /// Sign in with a code on another device (Outlook; automatic over SSH without a display)
        #[arg(long)]
        device_code: bool,
    },
    /// Revoke an OAuth account's sign-in and delete it from the OS keyring
    Logout {
        /// The account's name in the config
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Store an account's password in the OS keyring: prompts on a terminal, else reads one line of stdin
    SetPassword {
        /// The account's name in the config
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Delete an account's password from the OS keyring
    DeletePassword {
        /// The account's name in the config
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(clap::Args)]
struct AddCommand {
    /// The provider; without it, a menu on a terminal
    provider: Option<AddProvider>,
    /// The account's name in neomail [default: gmail, outlook, proton; required for imap]
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// gmail, outlook: the address to sign in with
    #[arg(long, value_name = "ADDR")]
    email: Option<String>,
    /// gmail, outlook: your OAuth client's ID; checked with Google or Microsoft before the sign-in (gmail's secret is prompted for, or read from stdin; outlook has none)
    #[arg(long, value_name = "ID")]
    client_id: Option<String>,
    /// gmail, outlook: print the sign-in URL without opening a browser
    #[arg(long)]
    no_browser: bool,
    /// outlook: sign in with a code on another device (automatic over SSH without a display)
    #[arg(long)]
    device_code: bool,
    /// gmail: your Google Cloud project's ID, so the setup's console links open that project
    #[arg(long, value_name = "ID")]
    project: Option<String>,
    /// outlook: personal (Outlook.com, Hotmail) or work (work or school); picks the setup steps shown [default: personal]
    #[arg(long, value_name = "TYPE")]
    account_type: Option<AccountTypeArg>,
    /// imap, proton, gmail-app-password: the login name (usually the address)
    #[arg(long, value_name = "USER")]
    username: Option<String>,
    /// imap: the IMAP server
    #[arg(long, value_name = "HOST")]
    host: Option<String>,
    /// imap, proton: the port [default: 993 with tls, 143 with starttls; proton 1143]
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
    /// imap: how the connection is secured [default: tls]
    #[arg(long, value_name = "MODE")]
    security: Option<SecurityArg>,
    /// imap, proton, gmail-app-password: run this through `sh -c` for the password instead of storing it in the OS keyring
    #[arg(long, value_name = "CMD")]
    password_command: Option<String>,
    /// imap: the SMTP server to send through, with TLS [default: none, so the account cannot send until the config names one]
    #[arg(long, value_name = "HOST")]
    smtp_host: Option<String>,
    /// imap: the SMTP port [default: 587 with starttls, else 465]
    #[arg(long, value_name = "PORT", requires = "smtp_host")]
    smtp_port: Option<u16>,
    /// imap: how the SMTP connection is secured [default: starttls on port 587, else tls]
    #[arg(long, value_name = "MODE", requires = "smtp_host")]
    smtp_security: Option<SecurityArg>,
    /// imap, proton: the server certificate's SHA-256 to pin (64 hex digits, colons allowed)
    #[arg(long, value_name = "HEX", value_parser = parse_pin)]
    cert_sha256: Option<CertPin>,
    /// Skip the read-only login check before the config is written; gmail and outlook also skip the client check before the sign-in
    #[arg(long)]
    no_check: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum AddProvider {
    /// Gmail, through your own Google OAuth app (https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md)
    Gmail,
    /// Outlook or Hotmail, through your own Microsoft Entra app (https://github.com/AH-Merii/neomail/blob/main/docs/outlook.md)
    Outlook,
    /// Gmail with an app password over IMAP (needs Google 2-Step Verification)
    GmailAppPassword,
    /// Proton Mail, through Proton Mail Bridge on this computer
    Proton,
    /// Any IMAP server, with a password
    Imap,
}

impl AddProvider {
    const ALL: [Self; 5] = [
        Self::Gmail,
        Self::Outlook,
        Self::GmailAppPassword,
        Self::Proton,
        Self::Imap,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Outlook => oauth::MICROSOFT.setup.default_name,
            Self::GmailAppPassword => wizard::gmail::GMAIL_APP_PASSWORD.id,
            Self::Proton => setup::PROTON.id,
            Self::Imap => setup::IMAP.id,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail, through your own Google OAuth app",
            Self::Outlook => "Outlook or Hotmail, through your own Microsoft Entra app",
            Self::GmailAppPassword => "Gmail with an app password (needs 2-Step Verification)",
            Self::Proton => "Proton Mail, through Proton Mail Bridge",
            Self::Imap => "any IMAP server, with a password",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum AccountTypeArg {
    /// Outlook.com, Hotmail or Live
    Personal,
    /// An address your organisation gave you
    Work,
}

impl AccountTypeArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Personal => oauth::steps::PERSONAL,
            Self::Work => oauth::steps::WORK,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SecurityArg {
    /// TLS from the first byte
    Tls,
    /// A plain connection upgraded with STARTTLS before login
    Starttls,
}

impl SecurityArg {
    fn security(self) -> Security {
        match self {
            Self::Tls => Security::Tls,
            Self::Starttls => Security::StartTls,
        }
    }
}

fn parse_pin(s: &str) -> Result<CertPin, String> {
    s.parse().map_err(|e: nm_imap::InvalidPin| e.to_string())
}

/// Bad usage that clap cannot see: a value that is missing and cannot be asked for, or a flag
/// that does not apply. Exits 2, like clap's own usage errors.
#[derive(Debug)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

pub fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UsageError(message.into()))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let cli = match parse(&args) {
        Ok(cli) => cli,
        Err(e) => return clap_exit(&e, &args),
    };
    // The one colour choice per stream, passed down from here.
    let colors = Colors::new(
        style::decide(cli.color, style::env_var, io::stdout().is_terminal()),
        style::decide(cli.color, style::env_var, io::stderr().is_terminal()),
    );
    match run(cli, colors) {
        Ok(code) => code,
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e:#}", colors.stderr.error("neomail"));
            if e.chain().any(|c| c.is::<UsageError>()) {
                ExitCode::from(EXIT_USAGE)
            } else {
                ExitCode::from(EXIT_FAILURE)
            }
        }
    }
}

/// The command line as help shows it: each command's examples headed like clap's headings.
fn cli_command() -> clap::Command {
    help::headed(Cli::command())
}

/// [`Parser::try_parse_from`] on [`cli_command`], so the help it prints has those headings.
fn parse(args: &[OsString]) -> Result<Cli, clap::Error> {
    let mut matches = cli_command().try_get_matches_from(args)?;
    Cli::from_arg_matches_mut(&mut matches).map_err(|e| e.format(&mut cli_command()))
}

/// Prints clap's help, version or usage error, coloured by the same rules as everything else
/// (`--color` read from the raw arguments, since they did not parse). The top-level help opens
/// with the logo on a terminal.
fn clap_exit(e: &clap::Error, args: &[OsString]) -> ExitCode {
    let to_stdout = !e.use_stderr();
    let stream = if to_stdout {
        help::Stream::Stdout
    } else {
        help::Stream::Stderr
    };
    let color = style::decide(color_flag(args), style::env_var, stream.is_terminal())
        != style::Decision::Off;
    let styled = e.render();
    let text = if color {
        styled.ansi().to_string()
    } else {
        styled.to_string()
    };
    let text = if is_top_level_help(e, args) {
        help::with_logo(stream.logo(color), &text)
    } else {
        text
    };
    // A closed pipe (`neomail --help | head -1`) is not an error.
    let _ = if to_stdout {
        io::stdout().lock().write_all(text.as_bytes())
    } else {
        io::stderr().lock().write_all(text.as_bytes())
    };
    ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(EXIT_USAGE))
}

/// Whether `e` shows the top-level help (`neomail --help`, `-h`, `neomail help`) rather than a
/// command's: its text is the top-level help's, under the name clap took from `args[0]`.
fn is_top_level_help(e: &clap::Error, args: &[OsString]) -> bool {
    if e.kind() != clap::error::ErrorKind::DisplayHelp {
        return false;
    }
    let mut top = cli_command();
    if let Some(name) = args
        .first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|n| n.to_str())
    {
        top = top.bin_name(name);
    }
    let shown = e.render().to_string();
    shown == top.render_help().to_string() || shown == top.render_long_help().to_string()
}

/// The last `--color WHEN` or `--color=WHEN` before `--`, for arguments that did not parse.
fn color_flag(args: &[OsString]) -> Option<ColorWhen> {
    let mut found = None;
    let mut args = args.iter().skip(1).map(|a| a.to_string_lossy());
    while let Some(a) = args.next() {
        let value = match a.as_ref() {
            "--" => break,
            "--color" => args.next().map(|v| v.into_owned()),
            a => a.strip_prefix("--color=").map(str::to_owned),
        };
        if let Some(v) = value {
            found = ColorWhen::from_str(&v, true).ok().or(found);
        }
    }
    found
}

/// `NEOMAIL_NO_INPUT` set to anything but empty or `0`.
fn no_input_env() -> bool {
    std::env::var_os("NEOMAIL_NO_INPUT").is_some_and(|v| !v.is_empty() && v != "0")
}

/// What bare `neomail` prints on standard error when there is no config yet.
fn start_text() -> String {
    format!(
        "neomail: no accounts yet. Start with:\n  neomail account add\n  neomail sync && neomail list\n\
         Guide: {}\n",
        config::GUIDE_URL
    )
}

/// The hint for an empty or missing cache: add an account first when there is no config file,
/// else sync.
fn no_cache_hint(explicit: Option<PathBuf>) -> String {
    match config::config_path(explicit) {
        Ok(path) if !config::exists(&path) => format!("no accounts yet; {}", config::ADD_HINT),
        _ => "nothing is cached yet; run `neomail sync` first".to_owned(),
    }
}

/// Whether `e` is the reader of standard output going away. A broken pipe met while sending is a
/// socket's, and a failure like any other.
fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.downcast_ref::<send::Sending>().is_none()
        && e.chain()
            .filter_map(|c| c.downcast_ref::<io::Error>())
            .any(|io| io.kind() == io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli, colors: Colors) -> Result<ExitCode> {
    let input = Input::detect(cli.no_input || no_input_env());
    let Some(command) = cli.command else {
        let path = config::config_path(cli.config)?;
        if config::exists(&path) {
            let help = cli_command().render_help();
            let color = colors.stderr.is_on();
            let text = if color {
                help.ansi().to_string()
            } else {
                help.to_string()
            };
            eprint!(
                "{}",
                help::with_logo(help::Stream::Stderr.logo(color), &text)
            );
        } else {
            eprint!("{}", start_text());
        }
        return Ok(ExitCode::from(EXIT_USAGE));
    };
    let stdout = io::stdout();
    let terminal = stdout.is_terminal();
    let mut out = BufWriter::new(stdout.lock());
    let code = match command {
        Command::Sync { account, limit } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            let accounts = config.select(account.as_deref())?;
            let data_dir = config::data_dir()?;
            let mut cache = open_cache(true)?.context("the cache could not be created")?;
            let runtime = account::runtime()?;
            let stderr = io::stderr();
            let progress = output::ProgressLine::new(stderr.is_terminal());
            let mut syncer = sync::Syncer::new(&runtime, limit.get(), progress);
            let mut failed = false;
            for account in accounts {
                // One sync per account: the daemon may be syncing this one.
                let result = lock::wait_for_account(&data_dir, &account.name, || {
                    eprintln!(
                        "neomail: waiting for neomail daemon to finish syncing {}",
                        account.name
                    );
                })
                .and_then(|_held| {
                    let synced = syncer.sync(account, &mut cache);
                    syncer.progress.clear(&mut io::stderr());
                    for warning in synced.warnings {
                        eprintln!("neomail: {}: {warning}", account.name);
                    }
                    synced.report
                })
                .map(|report| sync::report_line(&account.name, &report));
                syncer.progress.clear(&mut io::stderr());
                match result {
                    Ok(line) => {
                        writeln!(out, "{line}")?;
                        out.flush()?;
                    }
                    Err(e) => {
                        out.flush()?;
                        eprintln!("neomail: {}: {e:#}", account.name);
                        failed = true;
                    }
                }
            }
            if failed {
                ExitCode::from(EXIT_FAILURE)
            } else {
                ExitCode::SUCCESS
            }
        }
        #[cfg(unix)]
        Command::Daemon {
            interval,
            no_notify,
        } => daemon::run(config::config_path(cli.config)?, interval, no_notify)?,
        Command::List {
            account,
            limit,
            group,
            format,
        } => {
            // The thresholds that turn stored answers into groups; a missing file has none.
            let rules = config::load_optional(cli.config.clone(), false)?
                .map(|c| c.classify.rules)
                .unwrap_or_default();
            let entries = match open_cache(false)? {
                Some(cache) => {
                    if let Some(name) = account.as_deref() {
                        check_cached_account(&cache, name)?;
                    }
                    cache.list_grouped(
                        account.as_deref(),
                        group.unwrap_or(nm_core::GroupFilter::Any),
                        limit.get().get(),
                        &rules,
                        chrono::Utc::now(),
                    )?
                }
                None if account.is_some() => bail!("{}", no_cache_hint(cli.config)),
                None => Vec::new(),
            };
            let mode = format.mode(terminal);
            if entries.is_empty() && group.is_some() && matches!(mode, Mode::Human | Mode::Plain) {
                eprintln!("neomail: no cached messages in that group");
            } else if entries.is_empty() && matches!(mode, Mode::Human | Mode::Plain) {
                let hint = match no_cache_hint(cli.config) {
                    h if h.starts_with("nothing") => {
                        "no cached messages; run `neomail sync` first".to_owned()
                    }
                    h => h,
                };
                eprintln!("neomail: {hint}");
            }
            let items: Vec<_> = entries.iter().map(output::ListItem::from).collect();
            output::write_listing(
                &mut out,
                mode,
                &items,
                |out| output::write_list(out, &entries, terminal_width(), colors.stdout),
                |out| output::write_list_plain(out, &entries, colors.plain),
            )?;
            ExitCode::SUCCESS
        }
        Command::Show {
            ids,
            format,
            no_pager,
        } => show(
            &ids, &format, no_pager, cli.config, terminal, colors, &mut out,
        )?,
        Command::Redact { ids, format } => {
            let mode = Mode::choose(format.plain, format.json, format.jsonl, terminal);
            redact(&ids, mode, cli.config, colors, &mut out)?
        }
        Command::Classify(args) => {
            let ids = read_ids(&args.ids)?;
            classify::run(&args, ids, cli.config, input, terminal, colors, &mut out)?
        }
        Command::Key { command } => classify::key(&command, cli.config, input, &mut out)?,
        Command::Mark { state, ids, format } => run_action(
            state.action(),
            &ids,
            &format,
            cli.config,
            terminal,
            colors,
            &mut out,
        )?,
        Command::Archive { ids, format } => run_action(
            nm_core::Action::Archive,
            &ids,
            &format,
            cli.config,
            terminal,
            colors,
            &mut out,
        )?,
        Command::Delete { ids, format } => run_action(
            nm_core::Action::Delete,
            &ids,
            &format,
            cli.config,
            terminal,
            colors,
            &mut out,
        )?,
        Command::Move {
            folder,
            ids,
            format,
        } => run_action(
            nm_core::Action::Move(folder),
            &ids,
            &format,
            cli.config,
            terminal,
            colors,
            &mut out,
        )?,
        // These flush standard output themselves: once a message is sent, a failure to print is
        // only a warning, so the flush below must not turn it into an error.
        Command::Send(args) => {
            return send::send(args, send_env(cli.config, input, terminal, &mut out));
        }
        Command::Reply(args) => {
            return send::reply(args, send_env(cli.config, input, terminal, &mut out));
        }
        Command::Forward(args) => {
            return send::forward(args, send_env(cli.config, input, terminal, &mut out));
        }
        Command::Folder {
            command: FolderCommand::List { account, format },
        } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            let accounts = config.select(account.as_deref())?;
            let runtime = account::runtime()?;
            let mut logins = sync::Logins::default();
            let listed: Vec<_> = accounts
                .iter()
                .map(|a| (a.name.as_str(), folders::list(&runtime, a, &mut logins)))
                .collect();
            let code = folders::finish(
                &listed,
                format.mode(terminal),
                &mut out,
                &mut io::stderr().lock(),
                colors,
            )?;
            ExitCode::from(code)
        }
        Command::Completions { shell } => {
            // Into memory first: clap_complete panics when a write fails.
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "neomail", &mut script);
            out.write_all(&script)?;
            ExitCode::SUCCESS
        }
        Command::Config {
            command: ConfigCommand::Path,
        } => {
            writeln!(out, "{}", config::config_path(cli.config)?.display())?;
            ExitCode::SUCCESS
        }
        Command::Config {
            command: ConfigCommand::Check,
        } => {
            let path = config::config_path(cli.config)?;
            match check_config(&path)? {
                Ok(count) => {
                    let s = if count == 1 { "" } else { "s" };
                    writeln!(out, "ok: {count} account{s}")?;
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::from(EXIT_FAILURE)
                }
            }
        }
        Command::Account {
            command: AccountCommand::Add(add),
        } => {
            let path = config::config_path(cli.config)?;
            add_account(*add, &path, input, &mut out)?;
            ExitCode::SUCCESS
        }
        Command::Account {
            command: AccountCommand::List { format },
        } => {
            let path = config::config_path(cli.config)?;
            let accounts = if config::exists(&path) {
                Config::load(&path)?.accounts
            } else {
                Vec::new()
            };
            if accounts.is_empty() {
                eprintln!("neomail: no accounts yet; {}", config::ADD_HINT);
            }
            let items: Vec<_> = accounts.iter().map(output::AccountItem::from).collect();
            output::write_listing(
                &mut out,
                format.mode(terminal),
                &items,
                |out| output::write_accounts(out, &items, colors.stdout),
                |out| output::write_accounts_plain(out, &items, colors.plain),
            )?;
            ExitCode::SUCCESS
        }
        Command::Account { command } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            match command {
                AccountCommand::Add(_) | AccountCommand::List { .. } => {
                    unreachable!("handled above")
                }
                AccountCommand::Login {
                    name,
                    no_browser,
                    device_code,
                } => {
                    let account = config.select(Some(&name))?[0];
                    account::login(account, no_browser, device_code, input, &mut out)?;
                }
                AccountCommand::Logout { name } => {
                    account::logout(config.select(Some(&name))?[0], &mut out)?;
                }
                AccountCommand::SetPassword { name } => {
                    let account = config.select(Some(&name))?[0];
                    refuse_oauth(account, "login")?;
                    // Before the password: no one should type it only to learn it cannot be
                    // stored. On Linux, opening connects to the Secret Service.
                    let keyring = Keyring::open()?;
                    let password = secret::read_new_password(&account.name, input)?;
                    writeln!(out, "{}", set_password(&keyring, account, &password)?)?;
                }
                AccountCommand::DeletePassword { name } => {
                    let account = config.select(Some(&name))?[0];
                    refuse_oauth(account, "logout")?;
                    let keyring = Keyring::open()?;
                    let (done, prep) = if keyring.delete(&account.name)? {
                        ("Deleted the", "from")
                    } else {
                        ("There was no", "in")
                    };
                    writeln!(
                        out,
                        "{done} password for {:?} {prep} {}.",
                        account.name,
                        keyring.name()
                    )?;
                }
            }
            ExitCode::SUCCESS
        }
    };
    out.flush()?;
    Ok(code)
}

/// What `send`, `reply` and `forward` read and write.
fn send_env(
    config: Option<PathBuf>,
    input: Input,
    terminal: bool,
    out: &mut dyn Write,
) -> send::Env<'_> {
    send::Env {
        config,
        input,
        terminal,
        out,
    }
}

/// The terminal's width for the human list, 80 when unknown.
fn terminal_width() -> usize {
    terminal_size::terminal_size_of(io::stdout()).map_or(output::WIDTH, |(w, _)| usize::from(w.0))
}

/// `mark`, `archive`, `delete` and `move`: applies `action` to the ids on their servers, then in
/// the cache, and prints the report.
fn run_action(
    action: nm_core::Action,
    ids: &[IdArg],
    format: &ListingFormat,
    config: Option<PathBuf>,
    terminal: bool,
    colors: Colors,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let ids = read_ids(ids)?;
    let settings = Config::load(&config::config_path(config.clone())?)?;
    let Some(mut cache) = open_cache(false)? else {
        bail!("{}", no_cache_hint(config));
    };
    let runtime = account::runtime()?;
    let data_dir = config::data_dir()?;
    let mut writer = triage::Writer {
        runtime: &runtime,
        config: &settings,
        data_dir: &data_dir,
        logins: sync::Logins::default(),
    };
    let ids: Vec<MessageId> = ids.into_iter().map(MessageId::new).collect();
    let report = nm_core::action::apply(action.clone(), &ids, &mut cache, &mut writer)?;
    let code = triage::finish(
        &action,
        &report,
        format.mode(terminal),
        out,
        &mut io::stderr().lock(),
        colors.stderr,
    )?;
    Ok(ExitCode::from(code))
}

/// `neomail show`: one or more ids, or `-` for ids on standard input. Missing or unreadable
/// messages are reported on standard error after the others are shown, and exit 1.
fn show(
    ids: &[IdArg],
    format: &ShowFormat,
    no_pager: bool,
    config: Option<PathBuf>,
    terminal: bool,
    colors: Colors,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let one_literal = matches!(ids, [IdArg::Id(_)]);
    let ids = read_ids(ids)?;
    if format.raw && ids.len() != 1 {
        return Err(usage(format!(
            "--raw takes exactly one id, not {}",
            ids.len()
        )));
    }
    let Some(cache) = open_cache(false)? else {
        bail!("{}", no_cache_hint(config));
    };
    // Reported after the output is written or paged, so the reason for exit 1 is the last
    // thing on the terminal rather than scrolled away above the messages.
    let mut failures = Vec::new();
    let mut messages = Vec::new();
    for id in ids {
        let Some(stored) = cache.get(MessageId::new(id))? else {
            failures.push(format!(
                "no cached message with id {id}; see `neomail list`"
            ));
            continue;
        };
        if format.raw {
            out.write_all(&stored.raw)?;
            continue;
        }
        match nm_core::parse::parse(&stored.raw) {
            Ok(parsed) => messages.push((stored, parsed)),
            Err(_) => failures.push(format!("message {id} cannot be read; try --raw")),
        }
    }
    let items = messages
        .iter()
        .map(|(stored, parsed)| output::ShowItem::new(stored, parsed));
    if format.jsonl {
        output::write_jsonl(out, items)?;
    } else if format.json {
        let items: Vec<_> = items.collect();
        match items.as_slice() {
            [item] if one_literal => output::write_json(out, item)?,
            // One literal id that is missing prints nothing, as before.
            [] if one_literal => {}
            items => output::write_json(out, items)?,
        }
    } else if !format.raw {
        let pager = pager::choose(no_pager, terminal, style::env_var);
        let mut text = Vec::new();
        for (i, item) in items.enumerate() {
            if i > 0 {
                output::write_separator(&mut text, item.id, colors.stdout)?;
            }
            output::write_show(&mut text, &item, colors.stdout)?;
        }
        match pager {
            Some(pager) if !text.is_empty() => {
                out.flush()?;
                pager::page(&pager, &text, out)?;
            }
            _ => out.write_all(&text)?,
        }
    }
    out.flush()?;
    for message in &failures {
        eprintln!("{}: {message}", colors.stderr.error("neomail"));
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    })
}

/// `neomail redact`: for each id, the text classification would send, from the cache only.
/// Unknown or unreadable messages are reported on standard error after the others, and exit 1.
fn redact(
    ids: &[IdArg],
    mode: Mode,
    config: Option<PathBuf>,
    colors: Colors,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let one_literal = matches!(ids, [IdArg::Id(_)]);
    let ids = read_ids(ids)?;
    let Some(cache) = open_cache(false)? else {
        bail!("{}", no_cache_hint(config));
    };
    // The owner's addresses come from the config. Without the default one there are none to mask
    // whole, but a config the user named must load: a typo never quietly drops the owner rule.
    let config = config::load_optional(config, true)?;
    let owners = config
        .as_ref()
        .map(redact::owner_addresses)
        .unwrap_or_default();
    let mut failures = Vec::new();
    let mut messages = Vec::new();
    for id in ids {
        let Some(stored) = cache.get(MessageId::new(id))? else {
            failures.push(format!(
                "no cached message with id {id}; see `neomail list`"
            ));
            continue;
        };
        match nm_core::parse::parse(&stored.raw) {
            Ok(parsed) => {
                let outgoing = redact::outgoing(&parsed, &owners);
                messages.push((stored, outgoing));
            }
            Err(_) => failures.push(format!(
                "message {id} cannot be read; see `neomail show --raw {id}`"
            )),
        }
    }
    let items = messages
        .iter()
        .map(|(stored, outgoing)| output::RedactItem::new(stored, outgoing));
    match mode {
        Mode::Jsonl => output::write_jsonl(out, items)?,
        Mode::Json => {
            let items: Vec<_> = items.collect();
            match items.as_slice() {
                [item] if one_literal => output::write_json(out, item)?,
                [] if one_literal => {}
                items => output::write_json(out, items)?,
            }
        }
        Mode::Human | Mode::Plain => {
            let paint = if mode == Mode::Human {
                colors.stdout
            } else {
                colors.plain
            };
            for (i, item) in items.enumerate() {
                if i > 0 {
                    output::write_separator(out, item.id, paint)?;
                }
                output::write_redacted(out, &item, paint)?;
            }
        }
    }
    out.flush()?;
    for message in &failures {
        eprintln!("{}: {message}", colors.stderr.error("neomail"));
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    })
}

/// The ids of `show`, `redact`, `mark`, `archive` and `delete`: the ones given, or with `-` the ones on
/// standard input. `-` with other ids is a usage error.
fn read_ids(ids: &[IdArg]) -> Result<Vec<i64>> {
    if !ids.contains(&IdArg::Stdin) {
        return Ok(ids
            .iter()
            .filter_map(|id| match id {
                IdArg::Id(id) => Some(*id),
                IdArg::Stdin => None,
            })
            .collect());
    }
    if ids.len() > 1 {
        return Err(usage(
            "`-` reads the ids from standard input; give no other ids",
        ));
    }
    let mut input = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut input)
        .context("cannot read standard input")?;
    ids_from_lines(&String::from_utf8_lossy(&input))
}

/// The ids on standard input, one per line, in order. Blank lines are skipped; a line without
/// an id is a usage error naming its line number.
fn ids_from_lines(input: &str) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for (n, line) in input.lines().enumerate() {
        match id_from_line(line) {
            Ok(Some(id)) => ids.push(id),
            Ok(None) => {}
            Err(()) => {
                return Err(usage(format!(
                    "standard input line {}: no message id at its start (the first column of `neomail list`)",
                    n + 1
                )));
            }
        }
    }
    Ok(ids)
}

/// A line's id: after leading whitespace and ANSI SGR codes, the digits up to the first tab or
/// space (SGR codes may follow the digits). `Ok(None)` for a blank line.
fn id_from_line(line: &str) -> std::result::Result<Option<i64>, ()> {
    let mut rest = line.trim_start();
    while let Some(after) = strip_sgr(rest) {
        rest = after.trim_start();
    }
    if rest.is_empty() {
        return Ok(None);
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (digits, mut tail) = rest.split_at(end);
    while let Some(after) = strip_sgr(tail) {
        tail = after;
    }
    if !(tail.is_empty() || tail.starts_with(['\t', ' '])) {
        return Err(());
    }
    digits.parse().map(Some).map_err(|_| ())
}

/// `s` without one leading SGR code (`ESC [ digits;... m`).
fn strip_sgr(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("\x1b[")?;
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == ';'))?;
    rest[end..].strip_prefix('m')
}

/// `neomail config check`: the number of accounts, or the message to print ("PATH:LINE:COL:
/// message" for a syntax error). Only a missing or unreadable file is an `Err`.
fn check_config(path: &Path) -> Result<std::result::Result<usize, String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => bail!(
            "no config file at {}: no accounts yet; {}",
            path.display(),
            config::ADD_HINT
        ),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    Ok(match Config::parse(&text) {
        Ok(config) => Ok(config.accounts.len()),
        Err(e) => Err(match e.downcast_ref::<config::SyntaxError>() {
            Some(s) => format!("{}:{}:{}: {}", path.display(), s.line, s.column, s.message),
            None => format!("{}: {e:#}", path.display()),
        }),
    })
}

/// `neomail account add`: picks the provider (a menu on a terminal), refuses flags that do not
/// apply to it, then runs its flow.
fn add_account(add: AddCommand, path: &Path, input: Input, out: &mut impl Write) -> Result<()> {
    let provider = match add.provider {
        Some(p) => p,
        None if input.can_prompt() => choose_provider()?,
        None => {
            return Err(usage(format!(
                "{}: name the provider, one of {} (`neomail account add PROVIDER`)",
                input.why_not(),
                AddProvider::ALL.map(AddProvider::id).join(", ")
            )));
        }
    };
    // --device-code on gmail reaches account::add, which says why Google cannot use it.
    let oauth_only = [
        ("--email", add.email.is_some()),
        ("--client-id", add.client_id.is_some()),
        ("--no-browser", add.no_browser),
        ("--device-code", add.device_code),
    ];
    let password_only = [
        ("--username", add.username.is_some()),
        ("--password-command", add.password_command.is_some()),
    ];
    let gmail_only = [("--project", add.project.is_some())];
    let outlook_only = [("--account-type", add.account_type.is_some())];
    let imap_only = [
        ("--host", add.host.is_some()),
        ("--security", add.security.is_some()),
        ("--smtp-host", add.smtp_host.is_some()),
        ("--smtp-port", add.smtp_port.is_some()),
        ("--smtp-security", add.smtp_security.is_some()),
    ];
    let port_pin = [
        ("--port", add.port.is_some()),
        ("--cert-sha256", add.cert_sha256.is_some()),
    ];
    let refused: Vec<(&str, bool)> = match provider {
        AddProvider::Gmail => [&password_only[..], &outlook_only, &imap_only, &port_pin].concat(),
        AddProvider::Outlook => [&password_only[..], &gmail_only, &imap_only, &port_pin].concat(),
        AddProvider::GmailAppPassword => [
            &oauth_only[..],
            &gmail_only,
            &outlook_only,
            &imap_only,
            &port_pin,
        ]
        .concat(),
        AddProvider::Proton => [&oauth_only[..], &gmail_only, &outlook_only, &imap_only].concat(),
        AddProvider::Imap => [&oauth_only[..], &gmail_only, &outlook_only].concat(),
    };
    if let Some((flag, _)) = refused.iter().find(|(_, given)| *given) {
        return Err(usage(format!(
            "{flag} does not apply to `neomail account add {}`",
            provider.id()
        )));
    }
    let preset = match provider {
        AddProvider::Gmail | AddProvider::Outlook => {
            let args = account::AddArgs {
                name: add.name,
                email: add.email,
                client_id: add.client_id,
                project: add.project,
                account_type: add.account_type.map(|t| t.as_str().to_owned()),
                no_browser: add.no_browser,
                device_code: add.device_code,
                no_check: add.no_check,
            };
            let oauth_provider = match provider {
                AddProvider::Outlook => &oauth::MICROSOFT,
                _ => &oauth::GOOGLE,
            };
            return account::add(oauth_provider, path, args, input, out);
        }
        AddProvider::GmailAppPassword => {
            let args = wizard::gmail::Args {
                name: add.name,
                username: add.username,
                password_command: add.password_command,
                no_check: add.no_check,
            };
            return wizard::gmail::add(path, args, input, out);
        }
        AddProvider::Proton => &setup::PROTON,
        AddProvider::Imap => &setup::IMAP,
    };
    let args = setup::Args {
        name: add.name,
        username: add.username,
        host: add.host,
        port: add.port,
        security: add.security.map(SecurityArg::security),
        password_command: add.password_command,
        cert_sha256: add.cert_sha256,
        no_check: add.no_check,
        smtp_host: add.smtp_host,
        smtp_port: add.smtp_port,
        smtp_security: add.smtp_security.map(SecurityArg::security),
    };
    setup::add(preset, path, args, input, out)
}

/// The provider menu, on standard error.
fn choose_provider() -> Result<AddProvider> {
    eprintln!("Which kind of account?");
    for (i, p) in AddProvider::ALL.iter().enumerate() {
        eprintln!("  {}. {:<20} {}", i + 1, p.id(), p.label());
    }
    loop {
        let typed = secret::prompt_line(&format!("Number [1-{}]: ", AddProvider::ALL.len()))?;
        if let Some(p) = parse_choice(&typed) {
            return Ok(p);
        }
        eprintln!("neomail: {typed:?} is not one of the numbers or names above");
    }
}

/// A menu answer: its number or the provider's name.
fn parse_choice(typed: &str) -> Option<AddProvider> {
    let typed = typed.trim();
    AddProvider::ALL.into_iter().enumerate().find_map(|(i, p)| {
        (typed == (i + 1).to_string() || typed.eq_ignore_ascii_case(p.id())).then_some(p)
    })
}

/// Opens the cache. With `create` unset, a missing cache is `None` rather than a new empty file.
fn open_cache(create: bool) -> Result<Option<Cache>> {
    let dir = config::data_dir()?;
    let path = dir.join(CACHE_FILE);
    if !create && !path.exists() {
        return Ok(None);
    }
    create_private_dir(&dir)?;
    #[cfg(unix)]
    restrict_cache_files(&path)?;
    let cache = Cache::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    Ok(Some(cache))
}

/// Makes the cache file and SQLite's `-wal` and `-shm` files readable only by the user, even in
/// a data directory that already existed with looser modes. A missing cache file is created
/// empty with mode 0600 first; SQLite gives the `-wal` and `-shm` files it creates later the
/// same mode.
#[cfg(unix)]
fn restrict_cache_files(path: &Path) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        let file = Path::new(&file);
        let Ok(meta) = std::fs::metadata(file) else {
            continue; // Not there (yet).
        };
        if meta.permissions().mode() & 0o777 != 0o600 {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("cannot make {} private", file.display()))?;
        }
    }
    Ok(())
}

/// Creates the data directory readable only by the user: it holds mail.
fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))
}

fn check_cached_account(cache: &Cache, name: &str) -> Result<()> {
    let cached = cache.accounts()?;
    if !cached.iter().any(|a| a == name) {
        bail!(
            "no cached account named {name:?}; cached: {}",
            if cached.is_empty() {
                "none".to_owned()
            } else {
                cached.join(", ")
            }
        );
    }
    Ok(())
}

/// Fails for an OAuth account, which has no password: `instead` is the account command to use.
fn refuse_oauth(account: &Account, instead: &str) -> Result<()> {
    if let Login::OAuth(settings) = &account.login {
        bail!(
            "account {:?} signs in with {} (OAuth) and has no password; use `neomail account {instead} {}`",
            account.name,
            settings.provider.name,
            account.name
        );
    }
    Ok(())
}

/// Stores the password and says where, in one line.
fn set_password(keyring: &Keyring, account: &Account, password: &Password) -> Result<String> {
    keyring.set(&account.name, password)?;
    let mut line = format!(
        "Stored the password for {:?} in {} (service {:?}, username {:?}).",
        account.name,
        keyring.name(),
        keyring::SERVICE,
        account.name
    );
    if matches!(account.login, Login::Password(PasswordSource::Command(_))) {
        line.push_str(" The account's password_command takes precedence while it is set.");
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only standard output's reader leaving is quiet: a socket's broken pipe met while sending
    /// is a failure, or a script would take an unsent message for a sent one.
    #[test]
    fn a_broken_pipe_is_quiet_only_off_standard_output() {
        let pipe = || io::Error::from(io::ErrorKind::BrokenPipe);
        assert!(is_broken_pipe(
            &anyhow::Error::new(pipe()).context("writing the list")
        ));
        let smtp = nm_imap::smtp::SmtpError::Io {
            step: "the message",
            source: pipe(),
        };
        let sending: Result<()> = Err(smtp).context(send::Sending("sending from work".into()));
        assert!(!is_broken_pipe(&sending.unwrap_err().context("outer")));
    }

    #[test]
    fn menu_answers_are_a_number_or_a_name() {
        assert!(matches!(parse_choice("1"), Some(AddProvider::Gmail)));
        assert!(matches!(parse_choice("2"), Some(AddProvider::Outlook)));
        assert!(matches!(parse_choice(" 4 "), Some(AddProvider::Proton)));
        assert!(matches!(
            parse_choice("outlook"),
            Some(AddProvider::Outlook)
        ));
        assert!(matches!(parse_choice("IMAP"), Some(AddProvider::Imap)));
        assert!(matches!(
            parse_choice("gmail-app-password"),
            Some(AddProvider::GmailAppPassword)
        ));
        for bad in ["", "0", "6", "hotmail"] {
            assert!(parse_choice(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_stdin_line_s_id_is_its_first_field() {
        let ok = |line: &str| id_from_line(line).unwrap();
        assert_eq!(ok("42\t*\t2026-10-10T09:30:00Z\twork\tAda\tHi"), Some(42));
        assert_eq!(ok("  7 * 2026-10-10  Ada  Hi"), Some(7));
        assert_eq!(ok("\x1b[33m42\x1b[0m\t\x1b[1m*\x1b[0m\tx"), Some(42));
        assert_eq!(ok("\x1b[33m  42\x1b[0m \x1b[1m*\x1b[0m"), Some(42));
        assert_eq!(ok("42"), Some(42));
        assert_eq!(ok(""), None);
        assert_eq!(ok(" \t "), None);
        for bad in [
            "abc",
            "42abc",
            "-1",
            "x 42",
            "\x1b[31",
            "99999999999999999999",
        ] {
            assert!(id_from_line(bad).is_err(), "{bad:?}");
        }
        let ids = ids_from_lines("1\tx\n\n  2 y\r\n").unwrap();
        assert_eq!(ids, [1, 2]);
        let e = ids_from_lines("1\n\nnope\n").unwrap_err();
        assert!(e.to_string().contains("line 3"), "{e}");
        assert!(e.is::<UsageError>());
    }

    #[test]
    fn limit_is_a_positive_number_or_all() {
        assert_eq!(parse_limit("all"), Ok(Limit::All));
        assert_eq!(Limit::All.get(), NonZeroU32::MAX);
        assert_eq!(
            parse_limit("3").map(Limit::get),
            Ok(NonZeroU32::MIN.saturating_add(2))
        );
        for bad in ["0", "-1", "ALL", ""] {
            assert!(parse_limit(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn color_flag_is_found_in_arguments_that_did_not_parse() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(color_flag(&args(&["neomail", "--bogus"])), None);
        assert_eq!(
            color_flag(&args(&["neomail", "--color", "never", "list", "--bogus"])),
            Some(ColorWhen::Never)
        );
        assert_eq!(
            color_flag(&args(&["neomail", "--color=always", "--color=never"])),
            Some(ColorWhen::Never)
        );
        assert_eq!(
            color_flag(&args(&["neomail", "--", "--color=always"])),
            None
        );
    }

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// `mark`'s state and ids, or clap's usage error (exit 2).
    fn parse_mark(args: &[&str]) -> Result<(nm_core::Action, Vec<IdArg>), i32> {
        let argv = ["neomail", "mark"].iter().chain(args);
        match Cli::try_parse_from(argv) {
            Ok(Cli {
                command: Some(Command::Mark { state, ids, .. }),
                ..
            }) => Ok((state.action(), ids)),
            Ok(_) => panic!("not mark"),
            Err(e) => Err(e.exit_code()),
        }
    }

    #[test]
    fn mark_takes_a_state_then_ids_or_a_dash() {
        use nm_core::Action::{MarkRead, MarkUnread};
        assert_eq!(
            parse_mark(&["read", "4", "2"]),
            Ok((MarkRead, vec![IdArg::Id(4), IdArg::Id(2)]))
        );
        assert_eq!(
            parse_mark(&["unread", "-"]),
            Ok((MarkUnread, vec![IdArg::Stdin]))
        );
        for bad in [
            &["read"][..],
            &["seen", "4"],
            &["read", "abc"],
            &["read", "4", "--json", "--plain"],
        ] {
            assert_eq!(parse_mark(bad), Err(2), "{bad:?}");
        }
    }

    /// `archive`'s or `delete`'s action and ids, or clap's usage error (exit 2).
    fn parse_move(args: &[&str]) -> Result<(nm_core::Action, Vec<IdArg>), i32> {
        match Cli::try_parse_from(["neomail"].iter().chain(args)) {
            Ok(Cli {
                command: Some(Command::Archive { ids, .. }),
                ..
            }) => Ok((nm_core::Action::Archive, ids)),
            Ok(Cli {
                command: Some(Command::Delete { ids, .. }),
                ..
            }) => Ok((nm_core::Action::Delete, ids)),
            Ok(Cli {
                command: Some(Command::Move { folder, ids, .. }),
                ..
            }) => Ok((nm_core::Action::Move(folder), ids)),
            Ok(_) => panic!("not archive, delete or move"),
            Err(e) => Err(e.exit_code()),
        }
    }

    #[test]
    fn archive_and_delete_take_ids_or_a_dash() {
        use nm_core::Action::{Archive, Delete};
        assert_eq!(
            parse_move(&["archive", "4", "2"]),
            Ok((Archive, vec![IdArg::Id(4), IdArg::Id(2)]))
        );
        assert_eq!(
            parse_move(&["delete", "-", "--jsonl"]),
            Ok((Delete, vec![IdArg::Stdin]))
        );
        for bad in [
            &["archive"][..],
            &["delete", "abc"],
            &["archive", "4", "--json", "--plain"],
            &["delete", "4", "--force"],
        ] {
            assert_eq!(parse_move(bad), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn move_takes_a_folder_then_ids_or_a_dash() {
        let receipts = || nm_core::Action::Move(nm_core::FolderName::new("Receipts").unwrap());
        assert_eq!(
            parse_move(&["move", "Receipts", "4", "2"]),
            Ok((receipts(), vec![IdArg::Id(4), IdArg::Id(2)]))
        );
        assert_eq!(
            parse_move(&["move", "Receipts", "-", "--json"]),
            Ok((receipts(), vec![IdArg::Stdin]))
        );
        assert_eq!(
            parse_move(&["move", "[Gmail]/All Mail", "4"]),
            Ok((
                nm_core::Action::Move(nm_core::FolderName::new("[Gmail]/All Mail").unwrap()),
                vec![IdArg::Id(4)]
            ))
        );
        for bad in [
            &["move"][..],
            &["move", "Receipts"],
            &["move", "", "4"],
            &["move", "Receipts", "abc"],
            &["move", "Receipts", "4", "--json", "--plain"],
        ] {
            assert_eq!(parse_move(bad), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn folder_list_takes_an_account_and_a_format() {
        let parse = |args: &[&str]| match Cli::try_parse_from(["neomail"].iter().chain(args)) {
            Ok(Cli {
                command:
                    Some(Command::Folder {
                        command: FolderCommand::List { account, format },
                    }),
                ..
            }) => Ok((account, format.mode(false))),
            Ok(_) => panic!("not folder list"),
            Err(e) => Err(e.exit_code()),
        };
        assert_eq!(parse(&["folder", "list"]), Ok((None, Mode::Plain)));
        assert_eq!(
            parse(&["folder", "list", "--account", "gmail", "--jsonl"]),
            Ok((Some("gmail".to_owned()), Mode::Jsonl))
        );
        for bad in [
            &["folder"][..],
            &["folder", "list", "extra"],
            &["folder", "list", "--json", "--plain"],
        ] {
            assert_eq!(parse(bad), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn a_dash_with_other_ids_is_a_usage_error() {
        for ids in [
            &[IdArg::Id(1), IdArg::Stdin][..],
            &[IdArg::Stdin, IdArg::Stdin],
        ] {
            let e = read_ids(ids).unwrap_err();
            assert!(e.is::<UsageError>(), "{e}");
        }
        assert_eq!(read_ids(&[IdArg::Id(3), IdArg::Id(1)]).unwrap(), [3, 1]);
    }
}
