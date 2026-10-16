//! Hands the user a script to run and picks up what it hands back, so the user never copies it
//! (DEC-2026-10-16-commands-route). A [`Kind::Run`](super::Kind::Run) step gets a directory of
//! its own under neomail's data directory, readable by the user only, made when the step shows
//! and removed with all it holds when the step ends. The step writes its script there as
//! [`SCRIPT`] and shows [`Inbox::paste`], one short line that runs it. The script writes
//! [`RESULT`] there (to a temporary name first, then renamed, so a half written file is never
//! read), and the screen polls for it while it waits for keys.
//!
//! The file is lines: `ok` and the value, or `failed`, the stage that failed and what the
//! command said, as many lines as it takes.

use std::io;
use std::path::{Path, PathBuf};

/// The file a command writes its result to, in the directory it was given.
pub const RESULT: &str = "result";

/// The script the step writes into its directory for the user to run.
pub const SCRIPT: &str = "setup.sh";

/// `text` quoted for sh: in single quotes, each `'` in it written as `'\''`.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What a command handed back.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The value the step asks for.
    Value(String),
    /// It failed at `stage`, a word the step's own [`Run::failure`](super::Run::failure)
    /// knows, having said `said`.
    Failed { stage: String, said: String },
}

/// Reads a result file's text, or `None` when it is not one.
pub fn parse(text: &str) -> Option<Outcome> {
    let mut lines = text.lines();
    match lines.next()?.trim() {
        "ok" => {
            let value = lines.next()?.trim();
            (!value.is_empty()).then(|| Outcome::Value(value.to_owned()))
        }
        "failed" => {
            let stage = lines.next().unwrap_or_default().trim().to_owned();
            let said: Vec<&str> = lines.map(str::trim_end).filter(|l| !l.is_empty()).collect();
            Some(Outcome::Failed {
                stage,
                said: said.join("\n"),
            })
        }
        _ => None,
    }
}

/// A directory a command writes its result to, removed when this drops.
#[derive(Debug)]
pub struct Inbox {
    dir: PathBuf,
}

impl Inbox {
    /// Makes a new directory under `data_dir`, readable by the user only.
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let parent = data_dir.join("runs");
        std::fs::create_dir_all(&parent)?;
        // Short, as it shows in the command: one step waits at a time in a process, and a
        // directory a crashed run of the same process ID left is replaced.
        let dir = parent.join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&dir);
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&dir)?;
        Ok(Self { dir })
    }

    /// The directory itself.
    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes `script`, with each `{result}` in it replaced by this directory quoted for sh,
    /// as [`SCRIPT`], readable and writable by the user only, and returns the
    /// line that runs it: `sh '/path/setup.sh'`, which fish, zsh, bash and sh all read the same.
    /// `None` when the path holds a character one of them would read inside single quotes (`'`
    /// or `\`), or a control character, or the file cannot be written: then nothing is written.
    pub fn paste(&self, script: &str) -> Option<String> {
        let path = self.dir.join(SCRIPT);
        let text = path.to_str()?;
        let unsafe_char = |c: char| c.is_control() || matches!(c, '\'' | '\\');
        if text.contains(unsafe_char) {
            return None;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let script = script.replace("{result}", &sh_quote(self.dir.to_str()?));
        let mut file = options.open(&path).ok()?;
        io::Write::write_all(&mut file, script.as_bytes()).ok()?;
        Some(format!("sh '{text}'"))
    }

    /// The result, once the command wrote it; the file is removed, so a second run of the
    /// command is picked up too.
    pub fn take(&self) -> Option<Outcome> {
        let file = self.dir.join(RESULT);
        let text = std::fs::read_to_string(&file).ok()?;
        let _ = std::fs::remove_file(&file);
        parse(&text)
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        // The parent goes too once no other run uses it.
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
