//! Advisory locks in the data directory, with `flock(2)`: one daemon per data directory
//! (`daemon.lock`), and one sync per account (`locks/ACCOUNT.lock`), shared by `neomail sync`
//! and `neomail daemon`.
//!
//! A lock belongs to its open file and dies with the process, so a crash leaves nothing to clean
//! up, and a lock file is never deleted: deleting it while it is held would let a second process
//! lock a new file of the same name. Two opens of the same file conflict even within one process,
//! which is what the tests rely on.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A held lock, released when dropped (the file is closed).
#[derive(Debug)]
pub struct Held {
    _file: File,
}

/// The daemon's lock, held for its whole life. A second daemon for the same data directory is
/// refused.
pub fn daemon(data_dir: &Path) -> Result<Held> {
    let path = data_dir.join("daemon.lock");
    match try_lock(&path)? {
        Some(held) => Ok(held),
        None => bail!(
            "neomail daemon is already running for {}",
            data_dir.display()
        ),
    }
}

/// The account's sync lock, or `None` when someone else is syncing it: the daemon skips the
/// account for this round.
pub fn try_account(data_dir: &Path, account: &str) -> Result<Option<Held>> {
    try_lock(&account_path(data_dir, account)?)
}

/// The account's sync lock, waiting for it if it is held; `waiting` runs once, before the wait.
pub fn wait_for_account(data_dir: &Path, account: &str, waiting: impl FnOnce()) -> Result<Held> {
    let path = account_path(data_dir, account)?;
    let file = open(&path)?;
    if lock(&file, false).with_context(|| format!("cannot lock {}", path.display()))? {
        return Ok(Held { _file: file });
    }
    waiting();
    lock(&file, true).with_context(|| format!("cannot lock {}", path.display()))?;
    Ok(Held { _file: file })
}

/// `DATA_DIR/locks/NAME.lock`, creating `locks` (mode 0700) if needed.
fn account_path(data_dir: &Path, account: &str) -> Result<PathBuf> {
    let dir = data_dir.join("locks");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir.join(format!("{}.lock", file_name(account))))
}

/// An account name as a file name: letters, digits, `_`, `-` and `.` stay, every other byte
/// becomes `%XX`, and so does a leading `.`, so no name can leave the directory or hide.
fn file_name(account: &str) -> String {
    let mut name = String::with_capacity(account.len());
    for (i, b) in account.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (b == b'.' && i > 0) {
            name.push(char::from(b));
        } else {
            name.push_str(&format!("%{b:02X}"));
        }
    }
    name
}

fn try_lock(path: &Path) -> Result<Option<Held>> {
    let file = open(path)?;
    if lock(&file, false).with_context(|| format!("cannot lock {}", path.display()))? {
        Ok(Some(Held { _file: file }))
    } else {
        Ok(None)
    }
}

/// Opens (creating, mode 0600) a lock file without truncating it.
fn open(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))
}

/// An exclusive lock on `file`: `Ok(false)` when it is held elsewhere and `wait` is false.
#[cfg(unix)]
fn lock(file: &File, wait: bool) -> io::Result<bool> {
    use rustix::fs::{FlockOperation, flock};
    let operation = if wait {
        FlockOperation::LockExclusive
    } else {
        FlockOperation::NonBlockingLockExclusive
    };
    loop {
        match flock(file, operation) {
            Ok(()) => return Ok(true),
            Err(rustix::io::Errno::INTR) => continue,
            Err(rustix::io::Errno::WOULDBLOCK) if !wait => return Ok(false),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Elsewhere there is no lock: neomail is built for Linux and macOS.
#[cfg(not(unix))]
fn lock(_file: &File, _wait: bool) -> io::Result<bool> {
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_second_daemon_is_refused_until_the_first_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let first = daemon(dir.path()).unwrap();
        let e = daemon(dir.path()).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!(
                "neomail daemon is already running for {}",
                dir.path().display()
            )
        );
        drop(first);
        daemon(dir.path()).unwrap();
        assert!(dir.path().join("daemon.lock").exists(), "never deleted");
    }

    #[test]
    fn a_held_account_is_skipped_and_others_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let held = try_account(dir.path(), "work").unwrap().unwrap();
        assert!(try_account(dir.path(), "work").unwrap().is_none());
        assert!(try_account(dir.path(), "home").unwrap().is_some());
        drop(held);
        assert!(try_account(dir.path(), "work").unwrap().is_some());
    }

    #[test]
    fn sync_says_it_waits_once_then_gets_the_lock_when_it_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let held = try_account(dir.path(), "work").unwrap().unwrap();
        let (said, waiting) = mpsc::channel();
        let path = dir.path().to_owned();
        let waiter = std::thread::spawn(move || {
            wait_for_account(&path, "work", || said.send(()).unwrap()).unwrap()
        });
        // The waiter says so before it blocks; nothing is released until it has.
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!waiter.is_finished());
        drop(held);
        let got = waiter.join().unwrap();
        assert!(
            try_account(dir.path(), "work").unwrap().is_none(),
            "the waiter holds it"
        );
        drop(got);
        // Free: no message.
        let mut said = false;
        wait_for_account(dir.path(), "work", || said = true).unwrap();
        assert!(!said);
    }

    #[test]
    fn account_names_cannot_leave_the_lock_directory() {
        assert_eq!(file_name("work"), "work");
        assert_eq!(file_name("me.work-2_b"), "me.work-2_b");
        assert_eq!(file_name("../x"), "%2E.%2Fx");
        assert_eq!(file_name(".."), "%2E.");
        assert_eq!(file_name("a b/é"), "a%20b%2F%C3%A9");
    }
}
