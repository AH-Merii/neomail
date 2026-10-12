//! `neomail`: the neomail command line: a read-only INBOX (sync, list, show), passwords in the OS
//! keyring, and Gmail through the user's own OAuth client.

mod account;
mod browser;
mod config;
mod keyring;
mod oauth;
mod output;
mod secret;

use std::io::{self, BufWriter, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use nm_core::{Cache, MessageId, sync_inbox};
use nm_imap::ImapSource;

use crate::config::{Account, Config, Login, PasswordSource};
use crate::keyring::Keyring;
use crate::secret::Password;

/// Exit status for a failure other than bad usage (clap exits with 2 for that).
const EXIT_FAILURE: u8 = 1;
const CACHE_FILE: &str = "cache.sqlite3";

#[derive(Parser)]
#[command(
    name = "neomail",
    version,
    about = "neomail: a terminal email client (read-only INBOX for now)"
)]
struct Cli {
    /// Config file [env: NEOMAIL_CONFIG] [default: $XDG_CONFIG_HOME/neomail/config.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
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
        /// Print JSON (see docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Show one cached message by the id from `neomail list`
    Show {
        id: i64,
        /// Print JSON (see docs/json.md)
        #[arg(long, conflicts_with = "raw")]
        json: bool,
        /// Print the RFC 822 source
        #[arg(long)]
        raw: bool,
    },
    /// Add accounts, sign in and out, and manage passwords in the OS keyring
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Add an account: guides you through creating your own OAuth client, then signs in
    Add {
        /// The provider
        provider: AddProvider,
        /// The account's name in neomail [default: gmail]
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// The address to sign in with
        #[arg(long, value_name = "ADDR")]
        email: Option<String>,
        /// Your OAuth client's ID (the secret is prompted for, or read from stdin)
        #[arg(long, value_name = "ID")]
        client_id: Option<String>,
        /// Print the sign-in URL without opening a browser
        #[arg(long)]
        no_browser: bool,
    },
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

#[derive(Clone, Copy, ValueEnum)]
enum AddProvider {
    /// Gmail, through your own Google OAuth client (see docs/gmail.md)
    Gmail,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("neomail: {e:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let code = match cli.command {
        Command::Sync { account, limit } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            let accounts = config.select(account.as_deref())?;
            let mut cache = open_cache(true)?.context("the cache could not be created")?;
            let runtime = account::runtime()?;
            let mut failed = false;
            let mut keyring = None;
            for account in accounts {
                // Keyring reads stay outside block_on: their blocking calls start a runtime of
                // their own.
                let result = match &account.login {
                    Login::Password(source) => {
                        secret::account_password(&account.name, source, &mut keyring).and_then(
                            |password| {
                                let login = nm_imap::Login::Password(password.expose());
                                runtime.block_on(sync_account(account, login, &mut cache, limit))
                            },
                        )
                    }
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
                                sync_account(account, login, &mut cache, limit).await
                            })
                        }),
                };
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
                None if account.is_some() => {
                    bail!("nothing is cached yet; run `neomail sync` first")
                }
                None => Vec::new(),
            };
            if json {
                let items: Vec<_> = entries.iter().map(output::ListItem::from).collect();
                output::write_json(&mut out, &items)?;
            } else if entries.is_empty() {
                eprintln!("neomail: no cached messages; run `neomail sync` first");
            } else {
                output::write_list(&mut out, &entries)?;
            }
            ExitCode::SUCCESS
        }
        Command::Account {
            command:
                AccountCommand::Add {
                    provider: AddProvider::Gmail,
                    name,
                    email,
                    client_id,
                    no_browser,
                },
        } => {
            let args = account::AddArgs {
                name,
                email,
                client_id,
                no_browser,
            };
            let path = config::config_path(cli.config)?;
            account::add(&oauth::GOOGLE, &path, args, &mut out)?;
            ExitCode::SUCCESS
        }
        Command::Account { command } => {
            let config = Config::load(&config::config_path(cli.config)?)?;
            match command {
                AccountCommand::Add { .. } => unreachable!("handled above"),
                AccountCommand::Login { name, no_browser } => {
                    account::login(config.select(Some(&name))?[0], no_browser, &mut out)?;
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
                    let password = secret::read_new_password(&account.name)?;
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
        Command::Show { id, json, raw } => {
            let cache =
                open_cache(false)?.context("nothing is cached yet; run `neomail sync` first")?;
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
    };
    out.flush()?;
    Ok(code)
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
) -> Result<String> {
    let mut source = ImapSource::connect(&account.imap, login).await?;
    let result = sync_inbox(&mut source, cache, &account.name, limit).await;
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
