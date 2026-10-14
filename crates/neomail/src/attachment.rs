//! `neomail attachment list|save|open`: a cached message's attachments, offline. What each one
//! is comes from nm_core::attachment; this module prints it, writes files and hands a file to the
//! desktop's opener (`NEOMAIL_OPENER`, else `open` on macOS, `wslview` on WSL, else `xdg-open`).
//! neomail never runs an attachment itself.

mod open;
mod save;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use nm_core::MessageId;
use nm_core::attachment::{Extracted, Info};
use serde::Serialize;

use crate::output::{self, Mode};
use crate::secret::Input;
use crate::style::{Colors, Paint};
use crate::{ListingFormat, no_cache_hint, open_cache};

pub const EXAMPLES: &str = "\
Examples:
  neomail attachment list 42        List message 42's attachments
  neomail attachment save 42        Save them all into the current directory
  neomail attachment save 42 2 --dir ~/Downloads
                                    Save attachment 2 into ~/Downloads
  neomail attachment open 42 1      Open attachment 1 with the desktop's default app

Attachments come from the cache: nothing is fetched. Each is named by its index, from 1, with
the type the sender declared and its real type, told from its content; `mismatch` marks a real
type that contradicts the declared type or the name's extension, `inline` an image the message's
HTML shows, and `executable` an executable or script, by content or by name. Saved files get a
safe name (no directories, control or bidi characters, leading dot or dash), mode 0600, and never
replace a file unless --overwrite says so. `open` writes the file into neomail's data directory
and hands it to NEOMAIL_OPENER, or else `open` on macOS, `wslview` on WSL and `xdg-open`
elsewhere. It asks before opening an executable or script, and refuses one without a terminal
unless --yes is given. neomail never runs a file itself.";

#[derive(Subcommand)]
pub enum Command {
    /// List a cached message's attachments: index, name, real type, declared type and size
    List {
        /// The message id, the first column of `neomail list`
        #[arg(value_name = "ID")]
        id: i64,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Save a cached message's attachments, all or those named by index, under safe names
    Save {
        /// The message id, the first column of `neomail list`
        #[arg(value_name = "ID")]
        id: i64,
        /// Attachment indexes, the first column of `neomail attachment list`; none saves them all
        #[arg(value_name = "INDEX", value_parser = clap::value_parser!(u64).range(1..))]
        indexes: Vec<u64>,
        /// The directory to save into [default: the current directory]
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Replace a file that already exists
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        format: ListingFormat,
    },
    /// Open one attachment with the desktop's default app, asking first for an executable or script
    Open {
        /// The message id, the first column of `neomail list`
        #[arg(value_name = "ID")]
        id: i64,
        /// The attachment's index, the first column of `neomail attachment list`
        #[arg(value_name = "INDEX", value_parser = clap::value_parser!(u64).range(1..))]
        index: u64,
        /// Open an executable or script without asking
        #[arg(long)]
        yes: bool,
        /// Print a JSON object: the file opened and the opener (https://github.com/AH-Merii/neomail/blob/main/docs/json.md)
        #[arg(long)]
        json: bool,
    },
}

/// One attachment in `list --json`: its message, then what nm_core tells about it.
#[derive(Serialize)]
struct Listed<'a> {
    id: i64,
    account: &'a str,
    #[serde(flatten)]
    info: &'a Info,
}

