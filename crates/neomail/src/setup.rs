//! `neomail account add imap`: any IMAP server, which gets no wizard (DEC-2026-10-13-wizards;
//! [`crate::wizard`] has Gmail and Proton Mail). A provider is data (a [`Preset`]); one flow
//! gathers what the flags left out, checks the login and writes the account with the shared
//! config writer. Nothing touching the network or the keyring is called directly: [`Deps`] holds
//! those steps, so the tests give in-memory ones.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use nm_imap::{CertPin, ImapConfig, Security};

use crate::config;
use crate::keyring::{self, Keyring};
use crate::secret::{self, Input, Password};

/// What a provider fixes and what it asks for.
#[derive(Debug)]
pub struct Preset {
    /// The provider's name on the command line.
    pub id: &'static str,
    /// The account name when none is given.
    pub default_name: Option<&'static str>,
    /// The fixed IMAP server; `None` asks for `--host`.
    pub host: Option<&'static str>,
    pub port: u16,
    pub security: Security,
    pub username_prompt: &'static str,
    pub password_prompt: &'static str,
}

pub const IMAP: Preset = Preset {
    id: "imap",
    default_name: None,
    host: None,
    port: 993,
    security: Security::Tls,
    username_prompt: "Username: ",
    password_prompt: "Password (not shown): ",
};

/// The flags `account add` takes for a password provider.
#[derive(Debug, Default)]
pub struct Args {
    pub name: Option<String>,
    pub username: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub security: Option<Security>,
    pub password_command: Option<String>,
    pub cert_sha256: Option<CertPin>,
    pub no_check: bool,
    /// imap: the SMTP server; Gmail's and Bridge's are known.
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    /// imap: how the SMTP connection is secured; by default STARTTLS on 587, else TLS.
    pub smtp_security: Option<Security>,
}

/// What `add` will do, once everything is known but nothing is written.
#[derive(Debug)]
pub struct Plan {
    pub name: String,
    pub imap: ImapConfig,
    pub password_command: Option<String>,
    pub check: bool,
    /// The SMTP server `--smtp-host` names; without it, the one [`config::preset_smtp`] knows
    /// for the IMAP server, if any.
    pub smtp: Option<config::SmtpSettings>,
}

/// The steps that reach outside the process. Plain function values, not a trait: one real set
/// in [`add`], one fake set per test.
pub struct Deps<'a> {
    pub open_keyring: &'a dyn Fn() -> Result<Keyring>,
    pub read_password: &'a dyn Fn() -> Result<Password>,
    /// Logs in read-only and returns how many messages INBOX holds.
    pub check_login: &'a dyn Fn(&ImapConfig, &Password) -> Result<u32>,
}

/// `neomail account add imap`.
pub fn add(
    preset: &'static Preset,
    config_path: &Path,
    args: Args,
    input: Input,
    out: &mut impl Write,
) -> Result<()> {
    let plan = gather(preset, config_path, args, input)?;
    let prompt = preset.password_prompt;
    let deps = Deps {
        open_keyring: &Keyring::open,
        read_password: &|| {
            Ok(Password::new(secret::read_secret(
                prompt, "password", input,
            )?))
        },
        check_login: &|imap, password| {
            let login = nm_imap::Login::Password(password.expose());
            Ok(crate::account::runtime()?.block_on(nm_imap::check_login(imap, login))?)
        },
    };
    execute(plan, config_path, &deps, out)
}

