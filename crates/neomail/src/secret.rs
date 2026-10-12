//! Getting an account's password (from its `password_command` or the OS keyring) or OAuth
//! sign-in (from the OS keyring), and reading secrets from the terminal or standard input.
//!
//! The password is never logged, echoed, stored or put in an error message. Error messages do not
//! quote the command either, in case someone inlined a secret in it.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::PasswordSource;
use crate::keyring::Keyring;
use crate::oauth::{MailGrant, Secret};

/// A password. `Debug` is redacted so it cannot leak through `{:?}`.
pub struct Password(String);

impl Password {
    pub fn new(password: String) -> Self {
        Self(password)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// How a `password_command` runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandMode {
    /// Attached to the terminal, so it can prompt (gpg, pass), with no time limit: `neomail sync`.
    Interactive,
    /// Standard input closed, and stopped after this long: `neomail daemon`, where nobody answers
    /// a prompt and one stuck command must not stall every other account.
    Unattended(Duration),
}

/// The password of the account named `account`, from its command (run as `mode` says) or the
/// keyring. `keyring` is opened on first use and kept for the next account; an account with a
/// command never opens it. Blocking: call it outside the async runtime (see `Keyring::open`).
/// Errors leave out the account name, which `neomail sync` puts in front.
pub fn account_password(
    account: &str,
    source: &PasswordSource,
    keyring: &mut Option<Keyring>,
    mode: CommandMode,
) -> Result<Password> {
    match source {
        PasswordSource::Command(command) => match mode {
            CommandMode::Interactive => run_password_command(command),
            CommandMode::Unattended(limit) => run_password_command_within(command, limit),
        },
        PasswordSource::Keyring => {
            let keyring = match keyring {
                Some(k) => k,
                None => keyring.insert(Keyring::open()?),
            };
            keyring.get(account)
        }
    }
}

/// An OAuth account's saved sign-in, from its keyring entry.
pub struct SavedSignIn {
    /// `None` for a public client's entry.
    pub client_secret: Option<Secret>,
    pub refresh_token: Secret,
    pub grant: MailGrant,
}

/// An OAuth account's saved sign-in from the keyring, opened on first use like
/// [`account_password`]. Blocking, like it.
pub fn oauth_secrets(account: &str, keyring: &mut Option<Keyring>) -> Result<SavedSignIn> {
    let keyring = match keyring {
        Some(k) => k,
        None => keyring.insert(Keyring::open_for_oauth()?),
    };
    let not_signed_in = || {
        anyhow::anyhow!(
            "not signed in (no entry in {}); run `neomail account login {account}`",
            keyring.name()
        )
    };
    let entry = keyring.get_oauth(account)?.ok_or_else(not_signed_in)?;
    let refresh_token = entry.refresh_token().ok_or_else(not_signed_in)?;
    Ok(SavedSignIn {
        client_secret: entry.client_secret(),
        refresh_token,
        grant: entry.grant(),
    })
}

/// Writes a rotated refresh token back to the account's keyring entry, keeping its client
/// secret and grant. Blocking, like [`oauth_secrets`].
pub fn save_refresh_token(
    account: &str,
    keyring: &mut Option<Keyring>,
    saved: &SavedSignIn,
    refresh_token: &Secret,
) -> Result<()> {
    let keyring = match keyring {
        Some(k) => k,
        None => keyring.insert(Keyring::open_for_oauth()?),
    };
    let entry = crate::keyring::OAuthEntry {
        client_secret: saved.client_secret.as_ref().map(|s| s.expose().to_owned()),
        refresh_token: Some(refresh_token.expose().to_owned()),
        mail_grant: Some(saved.grant),
    };
    keyring.set_oauth(account, &entry)
}

/// Whether neomail may ask for something: standard input is a terminal, and neither
/// `--no-input` nor `NEOMAIL_NO_INPUT` forbids prompts.
#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub terminal: bool,
    pub no_input: bool,
}

impl Input {
    pub fn detect(no_input: bool) -> Self {
        Self {
            terminal: io::stdin().is_terminal(),
            no_input,
        }
    }