/// Runs `neomail attachment`.
pub fn run(
    command: Command,
    config: Option<PathBuf>,
    input: Input,
    terminal: bool,
    colors: Colors,
    mut out: &mut dyn Write,
) -> Result<ExitCode> {
    match command {
        Command::List { id, format } => {
            let (account, attachments) = load(id, config)?;
            let mode = format.mode(terminal);
            if attachments.is_empty() && matches!(mode, Mode::Human | Mode::Plain) {
                eprintln!("neomail: message {id} has no attachments");
            }
            let items: Vec<_> = attachments
                .iter()
                .map(|a| Listed {
                    id,
                    account: &account,
                    info: &a.info,
                })
                .collect();
            output::write_listing(
                &mut out,
                mode,
                &items,
                |out| write_human(out, &attachments, colors.stdout),
                |out| write_plain(out, &attachments, colors.plain),
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Save {
            id,
            indexes,
            dir,
            overwrite,
            format,
        } => {
            let (_, attachments) = load(id, config)?;
            let dir = match dir {
                Some(d) => d,
                None => std::env::current_dir().context("cannot read the current directory")?,
            };
            save::run(
                id,
                &attachments,
                &indexes,
                &dir,
                overwrite,
                format.mode(terminal),
                colors,
                out,
            )
        }
        Command::Open {
            id,
            index,
            yes,
            json,
        } => {
            let (_, attachments) = load(id, config)?;
            let attachment = pick(id, &attachments, index)?;
            open::run(id, attachment, yes, json, input, terminal, out)
        }
    }
}

/// The account and the attachments of cached message `id`.
fn load(id: i64, config: Option<PathBuf>) -> Result<(String, Vec<Extracted>)> {
    let Some(cache) = open_cache(false)? else {
        bail!("{}", no_cache_hint(config));
    };
    let Some(stored) = cache.get(MessageId::new(id))? else {
        bail!("no cached message with id {id}; see `neomail list`");
    };
    let attachments = nm_core::attachment::extract(&stored.raw).map_err(|_| {
        anyhow::anyhow!("message {id} cannot be read; try `neomail show --raw {id}`")
    })?;
    Ok((stored.account, attachments))
}

/// Attachment `index` (from 1) of message `id`, or a failure naming how many there are.
fn pick(id: i64, attachments: &[Extracted], index: u64) -> Result<&Extracted> {
    usize::try_from(index)
        .ok()
        .and_then(|i| attachments.get(i.checked_sub(1)?))
        .with_context(|| match attachments.len() {
            0 => format!("message {id} has no attachments"),
            1 => format!("message {id} has no attachment {index}; it has one, 1"),
            n => format!("message {id} has no attachment {index}; it has 1 to {n}"),
        })
}

/// What `list` notes after the size: `inline`, `mismatch` (with the declared type) and
/// `executable`, comma-separated.
fn notes(info: &Info) -> String {
    let mut notes = Vec::new();
    if info.inline {
        notes.push("inline".to_owned());
    }
    if info.type_mismatch {
        notes.push("mismatch".to_owned());
    }
    if info.executable {
        notes.push("executable".to_owned());
    }
    notes.join(",")
}

/// One line per attachment: index, name, real type, size and notes, the declared type when it
/// differs from the real one.
fn write_human(
    out: &mut dyn Write,
    attachments: &[Extracted],
    paint: Paint,
) -> std::io::Result<()> {
    let width = attachments
        .iter()
        .map(|a| a.info.safe_name.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    for a in attachments {
        let i = &a.info;
        let mut line = format!(
            "{}  {:<width$}  {}  {}",
            paint.id(&format!("{:>3}", i.index)),
            output::one_line(&i.safe_name),
            output::one_line(&i.real_type),
            output::size(i.size),
        );
        if i.declared_type != i.real_type {
            line.push_str(&format!(
                "  (declared {})",
                output::one_line(&i.declared_type)
            ));
        }
        let notes = notes(i);
        if !notes.is_empty() {
            line.push_str(&format!("  {}", paint.strong(&notes.replace(',', ", "))));
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// TSV: index, safe name, real type, declared type, size in bytes, notes.
fn write_plain(
    out: &mut dyn Write,
    attachments: &[Extracted],
    paint: Paint,
) -> std::io::Result<()> {
    for a in attachments {
        let i = &a.info;
        let fields = [
            paint.id(&i.index.to_string()),
            output::one_line(&i.safe_name),
            output::one_line(&i.real_type),
            output::one_line(&i.declared_type),
            i.size.to_string(),
            notes(i),
        ];
        writeln!(out, "{}", fields.join("\t"))?;
    }
    Ok(())
}
