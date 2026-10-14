//! `neomail search`: the cached mail that matches a query, printed as `neomail list` prints.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use nm_core::{Cache, GroupFilter, MessageId, Query, Scope};

use crate::output::{self, Mode};
use crate::style::Colors;
use crate::{Limit, ListingFormat, config};

/// Messages indexed per transaction: a stopped run keeps what it committed.
pub const INDEX_BATCH: usize = 500;

#[derive(clap::Args)]
pub struct Args {
    /// What to find: words (each a prefix, all must match), "quoted phrases", and subject:WORD, from:WORD or to:WORD
    #[arg(value_name = "QUERY")]
    pub query: Vec<String>,
    /// Search only this account
    #[arg(long, value_name = "NAME")]
    pub account: Option<String>,
    /// Search only this group's messages (`neomail classify` sorts them), or unclassified
    #[arg(long, value_name = "GROUP", value_parser = crate::parse_group)]
    pub group: Option<GroupFilter>,
    /// Search only the thread of message ID, in its account; without a query, list the thread
    #[arg(long, value_name = "ID")]
    pub thread: Option<i64>,
    /// The most messages to print, or all
    #[arg(long, value_name = "N|all", value_parser = crate::parse_limit, default_value_t = Limit::Count(std::num::NonZeroU32::MIN.saturating_add(49)))]
    pub limit: Limit,
    #[command(flatten)]
    pub format: ListingFormat,
}

pub fn run(
    args: &Args,
    config_path: Option<PathBuf>,
    terminal: bool,
    colors: Colors,
    out: &mut impl Write,
) -> Result<ExitCode> {
    let query = Query::parse(&args.query.join(" ")).map_err(|e| crate::usage(e.to_string()))?;
    if query.is_empty() && args.thread.is_none() {
        let reason = if args.query.is_empty() {
            "give words to search for, or --thread ID"
        } else {
            "the query has no letter or digit to search for"
        };
        return Err(crate::usage(reason));
    }
    let rules = config::load_optional(config_path.clone(), false)?
        .map(|c| c.classify.rules)
        .unwrap_or_default();
    let mode = args.format.mode(terminal);
    let mut cached = true;
    let entries = match crate::open_cache(false)? {
        Some(mut cache) => {
            if let Some(name) = args.account.as_deref() {
                crate::check_cached_account(&cache, name)?;
            }
            let thread = args.thread.map(MessageId::new);
            if let Some(id) = thread
                && cache.locate(&[id])?.is_empty()
            {
                bail!("no cached message {id}");
            }
            index(&mut cache)?;
            let scope = Scope {
                account: args.account.clone(),
                group: args.group.unwrap_or(GroupFilter::Any),
                thread,
            };
            cache.search(
                &query,
                &scope,
                args.limit.get().get(),
                &rules,
                chrono::Utc::now(),
            )?
        }
        None if args.account.is_some() || args.thread.is_some() => {
            bail!("{}", crate::no_cache_hint(config_path))
        }
        None => {
            cached = false;
            if matches!(mode, Mode::Human | Mode::Plain) {
                let hint = match crate::no_cache_hint(config_path) {
                    h if h.starts_with("nothing") => {
                        "no cached messages; run `neomail sync` first".to_owned()
                    }
                    h => h,
                };
                eprintln!("neomail: {hint}");
            }
            Vec::new()
        }
    };
    if entries.is_empty() && cached && matches!(mode, Mode::Human | Mode::Plain) {
        eprintln!("neomail: no cached messages match; search covers the INBOX mail synced so far");
    }
    let items: Vec<_> = entries.iter().map(output::ListItem::from).collect();
    output::write_listing(
        out,
        mode,
        &items,
        |out| output::write_list(out, &entries, crate::terminal_width(), colors.stdout),
        |out| output::write_list_plain(out, &entries, colors.plain),
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Indexes what is not yet indexed (mail cached before search existed, or by a sync that could
/// not index it), with a counter on standard error when it is a terminal.
fn index(cache: &mut Cache) -> Result<()> {
    let total = cache.unindexed_count()?;
    let shown = total > INDEX_BATCH && io::stderr().is_terminal();
    let mut done = 0;
    loop {
        let n = cache.index_pending(INDEX_BATCH)?;
        if n == 0 {
            break;
        }
        done += n;
        if shown {
            eprint!("\r\x1b[2Kneomail: indexing for search {done}/{total}");
        }
    }
    if shown {
        eprint!("\r\x1b[2K");
    }
    Ok(())
}