    pub fn can_prompt(self) -> bool {
        self.terminal && !self.no_input
    }

    /// Why nothing can be asked, for the usage error that names the missing flags.
    pub fn why_not(self) -> &'static str {
        if self.no_input {
            "--no-input (or NEOMAIL_NO_INPUT) forbids prompts"
        } else {
            "standard input is not a terminal, so nothing can be asked"
        }
    }
}

/// A usage error for values that are missing and cannot be asked for: `flags` lists what to pass.
pub fn cannot_ask(input: Input, flags: &str) -> anyhow::Error {
    anyhow::Error::new(crate::UsageError(format!(
        "{}: pass {flags}",
        input.why_not()
    )))
}

/// A new password for `account`: prompted for without echo on a terminal, otherwise one line of
/// standard input, so `pass show mail/work | neomail account set-password work` works.
pub fn read_new_password(account: &str, input: Input) -> Result<Password> {
    let password = read_secret(&format!("Password for {account}: "), "password", input)?;
    Ok(Password(password))
}

/// A secret, prompted for with `prompt` and without echo on a terminal, otherwise one line of
/// standard input. `what` names it in errors ("password", "client secret"). With `--no-input`
/// on a terminal it is a usage error: pipe the secret instead.
pub fn read_secret(prompt: &str, what: &str, input: Input) -> Result<String> {
    let stdin = io::stdin();
    if !input.terminal {
        return read_secret_line(stdin.lock(), what);
    }
    if input.no_input {
        return Err(anyhow::Error::new(crate::UsageError(format!(
            "{}: pipe the {what} on standard input instead",
            input.why_not()
        ))));
    }
    let secret =
        rpassword::prompt_password(prompt).with_context(|| format!("cannot read the {what}"))?;
    non_empty(secret, what)
}

/// A secret as one line of `input`, without its line ending.
fn read_secret_line(mut input: impl BufRead, what: &str) -> Result<String> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .with_context(|| format!("cannot read the {what} from standard input"))?;
    non_empty(trim_newline(line), what)
}

fn non_empty(secret: String, what: &str) -> Result<String> {
    if secret.is_empty() {
        bail!("the {what} is empty; nothing was stored");
    }
    Ok(secret)
}

/// One line typed on the terminal (echoed), after `prompt` on standard error, trimmed.
pub fn prompt_line(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        bail!("standard input closed");
    }
    Ok(line.trim().to_owned())
}

/// Removes one trailing `\n` or `\r\n`.
fn trim_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

/// Runs `command` through `sh -c` and returns its standard output with one trailing newline
/// removed. Standard input and standard error stay attached, so a command can prompt (gpg, pass).
pub fn run_password_command(command: &str) -> Result<Password> {
    let output = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(e) => bail!("could not run password_command with sh: {e}"),
    };
    password_from(output.status, output.stdout)
}

/// [`run_password_command`] for nobody at the terminal: standard input is closed, so a command
/// that would prompt fails instead, and a command still running after `limit` is killed (the
/// `sh` neomail started, nothing else) and reported as an error.
///
/// A process the command started in the background that keeps standard output open is left
/// running; so is the thread reading that output, until it closes.
pub fn run_password_command_within(command: &str, limit: Duration) -> Result<Password> {
    let deadline = Instant::now() + limit;
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => bail!("could not run password_command with sh: {e}"),
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        bail!("could not read password_command's output");
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.read_to_end(&mut bytes).map(|_| bytes);
        // The receiver is gone once the command timed out; nobody needs the output then.
        let _ = sender.send(read);
    });
    let timed_out = |mut child: std::process::Child| {
        // Kills only the child spawned above, never a pid; `wait` reaps it.
        let _ = child.kill();
        let _ = child.wait();
        anyhow::anyhow!(
            "password_command did not finish within {} s",
            limit.as_secs_f64()
        )
    };
    let stdout = match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("could not read password_command's output: {e}");
        }
        Err(_) => return Err(timed_out(child)),
    };
    // Standard output is closed; the command has almost always exited by now.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => return Err(timed_out(child)),
            Err(e) => bail!("could not wait for password_command: {e}"),
        }
    };
    password_from(status, stdout)
}

