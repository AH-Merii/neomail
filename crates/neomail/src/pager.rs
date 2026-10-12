//! The pager for `show`'s human output on a terminal: `$NEOMAIL_PAGER`, else `$PAGER`, else
//! `less`, run through `sh -c`. LESS=FRX is set for less when LESS is unset, as git does.

use std::borrow::Cow;
use std::io::{self, Write};
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq, Eq)]
pub struct Pager {
    /// Run through `sh -c`.
    pub command: String,
    /// Set LESS=FRX: the pager is less and LESS is unset.
    pub set_less: bool,
}

/// The pager to use, or `None` to print directly: with `--no-pager`, off a terminal, or when the
/// first pager variable that is set is empty or "cat". `var` reads the environment.
pub fn choose(
    no_pager: bool,
    terminal: bool,
    var: impl Fn(&str) -> Option<String>,
) -> Option<Pager> {
    if no_pager || !terminal {
        return None;
    }
    let command = var("NEOMAIL_PAGER")
        .or_else(|| var("PAGER"))
        .unwrap_or_else(|| "less".to_owned());
    let command = command.trim();
    if command.is_empty() || command == "cat" {
        return None;
    }
    let program = command.split_whitespace().next().unwrap_or("");
    let is_less = program.rsplit('/').next() == Some("less");
    Some(Pager {
        command: command.to_owned(),
        set_less: is_less && var("LESS").is_none(),
    })
}

/// Shows `text` through the pager. If the pager cannot start (it fails to spawn, or the shell
/// reports exit 126 or 127, "cannot run" or "not found"), `text` goes to `out` instead.
/// Quitting the pager before the end is not an error.
///
/// Ctrl-C and Ctrl-\ reach neomail and the pager together, as one foreground process group. less
/// catches the one (it only aborts a search there) and ignores the other, so neomail holds both
/// off until the pager exits, as git does: dying first would hand the terminal back to the shell
/// while less still reads it. A pager that dies of Ctrl-C takes neomail with it. A signal that
/// was ignored when neomail started stays ignored, in neomail and in the pager.
pub fn page<W: Write + ?Sized>(pager: &Pager, text: &[u8], out: &mut W) -> io::Result<()> {
    page_with("sh", pager, text, out)
}

/// [`page`] through `shell`, which the tests can make one that does not exist.
fn page_with<W: Write + ?Sized>(
    shell: &str,
    pager: &Pager,
    text: &[u8],
    out: &mut W,
) -> io::Result<()> {
    let mut command = Command::new(shell);
    command
        .arg("-c")
        .arg(&*shell_command(&pager.command))
        .stdin(Stdio::piped());
    if pager.set_less {
        command.env("LESS", "FRX");
    }
    let Ok(mut child) = command.spawn() else {
        return out.write_all(text);
    };
    // Held only once the pager runs, so a pager that cannot start leaves Ctrl-C as it was. The
    // pager starts with neomail's own dispositions (the default, or ignored). A Ctrl-C in the
    // instant between the spawn and the hold still ends neomail, as it did before paging.
    #[cfg(unix)]
    let interrupts = HeldInterrupts::hold();
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(text),
        None => Ok(()),
    };
    let status = child.wait()?;
    #[cfg(unix)]
    {
        drop(interrupts);
        interrupts::follow_the_pager(status);
    }
    if matches!(status.code(), Some(126 | 127)) {
        return out.write_all(text);
    }
    match written {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(e),
        _ => Ok(()),
    }
}

