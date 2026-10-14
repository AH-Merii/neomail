//! `neomail attachment save`: attachments written under their safe names into a directory the
//! user names, never replacing a file unless asked.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use nm_core::attachment::Extracted;
use serde::Serialize;

use super::pick;
use crate::output::{self, Mode};
use crate::style::Colors;

/// One saved attachment, as `save --json` prints it.
#[derive(Serialize)]
pub struct Saved {
    pub id: i64,
    pub index: usize,
    pub path: PathBuf,
    pub size: usize,
}

/// Saves the attachments `indexes` (all of them when empty) of message `id` into `dir`. Each
/// failure is reported on standard error after the others are saved, and makes the exit 1.
#[allow(clippy::too_many_arguments)]
pub fn run(
    id: i64,
    attachments: &[Extracted],
    indexes: &[u64],
    dir: &Path,
    overwrite: bool,
    mode: Mode,
    colors: Colors,
    mut out: &mut dyn Write,
) -> Result<ExitCode> {
    if attachments.is_empty() {
        anyhow::bail!("message {id} has no attachments");
    }
    let dir = std::path::absolute(dir).with_context(|| format!("cannot use {}", dir.display()))?;
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    let mut failures = Vec::new();
    let mut chosen = Vec::new();
    if indexes.is_empty() {
        chosen.extend(attachments);
    }
    for &index in indexes {
        match pick(id, attachments, index) {
            Ok(a) if !chosen.iter().any(|c| std::ptr::eq(*c, a)) => chosen.push(a),
            Ok(_) => {}
            Err(e) => failures.push(format!("{e:#}")),
        }
    }
    let mut saved = Vec::new();
    for a in chosen {
        let path = dir.join(&a.info.safe_name);
        match write_file(&path, &a.data, overwrite) {
            Ok(()) => saved.push(Saved {
                id,
                index: a.info.index,
                path,
                size: a.data.len(),
            }),
            Err(e) => failures.push(format!("attachment {}: {e:#}", a.info.index)),
        }
    }
    output::write_listing(
        &mut out,
        mode,
        &saved,
        |out| {
            for s in &saved {
                writeln!(
                    out,
                    "saved {} to {}",
                    colors.stdout.id(&s.index.to_string()),
                    output::one_line(&s.path.to_string_lossy())
                )?;
            }
            Ok(())
        },
        |out| {
            for s in &saved {
                let index = colors.plain.id(&s.index.to_string());
                writeln!(
                    out,
                    "{index}\t{}",
                    output::one_line(&s.path.to_string_lossy())
                )?;
            }
            Ok(())
        },
    )?;
    out.flush()?;
    for f in &failures {
        eprintln!("{}: {f}", colors.stderr.error("neomail"));
    }
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(crate::EXIT_FAILURE)
    })
}

/// Writes `data` to `path` with mode 0600, never through a symbolic link. Without `overwrite`
/// an existing file (or link) stays and is an error; with it, the data goes to a temporary file
/// in the same directory that then replaces `path`, so a failed write leaves the old file whole.
pub fn write_file(path: &Path, data: &[u8], overwrite: bool) -> Result<()> {
    if !overwrite {
        let mut file = create_new(path).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => {
                anyhow::anyhow!("{} exists; --overwrite replaces it", path.display())
            }
            _ => anyhow::Error::new(e).context(format!("cannot create {}", path.display())),
        })?;
        return finish(&mut file, data).inspect_err(|_| {
            let _ = fs::remove_file(path);
        });
    }
    let name = path.file_name().context("no file name")?.to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.neomail-{}.part", std::process::id()));
    let mut file =
        create_new(&temp).with_context(|| format!("cannot create {}", temp.display()))?;
    finish(&mut file, data)
        .and_then(|()| {
            fs::rename(&temp, path).with_context(|| format!("cannot replace {}", path.display()))
        })
        .inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
}

/// Creates `path` for writing, failing when anything is there already; a symbolic link counts,
/// and is never followed.
fn create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

fn finish(file: &mut File, data: &[u8]) -> Result<()> {
    file.write_all(data).context("cannot write the file")?;
    file.sync_all().context("cannot write the file")
}
