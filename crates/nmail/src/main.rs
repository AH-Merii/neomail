//! `nmail`: the neomail command line. Slice 1 is a read-only INBOX: sync, list, show.

mod config;
mod output;
mod secret;

use std::io::{self, BufWriter, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nm_core::{Cache, MessageId, sync_inbox};
use nm_imap::ImapSource;

use crate::config::{Account, Config};

/// Exit status for a failure other than bad usage (clap exits with 2 for that).
const EXIT_FAILURE: u8 = 1;
const CACHE_FILE: &str = "cache.sqlite3";

#[derive(Parser)]
#[command(
    name = "nmail",
    version,
    about = "neomail: a terminal email client (read-only INBOX for now)"
)]
struct Cli {
    /// Config file [env: NMAIL_CONFIG] [default: $XDG_CONFIG_HOME/neomail/config.toml]
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
    /// Show one cached message by the id from `nmail list`
    Show {
        id: i64,
        /// Print JSON (see docs/json.md)
        #[arg(long, conflicts_with = "raw")]
        json: bool,
        /// Print the RFC 822 source
        #[arg(long)]
        raw: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nmail: {e:#}");
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
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("cannot start the async runtime")?;
            let mut failed = false;
            for account in accounts {
                match runtime.block_on(sync_account(account, &mut cache, limit)) {
                    Ok(line) => {
                        writeln!(out, "{line}")?;
                        out.flush()?;
                    }
                    Err(e) => {
                        out.flush()?;
                        eprintln!("nmail: {}: {e:#}", account.name);
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
                None if account.is_some() => bail!("nothing is cached yet; run `nmail sync` first"),
                None => Vec::new(),
            };
            if json {
                let items: Vec<_> = entries.iter().map(output::ListItem::from).collect();
                output::write_json(&mut out, &items)?;
            } else if entries.is_empty() {
                eprintln!("nmail: no cached messages; run `nmail sync` first");
            } else {
                output::write_list(&mut out, &entries)?;
            }
            ExitCode::SUCCESS
        }
        Command::Show { id, json, raw } => {
            let cache =
                open_cache(false)?.context("nothing is cached yet; run `nmail sync` first")?;
            let stored = cache
                .get(MessageId::new(id))?
                .with_context(|| format!("no cached message with id {id}; see `nmail list`"))?;
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

async fn sync_account(account: &Account, cache: &mut Cache, limit: NonZeroU32) -> Result<String> {
    let password = secret::run_password_command(&account.name, &account.password_command)?;
    let mut source = ImapSource::connect(&account.imap, password.expose()).await?;
    drop(password);
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