/// Asks for what the flags left out, or fails with a usage error naming the flags when nothing
/// can be asked. Fails early when the name is taken.
pub fn gather(
    preset: &'static Preset,
    config_path: &Path,
    args: Args,
    input: Input,
) -> Result<Plan> {
    let ask = input.can_prompt();
    let mut missing = Vec::new();
    let mut value =
        |given: Option<String>, flag: &'static str, prompt: &str, default: Option<&str>| {
            let given = given.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
            match (given, default) {
                (Some(v), _) => Ok(v),
                (None, Some(d)) if !ask => Ok(d.to_owned()),
                (None, _) if !ask => {
                    missing.push(flag);
                    Ok(String::new())
                }
                (None, Some(d)) => {
                    let typed = secret::prompt_line(&format!("{prompt}[{d}] "))?;
                    Ok(if typed.is_empty() {
                        d.to_owned()
                    } else {
                        typed
                    })
                }
                (None, None) => secret::prompt_line(prompt),
            }
        };
    let name = value(
        args.name,
        "--name",
        "Account name in neomail: ",
        preset.default_name,
    )?;
    let username = value(args.username, "--username", preset.username_prompt, None)?;
    let host = match preset.host {
        Some(host) => host.to_owned(),
        None => value(args.host, "--host", "IMAP server: ", None)?,
    };
    if !missing.is_empty() {
        return Err(secret::cannot_ask(input, &missing.join(" and ")));
    }
    for (what, v) in [
        ("account name", &name),
        ("username", &username),
        ("host", &host),
    ] {
        if v.is_empty() {
            bail!("the {what} is empty");
        }
    }
    config::check_can_add(config_path, &name)?;
    let security = args.security.unwrap_or(preset.security);
    let port = args.port.unwrap_or(if security == preset.security {
        preset.port
    } else {
        match security {
            Security::Tls => 993,
            Security::StartTls => 143,
        }
    });
    let password_command = args.password_command.filter(|c| !c.trim().is_empty());
    let smtp = smtp_settings(args.smtp_host, args.smtp_port, args.smtp_security)?;
    Ok(Plan {
        imap: ImapConfig {
            host,
            port,
            security,
            username,
            cert_pin: args.cert_sha256,
        },
        check: !args.no_check,
        name,
        password_command,
        smtp,
    })
}

/// `--smtp-host`, `--smtp-port` and `--smtp-security`: without `--smtp-security`, port 587 is
/// submission with STARTTLS (RFC 6409 section 3.1), any other port TLS from the first byte (RFC
/// 8314 section 3.3). Without `--smtp-port`, 587 for STARTTLS and 465 for TLS, as the config
/// file has it. A server named here keeps no copy of sent mail, so neomail appends one to Sent,
/// except Gmail's ([`config::keeps_sent_copy`]).
fn smtp_settings(
    host: Option<String>,
    port: Option<u16>,
    security: Option<Security>,
) -> Result<Option<config::SmtpSettings>> {
    let Some(host) = host.map(|h| h.trim().to_owned()) else {
        return Ok(None);
    };
    if host.is_empty() {
        return Err(crate::usage("--smtp-host is empty"));
    }
    let security = security.unwrap_or(if port == Some(587) {
        Security::StartTls
    } else {
        Security::Tls
    });
    let port = port.unwrap_or(match security {
        Security::Tls => 465,
        Security::StartTls => 587,
    });
    let save_sent = !config::keeps_sent_copy(&host);
    Ok(Some(config::SmtpSettings {
        host,
        port,
        security,
        cert_pin: None,
        save_sent,
    }))
}

