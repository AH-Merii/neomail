//! Getting a password from the account's `password_command`.
//!
//! The password is never logged, echoed, stored or put in an error message. Error messages do not
//! quote the command either, in case someone inlined a secret in it.

use std::fmt;
use std::process::{Command, Stdio};

use anyhow::{Result, bail};

/// A password. `Debug` is redacted so it cannot leak through `{:?}`.
pub struct Password(String);

impl Password {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// Runs `command` through `sh -c` and returns its standard output with one trailing newline
/// removed. Standard input and standard error stay attached, so a command can prompt (gpg, pass).
pub fn run_password_command(account: &str, command: &str) -> Result<Password> {
    let output = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(e) => bail!("account {account:?}: could not run password_command with sh: {e}"),
    };
    if !output.status.success() {
        bail!(
            "account {account:?}: password_command failed ({})",
            output.status
        );
    }
    let Ok(mut password) = String::from_utf8(output.stdout) else {
        bail!("account {account:?}: password_command printed something that is not UTF-8");
    };
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    if password.is_empty() {
        bail!("account {account:?}: password_command printed nothing");
    }
    Ok(Password(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_one_trailing_newline() {
        let p = run_password_command("a", "printf 'hunter2\\n\\n'").unwrap();
        assert_eq!(p.expose(), "hunter2\n");
        let p = run_password_command("a", "printf ' spaced '").unwrap();
        assert_eq!(p.expose(), " spaced ");
        assert_eq!(format!("{p:?}"), "Password(<redacted>)");
    }

    #[test]
    fn failures_do_not_leak_the_output() {
        let e = run_password_command("a", "echo hunter2; exit 3").unwrap_err();
        assert!(!format!("{e:#}").contains("hunter2"), "{e:#}");
        assert!(run_password_command("a", "true").is_err());
        assert!(run_password_command("a", "printf '\\377'").is_err());
    }
}
