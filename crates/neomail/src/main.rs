//! `neomail`: the neomail command line: a read-only INBOX (sync, list, show), passwords in the OS
//! keyring, Gmail through the user's own OAuth client, and setup (`account add`, `account list`,
//! `config path|check`).

mod account;
mod browser;
mod config;
mod keyring;
mod oauth;
mod output;
mod secret;
mod setup;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use nm_core::{Cache, MessageId, sync_inbox_with_progress};
use nm_imap::{CertPin, ImapSource, Security};

use crate::config::{Account, Config, Login, PasswordSource};
use crate::keyring::Keyring;
use crate::secret::{Input, Password};

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
  neomail config check              Check the config file

Guide: https://github.com/AH-Merii/neomail#try-it";

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

Secrets are never flags: they are prompted for without echo, read as one line of standard input,
or come from --password-command.";

#[derive(Parser)]
#[command(
    name = "neomail",
    version,
    about = "neomail: a terminal email client (read-only INBOX for now)",
    after_help = TOP_EXAMPLES
)]
struct Cli {
    /// Config file [env: NEOMAIL_CONFIG] [default: $XDG_CONFIG_HOME/neomail/config.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Never prompt, even on a terminal; a value that would be asked for is a usage error [env: NEOMAIL_NO_INPUT=1]
    #[arg(long, global = true)]
    no_input: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch new mail into the local cache, read-only: nothing changes on the server
    Sync {
        /// Sync only this account
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// How many of the newest messages the first sync takes per account (later syncs fetch all new mail)
        #[arg(long, value_name = "N", default_value_t = nm_core::sync::DEFAULT_LIMIT)]
        limit: NonZeroU32,
    },
    /// List cached messages, newest first ('*' marks unread)
    List {
        /// List only this account
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// The most messages to list
        #[arg(long, value_name = "N", default_value_t = NonZeroU32::MIN.saturating_add(49))]
        limit: NonZeroU32,
        /// Print JSON (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Show one cached message by the id from `neomail list`
    Show {
        /// The message's id, the first column of `neomail list`
        id: i64,
        /// Print JSON (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
        #[arg(long, conflicts_with = "raw")]
        json: bool,
        /// Print the RFC 822 source
        #[arg(long)]
        raw: bool,
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
    Add(AddCommand),
    /// List the configured accounts (reads only the config: no keyring, no network)
    List,
    /// Sign an OAuth account in again, replacing its saved sign-in
    Login {
        /// The account's name in the config
        #[arg(value_name = "NAME")]
        name: String,
        /// Print the sign-in URL without opening a browser
        #[arg(long)]
        no_browser: bool,
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
    /// The account's name in neomail [default: gmail, proton; required for imap]
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// gmail: the address to sign in with
    #[arg(long, value_name = "ADDR")]
    email: Option<String>,
    /// gmail: your OAuth client's ID (the secret is prompted for, or read from stdin)
    #[arg(long, value_name = "ID")]
    client_id: Option<String>,
    /// gmail: print the sign-in URL without opening a browser
    #[arg(long)]
    no_browser: bool,
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
    /// imap, proton: the server certificate's SHA-256 to pin (64 hex digits, colons allowed)
    #[arg(long, value_name = "HEX", value_parser = parse_pin)]
    cert_sha256: Option<CertPin>,
    /// imap, proton, gmail-app-password: skip the read-only login before the config is written
    #[arg(long)]
    no_check: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum AddProvider {
    /// Gmail, through your own Google OAuth app (https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md)
    Gmail,
    /// Gmail with an app password over IMAP (needs Google 2-Step Verification)
    GmailAppPassword,
    /// Proton Mail, through Proton Mail Bridge on this computer
    Proton,
    /// Any IMAP server, with a password
    Imap,
}

impl AddProvider {
    const ALL: [Self; 4] = [
        Self::Gmail,
        Self::GmailAppPassword,
        Self::Proton,
        Self::Imap,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::GmailAppPassword => setup::GMAIL_APP_PASSWORD.id,
            Self::Proton => setup::PROTON.id,
            Self::Imap => setup::IMAP.id,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail, through your own Google OAuth app",
            Self::GmailAppPassword => "Gmail with an app password (needs 2-Step Verification)",
            Self::Proton => "Proton Mail, through Proton Mail Bridge",
            Self::Imap => "any IMAP server, with a password",
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
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("neomail: {e:#}");
            if e.chain().any(|c| c.is::<UsageError>()) {
                ExitCode::from(EXIT_USAGE)
            } else {
                ExitCode::from(EXIT_FAILURE)
            }
        }
    }
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

fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let input = Input::detect(cli.no_input || no_input_env());
    let Some(command) = cli.command else {
        let path = config::config_path(cli.config)?;
        if config::exists(&path) {
            eprint!("{}", Cli::command().render_help());
        } else {
            eprint!("{}", start_text());
        }
        return Ok(ExitCode::from(EXIT_USAGE));
    };
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let code = match command {
        Command::Sync { account, limit } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            let accounts = config.select(account.as_deref())?;
            let mut cache = open_cache(true)?.context("the cache could not be created")?;
            let runtime = account::runtime()?;
            let stderr = io::stderr();
            let mut progress = output::ProgressLine::new(stderr.is_terminal());
            let mut failed = false;
            let mut keyring = None;
            for account in accounts {
                let progress = &mut progress;
                // Keyring reads stay outside block_on: their blocking calls start a runtime of
                // their own.
                let result = match &account.login {
                    Login::Password(source) => secret::account_password(
                        &account.name,
                        source,
                        &mut keyring,
                    )
                    .and_then(|password| {
                        let login = nm_imap::Login::Password(password.expose());
                        runtime.block_on(sync_account(account, login, &mut cache, limit, progress))
                    }),
                    Login::OAuth(settings) => secret::oauth_secrets(&account.name, &mut keyring)
                        .and_then(|(client_secret, refresh_token)| {
                            runtime.block_on(async {
                                let token = account::access_token(
                                    account,
                                    settings,
                                    client_secret,
                                    &refresh_token,
                                )
                                .await?;
                                let login = nm_imap::Login::XOAuth2 {
                                    access_token: token.expose(),
                                };
                                sync_account(account, login, &mut cache, limit, progress).await
                            })
                        }),
                };
                progress.clear(&mut io::stderr());
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
        Command::List {
            account,
            limit,
            json,
        } => {
            let entries = match open_cache(false)? {
                Some(cache) => {
                    if let Some(name) = account.as_deref() {
                        check_cached_account(&cache, name)?;
                    }
                    cache.list(account.as_deref(), limit.get())?
                }
                None if account.is_some() => bail!("{}", no_cache_hint(cli.config)),
                None => Vec::new(),
            };
            if json {
                let items: Vec<_> = entries.iter().map(output::ListItem::from).collect();
                output::write_json(&mut out, &items)?;
            } else if entries.is_empty() {
                let hint = match no_cache_hint(cli.config) {
                    h if h.starts_with("nothing") => {
                        "no cached messages; run `neomail sync` first".to_owned()
                    }
                    h => h,
                };
                eprintln!("neomail: {hint}");
            } else {
                output::write_list(&mut out, &entries)?;
            }
            ExitCode::SUCCESS
        }
        Command::Show { id, json, raw } => {
            let Some(cache) = open_cache(false)? else {
                bail!("{}", no_cache_hint(cli.config));
            };
            let stored = cache
                .get(MessageId::new(id))?
                .with_context(|| format!("no cached message with id {id}; see `neomail list`"))?;
            if raw {
                out.write_all(&stored.raw)?;
            } else {
                let parsed = nm_core::parse::parse(&stored.raw)
                    .with_context(|| format!("message {id} cannot be read; try --raw"))?;
                let item = output::ShowItem::new(&stored, &parsed);
                if json {
                    output::write_json(&mut out, &item)?;
                } else {
                    output::write_show(&mut out, &item)?;
                }
            }
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
            add_account(add, &path, input, &mut out)?;
            ExitCode::SUCCESS
        }
        Command::Account {
            command: AccountCommand::List,
        } => {
            let path = config::config_path(cli.config)?;
            if !config::exists(&path) {
                eprintln!("neomail: no accounts yet; {}", config::ADD_HINT);
                return Ok(ExitCode::SUCCESS);
            }
            let config = Config::load(&path)?;
            if config.accounts.is_empty() {
                eprintln!("neomail: no accounts yet; {}", config::ADD_HINT);
            }
            output::write_accounts(&mut out, &config.accounts)?;
            ExitCode::SUCCESS
        }
        Command::Account { command } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            match command {
                AccountCommand::Add(_) | AccountCommand::List => unreachable!("handled above"),
                AccountCommand::Login { name, no_browser } => {
                    account::login(config.select(Some(&name))?[0], no_browser, input, &mut out)?;
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
    let oauth_only = [
        ("--email", add.email.is_some()),
        ("--client-id", add.client_id.is_some()),
        ("--no-browser", add.no_browser),
    ];
    let password_only = [
        ("--username", add.username.is_some()),
        ("--password-command", add.password_command.is_some()),
        ("--no-check", add.no_check),
    ];
    let imap_only = [
        ("--host", add.host.is_some()),
        ("--security", add.security.is_some()),
    ];
    let port_pin = [
        ("--port", add.port.is_some()),
        ("--cert-sha256", add.cert_sha256.is_some()),
    ];
    let refused: Vec<(&str, bool)> = match provider {
        AddProvider::Gmail => [&password_only[..], &imap_only, &port_pin].concat(),
        AddProvider::GmailAppPassword => [&oauth_only[..], &imap_only, &port_pin].concat(),
        AddProvider::Proton => [&oauth_only[..], &imap_only].concat(),
        AddProvider::Imap => oauth_only.to_vec(),
    };
    if let Some((flag, _)) = refused.iter().find(|(_, given)| *given) {
        return Err(usage(format!(
            "{flag} does not apply to `neomail account add {}`",
            provider.id()
        )));
    }
    let preset = match provider {
        AddProvider::Gmail => {
            let args = account::AddArgs {
                name: add.name,
                email: add.email,
                client_id: add.client_id,
                no_browser: add.no_browser,
            };
            return account::add(&oauth::GOOGLE, path, args, input, out);
        }
        AddProvider::GmailAppPassword => &setup::GMAIL_APP_PASSWORD,
        AddProvider::Proton => &setup::PROTON,
        AddProvider::Imap => &setup::IMAP,
    };
    let args = setup::Args {
        name: add.name,
        username: add.username,
        host: add.host,
        port: add.port,
        security: add.security.map(|s| match s {
            SecurityArg::Tls => Security::Tls,
            SecurityArg::Starttls => Security::StartTls,
        }),
        password_command: add.password_command,
        cert_sha256: add.cert_sha256,
        no_check: add.no_check,
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
        "Stored the password for {:?} in {} (service {:?}, user {:?}).",
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

async fn sync_account(
    account: &Account,
    login: nm_imap::Login<'_>,
    cache: &mut Cache,
    limit: NonZeroU32,
    progress: &mut output::ProgressLine,
) -> Result<String> {
    let mut source = ImapSource::connect(&account.imap, login).await?;
    let result = sync_inbox_with_progress(&mut source, cache, &account.name, limit, |p| {
        progress.update(&mut io::stderr(), &account.name, p);
    })
    .await;
    source.logout().await;
    let r = result?;
    let reset = if r.reset {
        " (UIDVALIDITY changed; cache rebuilt)"
    } else {
        ""
    };
    let missing = if r.missing > 0 {
        format!(", {} without a body (retried next sync)", r.missing)
    } else {
        String::new()
    };
    Ok(format!(
        "{}: {} new, {} updated, {} removed{missing}{reset}",
        account.name, r.new, r.updated, r.removed
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_answers_are_a_number_or_a_name() {
        assert!(matches!(parse_choice("1"), Some(AddProvider::Gmail)));
        assert!(matches!(parse_choice(" 3 "), Some(AddProvider::Proton)));
        assert!(matches!(parse_choice("IMAP"), Some(AddProvider::Imap)));
        assert!(matches!(
            parse_choice("gmail-app-password"),
            Some(AddProvider::GmailAppPassword)
        ));
        for bad in ["", "0", "5", "outlook"] {
            assert!(parse_choice(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