/// What `sh -c` runs for the pager `command`: `exec command` when it is one simple command whose
/// program is a file (a path, or found in PATH), so the shell replaces itself with the pager.
///
/// bash does that on its own for the last command of `-c`; dash, Debian's and Ubuntu's sh, does
/// not. It stays between neomail and the pager, and as a non-interactive shell it leaves
/// Ctrl-\ at its default action: it would die at once, neomail's wait would end, and neomail
/// would exit while the pager still reads the terminal. Anything else (a pipeline, a list, an
/// assignment, a quoted or unusual program name, a shell keyword or builtin) runs as written, so
/// a shell may stay in between there, and with dash Ctrl-\ still ends neomail early.
fn shell_command(command: &str) -> Cow<'_, str> {
    if is_one_program(command) {
        Cow::Owned(format!("exec {command}"))
    } else {
        Cow::Borrowed(command)
    }
}

/// Whether `command` is one simple command that `exec` can run as it is: no unquoted control
/// operator, and a first word of plain characters naming a file.
fn is_one_program(command: &str) -> bool {
    let Some(program) = command.split_whitespace().next() else {
        return false;
    };
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./+-,:@".contains(c);
    if !program.chars().all(plain) {
        return false;
    }
    let is_file = if program.contains('/') {
        std::path::Path::new(program).is_file()
    } else {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
    };
    if !is_file {
        return false;
    }
    let mut quote = None;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some(_), '"') => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|' | '&' | ';' | '(' | ')' | '`' | '\n') => return false,
            (None, _) => {}
        }
    }
    quote.is_none() && !escaped
}

#[cfg(unix)]
use interrupts::HeldInterrupts;

#[cfg(unix)]
mod interrupts {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    use signal_hook::consts::{SIGINT, SIGQUIT};

    /// The signals the terminal sends from the keyboard: Ctrl-C and Ctrl-\.
    const KEYBOARD: [i32; 2] = [SIGINT, SIGQUIT];

    /// While one lives, Ctrl-C and Ctrl-\ do nothing to neomail. When the last is dropped they
    /// take their default action (termination) again. Holds may follow each other or overlap.
    pub struct HeldInterrupts(());

    /// The process's one registration per signal, made at the first hold. signal-hook cannot
    /// take a registration back cleanly (after `unregister` the signal is ignored), so later holds
    /// reuse it and only flip `released`.
    struct State {
        /// What each registered signal does: its default action when true, nothing when false.
        released: Arc<AtomicBool>,
        holders: Mutex<usize>,
        /// The keyboard signals that were ignored when neomail started: never registered, so
        /// they stay ignored, and the pager inherits that.
        ignored: Vec<i32>,
    }

    fn state() -> &'static State {
        static STATE: OnceLock<State> = OnceLock::new();
        STATE.get_or_init(|| {
            // Read before the registration, which replaces the disposition. Nothing in neomail
            // touches these signals before the first hold, so this is how neomail started (an
            // asynchronous job without job control starts with both ignored).
            let ignored: Vec<i32> = KEYBOARD.into_iter().filter(|&s| ignored_now(s)).collect();
            let released = Arc::new(AtomicBool::new(true));
            for signal in KEYBOARD.into_iter().filter(|s| !ignored.contains(s)) {
                // If it cannot be registered, the signal keeps its default action: neomail dies
                // as it did before, which is no worse.
                let _ =
                    signal_hook::flag::register_conditional_default(signal, Arc::clone(&released));
            }
            State {
                released,
                holders: Mutex::new(0),
                ignored,
            }
        })
    }

    /// Whether `signal` is ignored now, from the SigIgn mask in /proc/self/status (std and
    /// signal-hook have no safe way to ask, and this crate has no `unsafe`).
    #[cfg(target_os = "linux")]
    fn ignored_now(signal: i32) -> bool {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return false;
        };
        status
            .lines()
            .find_map(|line| line.strip_prefix("SigIgn:"))
            .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
            .is_some_and(|mask| mask & (1 << (signal - 1)) != 0)
    }

    /// Elsewhere there is no safe way to ask: assume the default, as before this check.
    #[cfg(not(target_os = "linux"))]
    fn ignored_now(_signal: i32) -> bool {
        false
    }

    #[cfg(test)]
    thread_local! {
        /// The holds alive on this thread, for the tests.
        pub static ALIVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    impl HeldInterrupts {
        pub fn hold() -> Self {
            #[cfg(test)]
            ALIVE.with(|n| n.set(n.get() + 1));
            let state = state();
            let mut holders = state.holders.lock().unwrap_or_else(PoisonError::into_inner);
            *holders += 1;
            state.released.store(false, Ordering::SeqCst);
            Self(())
        }
    }

    impl Drop for HeldInterrupts {
        fn drop(&mut self) {
            #[cfg(test)]
            ALIVE.with(|n| n.set(n.get() - 1));
            let state = state();
            let mut holders = state.holders.lock().unwrap_or_else(PoisonError::into_inner);
            *holders = holders.saturating_sub(1);
            if *holders == 0 {
                state.released.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Dies of SIGINT as the pager did, so the shell sees an interrupted command (exit 130);
    /// not when SIGINT was ignored at start.
    pub fn follow_the_pager(status: ExitStatus) {
        if status.signal() == Some(SIGINT) && !state().ignored.contains(&SIGINT) {
            let _ = signal_hook::low_level::emulate_default_handler(SIGINT);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_second_hold_holds_too() {
            // Set on each delivery, so the test knows the signal arrived and was held.
            let seen = Arc::new(AtomicBool::new(false));
            for signal in KEYBOARD {
                signal_hook::flag::register(signal, Arc::clone(&seen)).unwrap();
            }
            for round in 0..3 {
                let held = HeldInterrupts::hold();
                for signal in KEYBOARD {
                    seen.store(false, Ordering::SeqCst);
                    // Kills this test process if the hold does not hold.
                    signal_hook::low_level::raise(signal).unwrap();
                    assert!(seen.load(Ordering::SeqCst), "round {round}: {signal}");
                }
                drop(held);
            }
        }

        /// Set in the copy of this test binary that the next test starts with the keyboard
        /// signals ignored.
        const IGNORED_CHILD: &str = "NEOMAIL_TEST_STARTED_WITH_KEYS_IGNORED";

        #[cfg(target_os = "linux")]
        #[test]
        fn signals_ignored_at_start_stay_ignored_after_a_hold() {
            if std::env::var_os(IGNORED_CHILD).is_some() {
                drop(HeldInterrupts::hold());
                for signal in KEYBOARD {
                    // Kills this copy if the hold left the signal at its default action.
                    signal_hook::low_level::raise(signal).unwrap();
                }
                return;
            }
            let status = std::process::Command::new("sh")
                .args([
                    "-c",
                    "trap '' INT QUIT; exec \"$0\" --exact \"$1\" --test-threads 1",
                ])
                .arg(std::env::current_exe().unwrap())
                .arg("pager::interrupts::tests::signals_ignored_at_start_stay_ignored_after_a_hold")
                .env(IGNORED_CHILD, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&status.stdout);
            assert!(status.status.success(), "{:?}\n{stdout}", status.status);
            assert!(stdout.contains("1 passed"), "{stdout}");
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn the_ignored_mask_is_read() {
            // SIGPIPE is ignored by the Rust runtime at start; SIGTERM is left at the default.
            assert!(ignored_now(signal_hook::consts::SIGPIPE));
            assert!(!ignored_now(signal_hook::consts::SIGTERM));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn choosing_the_pager() {
        let less = Some(Pager {
            command: "less".to_owned(),
            set_less: true,
        });
        assert_eq!(choose(false, true, env(&[])), less);
        // Never off a terminal, nor with --no-pager.
        assert_eq!(choose(false, false, env(&[])), None);
        assert_eq!(choose(true, true, env(&[("PAGER", "more")])), None);
        // NEOMAIL_PAGER, then PAGER.
        let more = |c: &str| {
            Some(Pager {
                command: c.to_owned(),
                set_less: false,
            })
        };
        assert_eq!(choose(false, true, env(&[("PAGER", "more")])), more("more"));
        assert_eq!(
            choose(
                false,
                true,
                env(&[("PAGER", "less"), ("NEOMAIL_PAGER", "most -s")])
            ),
            more("most -s")
        );
        // Empty or cat turn paging off, even when PAGER is set.
        assert_eq!(
            choose(
                false,
                true,
                env(&[("NEOMAIL_PAGER", ""), ("PAGER", "less")])
            ),
            None
        );
        assert_eq!(choose(false, true, env(&[("PAGER", "cat")])), None);
        assert_eq!(choose(false, true, env(&[("PAGER", " ")])), None);
        // LESS=FRX only for less, and only when LESS is unset.
        assert_eq!(
            choose(false, true, env(&[("PAGER", "/usr/bin/less -S")])),
            Some(Pager {
                command: "/usr/bin/less -S".to_owned(),
                set_less: true
            })
        );
        assert_eq!(
            choose(false, true, env(&[("LESS", "R")])),
            Some(Pager {
                command: "less".to_owned(),
                set_less: false
            })
        );
    }

    /// Collects what is written, and fails if Ctrl-C is held while it is.
    #[derive(Default)]
    struct Unheld(Vec<u8>);

    impl Write for Unheld {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            #[cfg(unix)]
            assert_eq!(interrupts::ALIVE.with(std::cell::Cell::get), 0, "held");
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_pager_that_cannot_start_prints_directly_without_holding_ctrl_c() {
        let missing = Pager {
            command: "neomail-no-such-pager-here".to_owned(),
            set_less: false,
        };
        // The shell reports 127, or the shell itself cannot be spawned.
        for shell in ["sh", "neomail-no-such-shell-here"] {
            let mut out = Unheld::default();
            page_with(shell, &missing, b"hello\n", &mut out).unwrap();
            assert_eq!(out.0, b"hello\n", "{shell}");
        }
    }

    #[test]
    fn one_program_runs_with_exec_and_anything_else_as_written() {
        // sh is a file in PATH wherever these tests run.
        for command in [
            "sh",
            "sh -c 'cat | cat'",
            "/bin/sh -s",
            "sh \"a; b\" # note",
            "sh 2>/dev/null",
        ] {
            assert_eq!(
                shell_command(command),
                format!("exec {command}"),
                "{command}"
            );
        }
        for command in [
            "sh | cat",
            "sh; cat",
            "sh && cat",
            "sh &",
            "(sh)",
            "{ sh; }",
            "sh `true`",
            "sh $(true)",
            "sh\ncat",
            "LESS=R less",
            "'sh'",
            "sh 'unclosed",
            "neomail-no-such-pager-here",
            "./no/such/pager",
            "",
        ] {
            assert_eq!(shell_command(command), command, "{command}");
        }
    }

    /// No shell stays between neomail and its pager, whichever sh the system has (dash would
    /// otherwise stay, and die of Ctrl-\ while the pager runs).
    #[cfg(unix)]
    #[test]
    fn the_pager_is_neomails_own_child() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        let script = dir.path().join("pager.sh");
        let body = format!("cat >/dev/null\necho $PPID > '{}'\n", parent.display());
        std::fs::write(&script, body).unwrap();
        let pager = Pager {
            command: format!("sh '{}'", script.display()),
            set_less: false,
        };
        page(&pager, b"hello\n", &mut Vec::new()).unwrap();
        let parent = std::fs::read_to_string(&parent).unwrap();
        assert_eq!(parent.trim(), std::process::id().to_string());
    }

    #[test]
    fn a_pager_that_quits_early_is_not_an_error() {
        let quits = Pager {
            command: "true".to_owned(),
            set_less: false,
        };
        let mut out = Vec::new();
        page(&quits, &vec![b'x'; 1 << 20], &mut out).unwrap();
        assert!(out.is_empty());
    }
}
