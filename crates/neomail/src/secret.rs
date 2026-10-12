//! Getting an account's password (from its `password_command` or the OS keyring) or OAuth
//! sign-in (from the OS keyring), and reading secrets from the terminal or standard input.
//!
//! The password is never logged, echoed, stored or put in an error message. Error messages do not
//! quote the command either, in case someone inlined a secret in it.

use std::fmt;
use std::io::{self, BufRead, IsTerminal};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::PasswordSource;
use crate::keyring::Keyring;
use crate::oauth::Secret;

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

/// The password of the account named `account`, from its command or the keyring. `keyring` is
/// opened on first use and kept for the next account; an account with a command never opens it.
/// Blocking: call it outside the async runtime (see `Keyring::open`). Errors leave out the
/// account name, which `neomail sync` puts in front.
pub fn account_password(
    account: &str,
    source: &PasswordSource,
    keyring: &mut Option<Keyring>,
) -> Result<Password> {
    match source {
        PasswordSource::Command(command) => run_password_command(command),
        PasswordSource::Keyring => {
            let keyring = match keyring {
                Some(k) => k,
                None => keyring.insert(Keyring::open()?),
            };
            keyring.get(account)
        }
    }
}

/// An OAuth account's client secret (`None` for a public client's entry) and refresh token from
/// the keyring, opened on first use like [`account_password`]. Blocking, like it.
pub fn oauth_secrets(
    account: &str,
    keyring: &mut Option<Keyring>,
) -> Result<(Option<Secret>, Secret)> {
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
    Ok((entry.client_secret(), refresh_token))
}

/// Writes a rotated refresh token back to the account's keyring entry, keeping its client
/// secret. Blocking, like [`oauth_secrets`].
pub fn save_refresh_token(
    account: &str,
    keyring: &mut Option<Keyring>,
    client_secret: Option<&Secret>,
    refresh_token: &Secret,
) -> Result<()> {
    let keyring = match keyring {
        Some(k) => k,
        None => keyring.insert(Keyring::open_for_oauth()?),
    };
    let entry = crate::keyring::OAuthEntry {
        client_secret: client_secret.map(|s| s.expose().to_owned()),
        refresh_token: Some(refresh_token.expose().to_owned()),
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
    if !output.status.success() {
        bail!("password_command failed ({})", output.status);
    }
    let Ok(password) = String::from_utf8(output.stdout) else {
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
