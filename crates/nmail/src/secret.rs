//! Getting an account's password: from its `password_command`, or from the OS keyring.
//!
//! The password is never logged, echoed, stored or put in an error message. Error messages do not
//! quote the command either, in case someone inlined a secret in it.

use std::fmt;
use std::io::{self, BufRead, IsTerminal};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::{Account, PasswordSource};
use crate::keyring::Keyring;

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

/// The account's password, from its command or the keyring. `keyring` is opened on first use
/// and kept for the next account; an account with a command never opens it. Blocking: call it
/// outside the async runtime (see `Keyring::open`). Errors leave out the account name, which
/// `nmail sync` puts in front.
pub fn account_password(account: &Account, keyring: &mut Option<Keyring>) -> Result<Password> {
    match &account.password {
        PasswordSource::Command(command) => run_password_command(command),
        PasswordSource::Keyring => {
            let keyring = match keyring {
                Some(k) => k,
                None => keyring.insert(Keyring::open()?),
            };
            keyring.get(&account.name)
        }
    }
}

/// A new password for `account`: prompted for without echo on a terminal, otherwise one line of
/// standard input, so `pass show mail/work | nmail account set-password work` works.
pub fn read_new_password(account: &str) -> Result<Password> {
    let stdin = io::stdin();
    let password = if stdin.is_terminal() {
        rpassword::prompt_password(format!("Password for {account}: "))
            .context("cannot read the password")?
    } else {
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .context("cannot read the password from standard input")?;
        trim_newline(line)
    };
    if password.is_empty() {
        bail!("the password is empty; nothing was stored");
    }
    Ok(Password(password))
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
fn run_password_command(command: &str) -> Result<Password> {
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
}