/// The password a finished `password_command` printed.
fn password_from(status: ExitStatus, stdout: Vec<u8>) -> Result<Password> {
    if !status.success() {
        bail!("password_command failed ({status})");
    }
    let Ok(password) = String::from_utf8(stdout) else {
        bail!("password_command printed something that is not UTF-8");
    };
    let password = trim_newline(password);
    if password.is_empty() {
        bail!("password_command printed nothing");
    }
    Ok(Password(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_one_trailing_newline() {
        let p = run_password_command("printf 'hunter2\\n\\n'").unwrap();
        assert_eq!(p.expose(), "hunter2\n");
        let p = run_password_command("printf ' spaced '").unwrap();
        assert_eq!(p.expose(), " spaced ");
        assert_eq!(format!("{p:?}"), "Password(<redacted>)");
    }

    #[test]
    fn failures_do_not_leak_the_output() {
        let e = run_password_command("echo hunter2; exit 3").unwrap_err();
        assert!(!format!("{e:#}").contains("hunter2"), "{e:#}");
        assert!(run_password_command("true").is_err());
        assert_eq!(trim_newline("a\r\n".into()), "a");
        assert_eq!(trim_newline("a\n\n".into()), "a\n");
        assert_eq!(trim_newline("a\r".into()), "a\r");
        assert!(run_password_command("printf '\\377'").is_err());
    }

    #[test]
    fn an_unattended_command_gets_the_same_password() {
        let p =
            run_password_command_within("printf 'hunter2\\n'", Duration::from_secs(60)).unwrap();
        assert_eq!(p.expose(), "hunter2");
        let e = run_password_command_within("echo hunter2; exit 3", Duration::from_secs(60))
            .unwrap_err();
        assert_eq!(format!("{e:#}"), "password_command failed (exit status: 3)");
    }

    #[test]
    fn an_unattended_command_cannot_wait_for_input() {
        // Standard input is closed, so a prompt reads end of file instead of waiting.
        let e = run_password_command_within(
            "read -r line; printf %s \"$line\"",
            Duration::from_secs(60),
        )
        .unwrap_err();
        assert_eq!(format!("{e:#}"), "password_command printed nothing");
    }

    #[test]
    fn an_unattended_command_that_never_finishes_is_stopped() {
        let limit = Duration::from_millis(50);
        let started = Instant::now();
        // Still printing: standard output stays open.
        let e = run_password_command_within("exec sleep 3600", limit).unwrap_err();
        assert_eq!(
            format!("{e:#}"),
            "password_command did not finish within 0.05 s"
        );
        // Done printing, but never exits.
        let e = run_password_command_within("printf hunter2; exec >&-; exec sleep 3600", limit)
            .unwrap_err();
        assert!(!format!("{e:#}").contains("hunter2"), "{e:#}");
        assert_eq!(
            format!("{e:#}"),
            "password_command did not finish within 0.05 s"
        );
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn a_secret_line_is_one_line_and_never_empty() {
        for input in ["", "\n", "\r\n"] {
            let e = read_secret_line(input.as_bytes(), "password").unwrap_err();
            assert_eq!(
                format!("{e:#}"),
                "the password is empty; nothing was stored"
            );
        }
        let secret = read_secret_line("hunter2 \r\nnext\n".as_bytes(), "password").unwrap();
        assert_eq!(secret, "hunter2 ");
    }

    #[test]
    fn no_input_on_a_terminal_refuses_the_prompt_as_a_usage_error() {
        let input = Input {
            terminal: true,
            no_input: true,
        };
        assert!(!input.can_prompt());
        let e = read_secret("Password: ", "password", input).unwrap_err();
        assert!(e.is::<crate::UsageError>(), "{e:#}");
        assert_eq!(
            format!("{e:#}"),
            "--no-input (or NEOMAIL_NO_INPUT) forbids prompts: pipe the password on standard \
             input instead"
        );
    }
}
