//! A second run remembers: the answers of an unfinished setup that are no secret and came from
//! no flag, kept in `wizard-ID.json` in the data directory (mode 0600) and offered again. The
//! file goes once the account is saved. Secrets never reach it: [`Session::typed`] holds none.
//! Only an interactive run reads or writes it.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Session;

/// The answers kept for one wizard, by step id.
#[derive(Debug, Default)]
pub struct Memory {
    path: Option<PathBuf>,
    pub answers: BTreeMap<String, String>,
}

impl Memory {
    /// Nothing kept and nothing to keep: off a terminal, with `--no-input`, and in tests that
    /// do not ask for it.
    pub fn none() -> Self {
        Self::default()
    }

    /// What `dir` holds for wizard `id`. A file that cannot be read or parsed is ignored: it only
    /// saves typing.
    pub fn load(dir: &Path, id: &str) -> Self {
        let path = dir.join(format!("wizard-{id}.json"));
        let answers = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            answers,
        }
    }

    pub fn get(&self, id: &str) -> Option<&str> {
        self.answers.get(id).map(String::as_str)
    }

    /// Keeps what `session` was typed so far, with what was kept before for the steps not
    /// reached this time.
    pub fn keep(&mut self, session: &Session) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        for (id, value) in session.typed() {
            self.answers.insert(id.to_owned(), value.to_owned());
        }
        if self.answers.is_empty() {
            return Ok(());
        }
        let dir = path.parent().context("no data directory")?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let text = serde_json::to_string_pretty(&self.answers)?;
        let tmp = path.with_extension("json.tmp");
        // A leftover from a crash keeps its own mode when opened: start from none.
        let _ = std::fs::remove_file(&tmp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        file.write_all(text.as_bytes())?;
        std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
    }

    /// The account is saved: nothing is left to offer.
    pub fn forget(&mut self) {
        self.answers.clear();
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}
