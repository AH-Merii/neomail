//! The pager for `show`'s human output on a terminal: `$NEOMAIL_PAGER`, else `$PAGER`, else
//! `less`, run through `sh -c`. LESS=FRX is set for less when LESS is unset, as git does.

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
/// Ctrl-C reaches neomail and the pager together, as one foreground process group. less catches
/// it (it only aborts a search there), so neomail holds it off until the pager exits, as git
/// does: dying first would hand the terminal back to the shell while less still reads it. A
/// pager that dies of Ctrl-C takes neomail with it.
pub fn page<W: Write + ?Sized>(pager: &Pager, text: &[u8], out: &mut W) -> io::Result<()> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(&pager.command).stdin(Stdio::piped());
    if pager.set_less {
        command.env("LESS", "FRX");
    }
    // Registered before the spawn; the pager starts with the default action again, since exec
    // resets a caught signal.
    #[cfg(unix)]
    let interrupts = HeldInterrupts::hold();
    let Ok(mut child) = command.spawn() else {
        return out.write_all(text);
    };
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

#[cfg(unix)]
use interrupts::HeldInterrupts;

#[cfg(unix)]
mod interrupts {
    use signal_hook::consts::SIGINT;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// SIGINT does nothing while this lives. Dropping it brings back the default action
    /// (termination) for the rest of the process.
    pub struct HeldInterrupts {
        released: Arc<AtomicBool>,
    }

    impl HeldInterrupts {
        pub fn hold() -> Self {
            let released = Arc::new(AtomicBool::new(false));
            // If the handler cannot be installed, Ctrl-C keeps its default action: neomail
            // dies as it did before, which is no worse.
            let _ = signal_hook::flag::register_conditional_default(SIGINT, Arc::clone(&released));
            Self { released }
        }
    }

    impl Drop for HeldInterrupts {
        fn drop(&mut self) {
            self.released.store(true, Ordering::SeqCst);
        }
    }

    /// Dies of SIGINT as the pager did, so the shell sees an interrupted command (exit 130).
    pub fn follow_the_pager(status: ExitStatus) {
        if status.signal() == Some(SIGINT) {
            let _ = signal_hook::low_level::emulate_default_handler(SIGINT);
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

    #[test]
    fn a_pager_that_cannot_start_prints_directly() {
        let missing = Pager {
            command: "neomail-no-such-pager-here".to_owned(),
            set_less: false,
        };
        let mut out = Vec::new();
        page(&missing, b"hello\n", &mut out).unwrap();
        assert_eq!(out, b"hello\n");
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
