//! `neomail account add` for the password providers: any IMAP server, Proton Mail Bridge and
//! Gmail with an app password. A provider is data (a [`Preset`]); one flow gathers what the flags
//! left out, reads and pins the certificate when the preset says so, checks the login and writes
//! the account with the shared config writer. Nothing touching the network or the keyring is
//! called directly: [`Deps`] holds those steps, so the tests give in-memory ones.

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
    /// Read the server's certificate and pin it (Bridge's is self-signed).
    pub pin: bool,
    /// Shown on a terminal before anything is asked.
    pub intro: &'static str,
}

pub const IMAP: Preset = Preset {
    id: "imap",
    default_name: None,
    host: None,
    port: 993,
    security: Security::Tls,
    username_prompt: "Username: ",
    password_prompt: "Password (not shown): ",
    pin: false,
    intro: "",
};

pub const PROTON: Preset = Preset {
    id: "proton",
    default_name: Some("proton"),
    host: Some("127.0.0.1"),
    port: 1143,
    security: Security::StartTls,
    username_prompt: "Username Bridge shows (your Proton address): ",
    password_prompt: "The password Bridge shows (not your Proton password; not shown): ",
    pin: true,
    intro: "\
Proton Mail through Proton Mail Bridge (paid Proton plans), on 127.0.0.1 port 1143 with
STARTTLS. Start Bridge and open the account's mailbox details: it shows a username and a
password for other apps. neomail asks for that password, never your Proton password, and pins
Bridge's self-signed certificate after showing you its SHA-256.
",
};

pub const GMAIL_APP_PASSWORD: Preset = Preset {
    id: "gmail-app-password",
    default_name: Some("gmail"),
    host: Some("imap.gmail.com"),
    port: 993,
    security: Security::Tls,
    username_prompt: "Gmail address: ",
    password_prompt: "App password (not shown): ",
    pin: false,
    intro: "\
Gmail with an app password, over IMAP (imap.gmail.com, port 993, TLS). Google: \"To create an
app password, you need 2-Step Verification on your Google Account\"
(https://support.google.com/accounts/answer/185833). Create one at
https://myaccount.google.com/apppasswords and paste it below.
",
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
    /// Read the server's certificate, and pin it: it must equal `expected` when one was given,
    /// else the user confirms it.
    pub read_pin: Option<Option<CertPin>>,
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
    pub read_certificate: &'a dyn Fn(&ImapConfig) -> Result<CertPin>,
    pub confirm_pin: &'a dyn Fn() -> Result<bool>,
    /// Logs in read-only and returns how many messages INBOX holds.
    pub check_login: &'a dyn Fn(&ImapConfig, &Password) -> Result<u32>,
}

/// `neomail account add imap|proton|gmail-app-password`.
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
        read_certificate: &|imap| {
            let addr = format!("{}:{}", imap.host, imap.port);
            crate::account::runtime()?
                .block_on(nm_imap::server_certificate(imap))
                .with_context(|| {
                    format!(
                        "cannot read the certificate of {addr} (is it running? --no-check with \
                         --cert-sha256 skips connecting); nothing was written"
                    )
                })
        },
        confirm_pin: &|| Ok(is_yes(&secret::prompt_line("Pin it? [y/N] ")?)),
        check_login: &|imap, password| {
            let login = nm_imap::Login::Password(password.expose());
            Ok(crate::account::runtime()?.block_on(nm_imap::check_login(imap, login))?)
        },
    };
    execute(plan, config_path, &deps, out)
}

/// A yes to a `[y/N]` question: `y` or `yes`, in any case.
fn is_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
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
    if ask && !preset.intro.is_empty() {
        eprintln!("{}", preset.intro);
    }
    if preset.pin && args.cert_sha256.is_none() && args.no_check {
        return Err(crate::usage(
            "--no-check does not connect, so the certificate cannot be read: pass --cert-sha256 \
             too",
        ));
    }
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
    let pin_missing = preset.pin && args.cert_sha256.is_none() && !ask;
    if pin_missing {
        missing.push("--cert-sha256");
    }
    if !missing.is_empty() {
        let mut flags = missing.join(" and ");
        if pin_missing {
            flags.push_str(&format!(
                " (the certificate's SHA-256, which cannot be confirmed here: run `neomail \
                 account add {}` on a terminal to see and confirm it)",
                preset.id
            ));
        }
        return Err(secret::cannot_ask(input, &flags));
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
        read_pin: (preset.pin && !args.no_check).then_some(args.cert_sha256),
        check: !args.no_check,
        name,
        password_command,
        smtp,
    })
}

