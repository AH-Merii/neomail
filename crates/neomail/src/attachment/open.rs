//! `neomail attachment open`: the attachment written to a private place and handed to the
//! desktop's opener, after a warning and a question for an executable or script.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use nm_core::attachment::{Extracted, Info};
use serde::Serialize;

use crate::secret::{self, Input};

/// The directory under neomail's data directory that opened attachments are written to.
const OPEN_DIR: &str = "attachments";

/// How long an opened attachment's copy is kept: an app may read the file well after the opener
/// returned, so copies are removed by a later `open`, not by this one.
const KEEP: Duration = Duration::from_secs(24 * 60 * 60);

/// What `open --json` prints.
#[derive(Serialize)]
struct Opened<'a> {
    id: i64,
    index: usize,
    path: &'a Path,
    opener: &'a str,
}

pub fn run(
    id: i64,
    attachment: &Extracted,
    yes: bool,
    json: bool,
    input: Input,
    terminal: bool,
    out: &mut dyn Write,
) -> Result<ExitCode> {
    let info = &attachment.info;
    let name = crate::output::one_line(&info.safe_name);
    if !confirm(info, yes, input, secret::prompt_line)? {
        eprintln!("neomail: nothing was opened");
        return Ok(ExitCode::from(crate::EXIT_FAILURE));
    }
    let root = crate::config::data_dir()?.join(OPEN_DIR);
    remove_old(&root, SystemTime::now());
    let path = write_private(&root, id, attachment)?;
    let words = opener(
        std::env::var("NEOMAIL_OPENER").ok().as_deref(),
        cfg!(target_os = "macos"),
        crate::keyring::is_wsl(),
        crate::browser::on_path,
    );
    let program = launch(&words, &path, terminal && !json)?;
    if json {
        crate::output::write_json(
            out,
            &Opened {
                id,
                index: info.index,
                path: &path,
                opener: &program,
            },
        )?;
    } else {
        writeln!(out, "{}", crate::output::one_line(&path.to_string_lossy()))?;
    }
    eprintln!(
        "neomail: opened {name} with {}; this copy is removed after a day, \
         `neomail attachment save` keeps one",
        crate::output::one_line(&program)
    );
    Ok(ExitCode::SUCCESS)
}

/// Whether to go on and open the attachment. An executable or script is warned about, then asked
/// about with `ask` on a terminal, and refused off one (a usage error) unless `yes` is set; an
/// attachment whose content contradicts its name or declared type is warned about.
pub fn confirm(
    info: &Info,
    yes: bool,
    input: Input,
    ask: impl FnOnce(&str) -> Result<String>,
) -> Result<bool> {
    let name = crate::output::one_line(&info.safe_name);
    if info.executable {
        eprintln!(
            "neomail: warning: {name} is an executable or a script (its content is {}); \
             opening it may run it",
            crate::output::one_line(&info.real_type)
        );
        if yes {
            return Ok(true);
        }
        if !input.can_prompt() {
            return Err(crate::usage(format!(
                "nothing was opened: {}; pass --yes to open it anyway",
                input.why_not()
            )));
        }
        let answer = ask("Open it anyway? [y/N] ").context("nothing was opened")?;
        return Ok(said_yes(&answer));
    }
    if info.type_mismatch {
        eprintln!(
            "neomail: warning: {name}'s content is {}, not what its name or declared type ({}) says",
            crate::output::one_line(&info.real_type),
            crate::output::one_line(&info.declared_type)
        );
    }
    Ok(true)
}

/// Whether the answer is a yes: `y` or `yes` in any case.
fn said_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// The opener's command words, before the file: `NEOMAIL_OPENER` split on whitespace when set and
/// not blank, else `open` on macOS, `wslview` on WSL when installed, else `xdg-open`.
pub fn opener(
    configured: Option<&str>,
    macos: bool,
    wsl: bool,
    on_path: impl Fn(&str) -> bool,
) -> Vec<String> {
    if let Some(words) = configured
        .map(|c| c.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
    {
        return words;
    }
    let program = if macos {
        "open"
    } else if wsl && on_path("wslview") {
        "wslview"
    } else {
        "xdg-open"
    };
    vec![program.to_owned()]
}

/// Runs the opener on `path` and waits for it (desktop openers return once the app started).
/// Its output goes to the terminal when `inherit` is set, and nowhere otherwise, so standard
/// output keeps only results. Returns the program's name.
fn launch(words: &[String], path: &Path, inherit: bool) -> Result<String> {
    let (program, args) = words.split_first().context("no opener")?;
    let output = || {
        if inherit {
            Stdio::inherit()
        } else {
            Stdio::null()
        }
    };
    let status = Command::new(program)
        .args(args)
        .arg(path)
        .stdout(output())
        .stderr(output())
        .status()
        .with_context(|| format!("cannot run {program}; the file is at {}", path.display()))?;
    if !status.success() {
        bail!(
            "{program} failed ({status}); the file is at {}",
            path.display()
        );
    }
    Ok(program.clone())
}

/// Writes the attachment to `root/ID-INDEX/NAME`: the directories 0700, the file 0600, an earlier
/// copy replaced.
fn write_private(root: &Path, id: i64, attachment: &Extracted) -> Result<PathBuf> {
    let dir = root.join(format!("{id}-{}", attachment.info.index));
    for d in [root, dir.as_path()] {
        crate::create_private_dir(d)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(d, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("cannot restrict {}", d.display()))?;
        }
    }
    let path = dir.join(&attachment.info.safe_name);
    super::save::write_file(&path, &attachment.data, true)?;
    Ok(path)
}

/// Removes the copies under `root` whose newest file was last written more than [`KEEP`] before
/// `now`, so a copy edited in place lives a day from that edit. Best effort: a copy that cannot be
/// removed stays until the next time.
pub fn remove_old(root: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let newest = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| f.metadata().and_then(|m| m.modified()).ok())
            .chain(entry.metadata().and_then(|m| m.modified()).ok())
            .max();
        let old = newest.is_some_and(|t| now.duration_since(t).is_ok_and(|age| age > KEEP));
        if old {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