/// Checks that the account can be appended, reads the password, checks the login, then stores
/// the password and appends the account. A failure before the store writes nothing and stores
/// nothing.
pub fn execute(
    plan: Plan,
    config_path: &Path,
    deps: &Deps<'_>,
    out: &mut impl Write,
) -> Result<()> {
    let name = plan.name.clone();
    let block = |plan: &Plan| {
        config::password_account_toml(
            &name,
            &plan.imap,
            plan.password_command.as_deref(),
            plan.smtp
                .clone()
                .or_else(|| config::preset_smtp(&plan.imap))
                .as_ref(),
        )
    };
    config::check_append(config_path, &name, &block(&plan))?;

    let (keyring, password) = match &plan.password_command {
        Some(_) if !plan.check => (None, None),
        Some(command) => (None, Some(secret::run_password_command(command)?)),
        // Before the password: no one should type it only to learn it cannot be stored.
        None => {
            let keyring = (deps.open_keyring)()?;
            (Some(keyring), Some((deps.read_password)()?))
        }
    };
    if let Some(password) = password.as_ref().filter(|_| plan.check) {
        let count = (deps.check_login)(&plan.imap, password)
            .context("the login check failed; nothing was written (--no-check skips it)")?;
        eprintln!(
            "Logged in to {}:{} as {} and opened INBOX read-only: {count} messages.",
            plan.imap.host, plan.imap.port, plan.imap.username
        );
    }
    let stored = match (&keyring, &password) {
        (Some(keyring), Some(password)) => {
            keyring.set(&name, password)?;
            format!(
                "Its password is in {} (service {:?}, username {name:?}).",
                keyring.name(),
                keyring::SERVICE
            )
        }
        _ => "It runs its password_command for the password.".to_owned(),
    };
    config::append_account(config_path, &name, &block(&plan)).with_context(|| {
        if keyring.is_some() {
            "the password is stored in the keyring, but the config file could not be updated"
        } else {
            "the config file could not be updated"
        }
    })?;
    writeln!(
        out,
        "Added {name:?} ({}) to {}. {stored}\nNext: neomail sync --account {name}",
        plan.imap.username,
        config_path.display(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use keyring_core::mock;

    use super::*;
    use crate::config::{Config, Login, PasswordSource};

    const SECRET: &str = "s3cret-fake-bridge-password";
    const OFF_TERMINAL: Input = Input {
        terminal: false,
        no_input: false,
    };

    /// Fake steps: a mock keyring, a fixed password, and a login check that succeeds unless
    /// `refuse` is set. Counts the logins checked.
    struct Fakes {
        store: Arc<mock::Store>,
        refuse: bool,
        logins: Cell<usize>,
    }

    impl Fakes {
        fn new() -> Self {
            Self {
                store: mock::Store::new().unwrap(),
                refuse: false,
                logins: Cell::new(0),
            }
        }

        fn keyring(&self) -> Keyring {
            Keyring::with_store(self.store.clone(), "the mock", false)
        }

        fn run(&self, plan: Plan, path: &Path) -> Result<String> {
            let open = || Ok(self.keyring());
            let password = || Ok(Password::new(SECRET.to_owned()));
            let check = |imap: &ImapConfig, password: &Password| {
                self.logins.set(self.logins.get() + 1);
                assert_eq!(password.expose(), SECRET);
                if self.refuse {
                    bail!("login as {} failed", imap.username);
                }
                Ok(7)
            };
            let deps = Deps {
                open_keyring: &open,
                read_password: &password,
                check_login: &check,
            };
            let mut out = Vec::new();
            execute(plan, path, &deps, &mut out)?;
            Ok(String::from_utf8(out).unwrap())
        }

        fn stored(&self, name: &str) -> Option<String> {
            self.keyring().get(name).ok().map(|p| p.expose().to_owned())
        }
    }

    fn imap_args() -> Args {
        Args {
            name: Some("work".into()),
            username: Some("user@mail.example".into()),
            host: Some("imap.mail.example".into()),
            ..Args::default()
        }
    }

    #[test]
    fn imap_writes_its_server_and_the_password_goes_to_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes::new();
        let plan = gather(&IMAP, &path, imap_args(), OFF_TERMINAL).unwrap();
        let out = fakes.run(plan, &path).unwrap();
        assert!(out.contains("Next: neomail sync --account work"), "{out}");
        assert!(!out.contains(SECRET), "{out}");
        assert_eq!(fakes.logins.get(), 1);
        assert_eq!(fakes.stored("work").as_deref(), Some(SECRET));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(SECRET), "{text}");
        let config = Config::parse(&text).unwrap();
        let a = config.find("work").unwrap();
        let imap = a.imap().unwrap();
        assert_eq!(
            (imap.host.as_str(), imap.port, imap.security, imap.cert_pin),
            ("imap.mail.example", 993, Security::Tls, None)
        );
        assert_eq!(a.login, Login::Password(PasswordSource::Keyring));
    }

    #[test]
    fn imap_takes_port_security_and_a_password_command_without_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let args = Args {
            name: Some("work".into()),
            username: Some("user@mail.example".into()),
            host: Some("imap.mail.example".into()),
            security: Some(Security::StartTls),
            password_command: Some(format!("printf {SECRET}")),
            ..Args::default()
        };
        let plan = gather(&IMAP, &path, args, OFF_TERMINAL).unwrap();
        assert_eq!(
            (plan.imap.port, plan.imap.security),
            (143, Security::StartTls)
        );
        let fakes = Fakes::new();
        let open =
            || -> Result<Keyring> { unreachable!("a password command never opens the keyring") };
        let password = || -> Result<Password> { unreachable!() };
        let check = |_: &ImapConfig, p: &Password| {
            assert_eq!(p.expose(), SECRET, "the command's output is what logs in");
            Ok(0)
        };
        let deps = Deps {
            open_keyring: &open,
            read_password: &password,
            check_login: &check,
        };
        execute(plan, &path, &deps, &mut Vec::new()).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(
            c.find("work").unwrap().login,
            Login::Password(PasswordSource::Command(format!("printf {SECRET}")))
        );
        assert!(fakes.stored("work").is_none());
    }

    #[test]
    fn a_refused_login_writes_nothing_and_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes {
            refuse: true,
            ..Fakes::new()
        };
        let plan = gather(&IMAP, &path, imap_args(), OFF_TERMINAL).unwrap();
        let e = format!("{:#}", fakes.run(plan, &path).unwrap_err());
        assert!(
            e.starts_with("the login check failed; nothing was written"),
            "{e}"
        );
        assert!(!e.contains(SECRET), "{e}");
        assert!(!path.exists());
        assert!(fakes.stored("work").is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn no_check_skips_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes {
            refuse: true,
            ..Fakes::new()
        };
        let args = Args {
            no_check: true,
            ..imap_args()
        };
        let plan = gather(&IMAP, &path, args, OFF_TERMINAL).unwrap();
        fakes.run(plan, &path).unwrap();
        assert_eq!(fakes.logins.get(), 0);
        assert!(Config::load(&path).unwrap().find("work").is_some());
        assert_eq!(fakes.stored("work").as_deref(), Some(SECRET));
    }

    #[test]
    fn imap_writes_the_smtp_server_it_is_given_and_appends_sent_copies() {
        for (port, security, expected) in [
            (None, None, (465, Security::Tls)),
            (Some(587), None, (587, Security::StartTls)),
            (Some(2465), None, (2465, Security::Tls)),
            (
                Some(2525),
                Some(Security::StartTls),
                (2525, Security::StartTls),
            ),
            (Some(587), Some(Security::Tls), (587, Security::Tls)),
            (None, Some(Security::StartTls), (587, Security::StartTls)),
            (None, Some(Security::Tls), (465, Security::Tls)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let args = Args {
                name: Some("work".into()),
                username: Some("user@mail.example".into()),
                host: Some("imap.mail.example".into()),
                smtp_host: Some("smtp.mail.example".into()),
                smtp_port: port,
                smtp_security: security,
                ..Args::default()
            };
            let plan = gather(&IMAP, &path, args, OFF_TERMINAL).unwrap();
            Fakes::new().run(plan, &path).unwrap();
            let config = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let smtp = config.find("work").unwrap().smtp.clone().unwrap();
            assert_eq!(smtp.host, "smtp.mail.example");
            assert_eq!((smtp.port, smtp.security), expected, "{port:?}");
            assert!(
                smtp.save_sent,
                "a server neomail does not know keeps no copy"
            );
        }
        // Gmail's SMTP keeps its own copy in Sent, so neomail appends none.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let args = Args {
            name: Some("work".into()),
            username: Some("user@mail.example".into()),
            host: Some("imap.mail.example".into()),
            smtp_host: Some("smtp.gmail.com".into()),
            ..Args::default()
        };
        let plan = gather(&IMAP, &path, args, OFF_TERMINAL).unwrap();
        Fakes::new().run(plan, &path).unwrap();
        let config = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!config.find("work").unwrap().smtp.clone().unwrap().save_sent);
        // Without one, a server neomail does not know gets no smtp at all.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let args = Args {
            name: Some("work".into()),
            username: Some("user@mail.example".into()),
            host: Some("imap.mail.example".into()),
            ..Args::default()
        };
        let plan = gather(&IMAP, &path, args, OFF_TERMINAL).unwrap();
        Fakes::new().run(plan, &path).unwrap();
        let config = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config.find("work").unwrap().smtp, None);
    }

    #[test]
    fn off_a_terminal_or_with_no_input_missing_flags_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let no_input = Input {
            terminal: true,
            no_input: true,
        };
        for (input, why) in [
            (OFF_TERMINAL, "standard input is not a terminal"),
            (no_input, "--no-input (or NEOMAIL_NO_INPUT) forbids prompts"),
        ] {
            let e = gather(&IMAP, &path, Args::default(), input).unwrap_err();
            assert!(e.is::<crate::UsageError>(), "{e:#}");
            let e = format!("{e:#}");
            assert!(e.starts_with(why), "{e}");
            assert!(e.ends_with("pass --name and --username and --host"), "{e}");
        }
        assert!(!path.exists());
    }

    #[test]
    fn a_taken_name_fails_before_any_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes::new();
        let plan = gather(&IMAP, &path, imap_args(), OFF_TERMINAL).unwrap();
        fakes.run(plan, &path).unwrap();
        let e = gather(&IMAP, &path, imap_args(), OFF_TERMINAL).unwrap_err();
        assert!(
            format!("{e:#}").contains("already has an account named \"work\""),
            "{e:#}"
        );
    }
}