/// `--smtp-host`, `--smtp-port` and `--smtp-security`: without `--smtp-security`, port 587 is
/// submission with STARTTLS (RFC 6409 section 3.1), any other port TLS from the first byte, 465 by
/// default (RFC 8314 section 3.3). A server named here keeps no copy of sent mail, so neomail
/// appends one to Sent.
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
    let port = port.unwrap_or(465);
    Ok(Some(config::SmtpSettings {
        host,
        port,
        security: security.unwrap_or(if port == 587 {
            Security::StartTls
        } else {
            Security::Tls
        }),
        cert_pin: None,
        save_sent: true,
    }))
}

/// Checks that the account can be appended, pins the certificate, reads the password, checks the
/// login, then stores the password and appends the account. A failure before the store writes
/// nothing and stores nothing.
pub fn execute(
    mut plan: Plan,
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

    if let Some(expected) = plan.read_pin {
        let shown = (deps.read_certificate)(&plan.imap)?;
        eprintln!(
            "The certificate at {}:{} has the SHA-256\n  {}",
            plan.imap.host,
            plan.imap.port,
            colon_hex(&shown)
        );
        match expected {
            Some(expected) if expected != shown => bail!(
                "the certificate's SHA-256 does not match --cert-sha256 {}; nothing was written",
                colon_hex(&expected)
            ),
            Some(_) => {}
            None if (deps.confirm_pin)()? => {}
            None => bail!("the certificate was not pinned; nothing was written"),
        }
        plan.imap.cert_pin = Some(shown);
    }

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

/// A pin as `openssl x509 -fingerprint -sha256` prints it: `AB:CD:…`.
pub fn colon_hex(pin: &CertPin) -> String {
    let hex = pin.to_string().to_ascii_uppercase();
    hex.as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":")
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

    fn pin(byte: &str) -> CertPin {
        byte.repeat(32).parse().unwrap()
    }

    /// Fake steps: a mock keyring, a fixed password and certificate, and a login check that
    /// succeeds unless `refuse` is set. Counts the logins checked.
    struct Fakes {
        store: Arc<mock::Store>,
        cert: CertPin,
        confirm: bool,
        refuse: bool,
        logins: Cell<usize>,
    }

    impl Fakes {
        fn new() -> Self {
            Self {
                store: mock::Store::new().unwrap(),
                cert: pin("ab"),
                confirm: true,
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
            let cert = |_: &ImapConfig| Ok(self.cert);
            let confirm = || Ok(self.confirm);
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
                read_certificate: &cert,
                confirm_pin: &confirm,
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

    fn proton_args(cert: Option<CertPin>) -> Args {
        Args {
            username: Some("you@example.com".into()),
            cert_sha256: cert,
            ..Args::default()
        }
    }

    #[test]
    fn each_preset_writes_its_server_and_the_password_goes_to_the_keyring() {
        for (preset, host, port, security, pinned) in [
            (&PROTON, "127.0.0.1", 1143, Security::StartTls, true),
            (
                &GMAIL_APP_PASSWORD,
                "imap.gmail.com",
                993,
                Security::Tls,
                false,
            ),
            (&IMAP, "imap.mail.example", 993, Security::Tls, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let args = Args {
                name: Some("acct".into()),
                host: (preset.host.is_none()).then(|| host.to_owned()),
                ..proton_args(pinned.then(|| pin("ab")))
            };
            let fakes = Fakes::new();
            let plan = gather(preset, &path, args, OFF_TERMINAL).unwrap();
            let out = fakes.run(plan, &path).unwrap();
            assert!(out.contains("Next: neomail sync --account acct"), "{out}");
            assert!(!out.contains(SECRET), "{out}");
            assert_eq!(fakes.logins.get(), 1);
            assert_eq!(fakes.stored("acct").as_deref(), Some(SECRET));
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains(SECRET), "{text}");
            let config = Config::parse(&text).unwrap();
            let a = config.find("acct").unwrap();
            assert_eq!(
                (
                    a.imap().unwrap().host.as_str(),
                    a.imap().unwrap().port,
                    a.imap().unwrap().security
                ),
                (host, port, security),
                "{}",
                preset.id
            );
            assert_eq!(a.imap().unwrap().cert_pin, pinned.then(|| pin("ab")));
            assert_eq!(a.login, Login::Password(PasswordSource::Keyring));
        }
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
        let cert = |_: &ImapConfig| -> Result<CertPin> { unreachable!() };
        let confirm = || -> Result<bool> { unreachable!() };
        let check = |_: &ImapConfig, p: &Password| {
            assert_eq!(p.expose(), SECRET, "the command's output is what logs in");
            Ok(0)
        };
        let deps = Deps {
            open_keyring: &open,
            read_password: &password,
            read_certificate: &cert,
            confirm_pin: &confirm,
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
        let plan = gather(&PROTON, &path, proton_args(Some(pin("ab"))), OFF_TERMINAL).unwrap();
        let e = format!("{:#}", fakes.run(plan, &path).unwrap_err());
        assert!(
            e.starts_with("the login check failed; nothing was written"),
            "{e}"
        );
        assert!(!e.contains(SECRET), "{e}");
        assert!(!path.exists());
        assert!(fakes.stored("proton").is_none());
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
            ..proton_args(Some(pin("cd")))
        };
        let plan = gather(&PROTON, &path, args, OFF_TERMINAL).unwrap();
        assert!(plan.read_pin.is_none(), "--no-check does not connect");
        fakes.run(plan, &path).unwrap();
        assert_eq!(fakes.logins.get(), 0);
        let c = Config::load(&path).unwrap();
        assert_eq!(
            c.find("proton").unwrap().imap().unwrap().cert_pin,
            Some(pin("cd"))
        );
        assert_eq!(fakes.stored("proton").as_deref(), Some(SECRET));
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
    fn a_pin_mismatch_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes::new();
        let plan = gather(&PROTON, &path, proton_args(Some(pin("cd"))), OFF_TERMINAL).unwrap();
        let e = format!("{:#}", fakes.run(plan, &path).unwrap_err());
        assert!(e.contains("does not match --cert-sha256 CD:CD:"), "{e}");
        assert!(e.ends_with("nothing was written"), "{e}");
        assert!(!path.exists());
        assert_eq!(fakes.logins.get(), 0);
        assert!(fakes.stored("proton").is_none());
    }

    #[test]
    fn an_unconfirmed_pin_writes_nothing_and_a_confirmed_one_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let terminal = Input {
            terminal: true,
            no_input: false,
        };
        // Every value is given, so nothing but the pin is asked.
        let args = || Args {
            name: Some("proton".into()),
            ..proton_args(None)
        };
        let plan = || gather(&PROTON, &path, args(), terminal).unwrap();
        let declined = Fakes {
            confirm: false,
            ..Fakes::new()
        };
        let e = format!("{:#}", declined.run(plan(), &path).unwrap_err());
        assert_eq!(e, "the certificate was not pinned; nothing was written");
        assert!(!path.exists());
        let fakes = Fakes::new();
        fakes.run(plan(), &path).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(
            c.find("proton").unwrap().imap().unwrap().cert_pin,
            Some(pin("ab"))
        );
        // Bridge's SMTP is written with the pin just confirmed, and keeps its own Sent copy.
        assert_eq!(
            c.find("proton").unwrap().smtp,
            Some(config::SmtpSettings {
                host: "127.0.0.1".into(),
                port: 1025,
                security: Security::StartTls,
                cert_pin: Some(pin("ab")),
                save_sent: false,
            })
        );
    }

    #[test]
    fn off_a_terminal_or_with_no_input_proton_needs_the_pin_and_missing_flags_are_named() {
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
            let e = gather(&PROTON, &path, proton_args(None), input).unwrap_err();
            assert!(e.is::<crate::UsageError>(), "{e:#}");
            let e = format!("{e:#}");
            assert!(
                e.starts_with(why)
                    && e.contains("pass --cert-sha256 (")
                    && e.contains(
                        "run `neomail account add proton` on a terminal to see and confirm it"
                    ),
                "{e}"
            );
            // Every missing flag at once, the pin included.
            let e = gather(&PROTON, &path, Args::default(), input).unwrap_err();
            let e = format!("{e:#}");
            assert!(e.contains("pass --username and --cert-sha256 ("), "{e}");
            let e = gather(&IMAP, &path, Args::default(), input).unwrap_err();
            assert!(e.is::<crate::UsageError>(), "{e:#}");
            let e = format!("{e:#}");
            assert!(e.ends_with("pass --name and --username and --host"), "{e}");
        }
        let args = Args {
            no_check: true,
            ..proton_args(None)
        };
        let e = gather(&PROTON, &path, args, OFF_TERMINAL).unwrap_err();
        assert!(format!("{e:#}").contains("pass --cert-sha256 too"), "{e:#}");
        assert!(!path.exists());
    }

    #[test]
    fn the_pin_question_takes_y_or_yes_in_any_case() {
        for yes in ["y", "Y", "yes", "YES", "Yes", " yes "] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "n", "no", "ye", "yess", "sure"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn a_taken_name_fails_before_any_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let fakes = Fakes::new();
        let plan = gather(&PROTON, &path, proton_args(Some(pin("ab"))), OFF_TERMINAL).unwrap();
        fakes.run(plan, &path).unwrap();
        let e = gather(&PROTON, &path, proton_args(Some(pin("ab"))), OFF_TERMINAL).unwrap_err();
        assert!(
            format!("{e:#}").contains("already has an account named \"proton\""),
            "{e:#}"
        );
    }

    #[test]
    fn colon_hex_is_openssls_format() {
        assert_eq!(colon_hex(&pin("0a"))[..8], *"0A:0A:0A");
        assert_eq!(colon_hex(&pin("0a")).len(), 32 * 3 - 1);
    }
}
