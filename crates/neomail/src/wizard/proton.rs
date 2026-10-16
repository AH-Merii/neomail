//! `neomail account add proton`: Proton Mail through Proton Mail Bridge (issue #50). Bridge runs
//! on this computer and serves the mailbox over IMAP and SMTP on 127.0.0.1 with its own
//! self-signed certificate and its own password. neomail never asks for the Proton password.

mod probe;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use nm_imap::{CertPin, ImapConfig, Security};

use self::probe::{ProbeError, Protocol};
use super::cli::{self, Saved, Ui};
use super::gmail::{NAME_TAKEN, REFUSED, check_login, clean_name, name_taken, other};
use super::term::Tty;
use super::{
    Answers, CheckError, Choice, Cleaned, Kind, Mistake, Question, Session, StartError, Step,
    Wizard,
};
use crate::config::{self, SmtpSettings};
use crate::keyring::{self, Keyring};
use crate::secret::{self, Input, Password};

const BRIDGE_PAGE: &str = "https://proton.me/mail/bridge";
const SETTINGS_GUIDE: &str = "https://proton.me/support/comprehensive-guide-to-bridge-settings";

pub static PROTON: Wizard = Wizard {
    id: "proton",
    title: "a Proton Mail account, through Proton Mail Bridge",
    steps: &[
        Step {
            id: "plan",
            title: "A paid Proton plan",
            why: "\
Proton Mail Bridge, which neomail reads Proton Mail through, comes only with a paid plan that
includes Mail.
Warning: on a free plan Bridge refuses the sign-in in step 3.",
            links: &[BRIDGE_PAGE, "https://proton.me/mail/pricing"],
            page: Some("https://proton.me/mail/pricing"),
            kind: Kind::Console,
        },
        Step {
            id: "install",
            title: "Install Proton Mail Bridge and a keyring",
            why: "\
Bridge decrypts your mail on this computer and serves it to apps such as neomail.
- Install it from Proton's page or your distribution (Arch: protonmail-bridge, or
  protonmail-bridge-core without the window).
Note: Bridge keeps its keys in a keyring: GNOME Keyring, KWallet, KeePassXC or pass.",
            links: &[
                "https://proton.me/support/bridge-for-linux",
                "https://proton.me/support/bridge-cli-guide",
            ],
            page: Some("https://proton.me/support/bridge-for-linux"),
            kind: Kind::Console,
        },
        Step {
            id: "sign-in",
            title: "Sign in to Bridge, and keep it running",
            why: "\
- Open Bridge and sign in with your Proton address and password, or run
  `protonmail-bridge --cli` and `login`.
Your Proton password goes only into Bridge, never into neomail.
- Keep Bridge running (its window in the tray, or `protonmail-bridge --noninteractive`, say from
  its systemd --user unit): neomail syncs and sends through it.",
            links: &["https://proton.me/support/bridge-cli-guide"],
            page: None,
            kind: Kind::Console,
        },
        Step {
            id: "export",
            title: "Export Bridge's certificate",
            why: "\
Bridge secures the connection with a certificate it made itself, which no authority vouches for.
neomail pins it, and checks it against a copy you export from Bridge now, before the mailbox
details are open.
- In Bridge, open Settings > Advanced settings > Export TLS certificates and pick a folder, or run
  `cert export` in `protonmail-bridge --cli`.
Warning: the export also writes key.pem, Bridge's private key, next to cert.pem: delete it once
the certificate is pinned.",
            links: &[SETTINGS_GUIDE, "https://proton.me/support/bridge-cli-guide"],
            page: None,
            kind: Kind::Console,
        },
        Step {
            id: "name",
            title: "Name the account",
            why: "What neomail calls it, as in `neomail sync --account NAME`.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--name"),
                prompt: "Account name in neomail: ",
                secret: false,
                default: Some("proton"),
                choices: &[],
                clean: clean_name,
            }),
        },
        Step {
            id: "name-free",
            title: "Check that the name is free",
            why: "No account in the config has this name yet.",
            links: &[],
            page: None,
            kind: Kind::Check { reads: &["name"] },
        },
        Step {
            id: "username",
            title: "Mailbox details: the username",
            why: "\
- In Bridge, pick the account and open its mailbox details (in `protonmail-bridge --cli`: `info`).
The next questions copy them. The username is usually your Proton address.
Note: with split addresses mode each address has its own details: add one neomail account per
address.",
            links: &[
                "https://proton.me/support/difference-combined-addresses-mode-split-addresses-mode",
            ],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--username"),
                prompt: "Username: ",
                secret: false,
                default: None,
                choices: &[],
                clean: clean_username,
            }),
        },
        Step {
            id: "imap-port",
            title: "Mailbox details: the IMAP port",
            why: "Bridge serves IMAP on 127.0.0.1, by default on port 1143.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--port"),
                prompt: "IMAP port: ",
                secret: false,
                default: Some("1143"),
                choices: &[],
                clean: clean_port,
            }),
        },
        Step {
            id: "imap-security",
            title: "Mailbox details: the IMAP security",
            why: "\
STARTTLS (the default), or SSL, which neomail calls tls: TLS from the first byte. Bridge sets it
under Settings > Advanced settings > Connection mode.",
            links: &[SETTINGS_GUIDE],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--security"),
                prompt: "IMAP security (starttls or tls): ",
                secret: false,
                default: Some("starttls"),
                choices: &[
                    Choice {
                        value: "starttls",
                        label: "STARTTLS, Bridge's default",
                    },
                    Choice {
                        value: "tls",
                        label: "SSL: TLS from the first byte",
                    },
                ],
                clean: clean_security,
            }),
        },
        Step {
            id: "smtp-port",
            title: "Mailbox details: the SMTP port",
            why: "neomail sends through Bridge's SMTP on 127.0.0.1, by default on port 1025.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--smtp-port"),
                prompt: "SMTP port: ",
                secret: false,
                default: Some("1025"),
                choices: &[],
                clean: clean_port,
            }),
        },
        Step {
            id: "smtp-security",
            title: "Mailbox details: the SMTP security",
            why: "STARTTLS (the default), or SSL, which neomail calls tls.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--smtp-security"),
                prompt: "SMTP security (starttls or tls): ",
                secret: false,
                default: Some("starttls"),
                choices: &[
                    Choice {
                        value: "starttls",
                        label: "STARTTLS, Bridge's default",
                    },
                    Choice {
                        value: "tls",
                        label: "SSL: TLS from the first byte",
                    },
                ],
                clean: clean_security,
            }),
        },
        Step {
            id: "bridge",
            title: "Is Bridge ready?",
            why: "neomail connects to both ports as you answered and reads Bridge's certificate.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &["imap-port", "imap-security", "smtp-port", "smtp-security"],
            },
        },
        Step {
            id: "certificate",
            title: "Pin Bridge's certificate",
            why: "\
neomail pins the SHA-256 of the certificate Bridge presented, shown above: it then talks only to
the server holding that certificate. Give the path of the cert.pem you exported and neomail
compares the two, so the pin is Bridge's own certificate rather than one read over the same
connection.
Note: then delete the key.pem the export wrote next to it.",
            links: &[SETTINGS_GUIDE],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--cert-sha256"),
                prompt: "Pin it? y, the path to the exported cert.pem, or its SHA-256 [y/path/N] ",
                secret: false,
                default: None,
                choices: &[],
                clean: clean_certificate,
            }),
        },
        Step {
            id: "pin",
            title: "Compare and pin the certificate",
            why: "The cert.pem or SHA-256 given must be the certificate Bridge presented.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &["certificate"],
            },
        },
        Step {
            id: "ready",
            title: "Check that the account can be saved",
            why: "The config can take the account and the keyring opens, before the password \
                  is asked.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &[
                    "name",
                    "username",
                    "imap-port",
                    "imap-security",
                    "smtp-port",
                    "smtp-security",
                ],
            },
        },
        Step {
            id: "password",
            title: "Paste the Bridge password",
            why: "\
The password Bridge's mailbox details show, not your Proton password. It goes to the keyring.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: None,
                prompt: "Bridge password (not shown): ",
                secret: true,
                default: None,
                choices: &[],
                clean: clean_password,
            }),
        },
        Step {
            id: "login",
            title: "Check the login",
            why: "neomail logs in to Bridge's IMAP, reading nothing, and to its SMTP, sending \
                  nothing, with the pinned certificate, before anything is written.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &[
                    "username",
                    "imap-port",
                    "imap-security",
                    "smtp-port",
                    "smtp-security",
                    "password",
                ],
            },
        },
        Step {
            id: "write",
            title: "Save the account",
            why: "The Bridge password goes to the keyring and the account to the config.",
            links: &[],
            page: None,
            kind: Kind::Commit,
        },
    ],
    mistakes: &[
        Mistake {
            check: "name-free",
            kind: NAME_TAKEN,
            step: "name",
            fix: "Choose another name (--name).",
        },
        Mistake {
            check: "bridge",
            kind: IMAP_REFUSED,
            step: "imap-port",
            fix: NOT_RUNNING_FIX,
        },
        Mistake {
            check: "bridge",
            kind: IMAP_OTHER,
            step: "imap-port",
            fix: "Check the IMAP port against Bridge's mailbox details (--port).",
        },
        Mistake {
            check: "bridge",
            kind: IMAP_SPEAKS,
            step: "imap-security",
            fix: SPEAKS_FIX,
        },
        Mistake {
            check: "bridge",
            kind: SMTP_REFUSED,
            step: "smtp-port",
            fix: NOT_RUNNING_FIX,
        },
        Mistake {
            check: "bridge",
            kind: SMTP_OTHER,
            step: "smtp-port",
            fix: "Check the SMTP port against Bridge's mailbox details (--smtp-port).",
        },
        Mistake {
            check: "bridge",
            kind: SMTP_SPEAKS,
            step: "smtp-security",
            fix: SPEAKS_FIX,
        },
        Mistake {
            check: "pin",
            kind: MISMATCH,
            step: "certificate",
            fix: "\
Export the certificate again from the Bridge that runs now and give that cert.pem, or check the
SHA-256 (--bridge-cert, --cert-sha256). Bridge makes a new certificate when it is reset or
installed again.",
        },
        Mistake {
            check: "pin",
            kind: CERT_FILE,
            step: "certificate",
            fix: "Give the cert.pem that Bridge's export wrote (--bridge-cert).",
        },
        Mistake {
            check: "login",
            kind: REFUSED,
            step: "password",
            fix: REFUSED_FIX,
        },
    ],
};

const IMAP_REFUSED: &str = "imap-refused";
const IMAP_SPEAKS: &str = "imap-speaks";
const IMAP_OTHER: &str = "imap-other";
const SMTP_REFUSED: &str = "smtp-refused";
const SMTP_SPEAKS: &str = "smtp-speaks";
const SMTP_OTHER: &str = "smtp-other";
const MISMATCH: &str = "mismatch";
const CERT_FILE: &str = "cert-file";
const NOT_PINNED: &str = "not-pinned";

const NOT_RUNNING_FIX: &str = "\
Start Bridge (its window, its systemd --user unit, or `protonmail-bridge --noninteractive`) and
keep it running, or check the port against Bridge's mailbox details.";

const SPEAKS_FIX: &str = "\
Answer the security Bridge's mailbox details show: SSL is tls. Bridge sets it under Settings >
Advanced settings > Connection mode.";

const REFUSED_FIX: &str = "\
Bridge refused this username and password. Paste the password Bridge's mailbox details show, not
your Proton password, and check the username (on a terminal, Esc goes back to it). If Bridge
lists no account, sign in to Bridge first.";

/// The username Bridge shows: usually an address. Bridge checks it; here it only has to be one
/// word.
fn clean_username(typed: &str) -> Result<Cleaned, &'static str> {
    let username = typed.trim();
    if username.is_empty() || username.contains(char::is_whitespace) {
        return Err("Give the username Bridge's mailbox details show, such as you@proton.me.");
    }
    Ok(Cleaned::ok(username))
}

fn clean_port(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim().parse::<u16>() {
        Ok(port) if port > 0 => Ok(Cleaned::ok(port.to_string())),
        _ => Err("That is not a port: give the number Bridge's mailbox details show."),
    }
}

/// `starttls`, or `tls`, which Bridge calls SSL.
fn clean_security(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim().to_ascii_lowercase().as_str() {
        "starttls" => Ok(Cleaned::ok("starttls")),
        "tls" | "ssl" => Ok(Cleaned::ok("tls")),
        _ => Err("Type starttls or tls (Bridge's SSL)."),
    }
}

fn security(answer: Option<&str>) -> Security {
    match answer {
        Some("tls") => Security::Tls,
        _ => Security::StartTls,
    }
}

/// How the certificate is confirmed: `yes`, `no`, `sha256:HEX` or `file:PATH`.
fn clean_certificate(typed: &str) -> Result<Cleaned, &'static str> {
    let typed = typed.trim();
    Ok(Cleaned::ok(match typed.to_ascii_lowercase().as_str() {
        "" | "n" | "no" => "no".to_owned(),
        "y" | "yes" => "yes".to_owned(),
        // --bridge-cert's path, which is never taken for a SHA-256.
        _ if typed.starts_with("file:") => typed.to_owned(),
        _ => match typed.parse::<CertPin>() {
            Ok(pin) => format!("sha256:{pin}"),
            Err(_) => format!("file:{typed}"),
        },
    }))
}

fn clean_password(typed: &str) -> Result<Cleaned, &'static str> {
    if typed.trim().is_empty() {
        return Err("The Bridge password is empty.");
    }
    Ok(Cleaned::ok(typed.trim()))
}

/// The flags `account add proton` takes.
#[derive(Debug, Default)]
pub struct Args {
    pub name: Option<String>,
    pub username: Option<String>,
    pub port: Option<u16>,
    pub security: Option<Security>,
    pub smtp_port: Option<u16>,
    pub smtp_security: Option<Security>,
    pub cert_sha256: Option<CertPin>,
    /// The cert.pem Bridge's export wrote, compared with the certificate Bridge presents.
    pub bridge_cert: Option<PathBuf>,
    pub password_command: Option<String>,
    pub no_check: bool,
}

/// What reaches outside the process besides Bridge's ports: the keyring, and where Bridge's
/// program is.
pub struct Deps<'a> {
    pub open_keyring: &'a dyn Fn() -> Result<Keyring>,
    /// Bridge's program on PATH, if any: its absence only warns.
    pub find_bridge: &'a dyn Fn() -> Option<PathBuf>,
}

/// Bridge's address: it listens on this computer only.
const HOST: &str = "127.0.0.1";

/// The names Bridge's program goes by: Proton's packages and Arch's GUI package, and Arch's
/// package without the window.
const PROGRAMS: &[&str] = &["protonmail-bridge", "protonmail-bridge-core"];

fn find_bridge() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| PROGRAMS.iter().map(move |p| dir.join(p)))
        .find(|p| p.is_file())
}

/// `neomail account add proton`.
pub fn add(
    config_path: &Path,
    args: Args,
    input: Input,
    paint: crate::style::Paint,
    out: &mut impl Write,
) -> Result<()> {
    let deps = Deps {
        open_keyring: &Keyring::open,
        find_bridge: &find_bridge,
    };
    let mut tty = Tty::new(input, "password");
    let mut ui = Ui::real(&mut tty, paint, false, PROTON.id);
    run(config_path, args, input, &mut ui, &deps, out)
}

/// The certificates Bridge presented, or the ones pinned: Bridge has one for both ports, but a
/// different SMTP one is pinned on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Certs {
    imap: CertPin,
    smtp: CertPin,
}

/// The wizard with `ui` for the user and `deps` for the outside.
pub fn run(
    config_path: &Path,
    args: Args,
    input: Input,
    ui: &mut Ui<'_>,
    deps: &Deps<'_>,
    out: &mut impl Write,
) -> Result<()> {
    if args.no_check && args.cert_sha256.is_none() && args.bridge_cert.is_none() {
        return Err(crate::usage(
            "--no-check does not connect, so the certificate cannot be read: pass --cert-sha256 \
             or --bridge-cert too",
        ));
    }
    let security_flag = |s: Option<Security>| s.map(|s| security_name(s).to_owned());
    let flag = |name: &str| match name {
        "--name" => args.name.clone(),
        "--username" => args.username.clone(),
        "--port" => args.port.map(|p| p.to_string()),
        "--security" => security_flag(args.security),
        "--smtp-port" => args.smtp_port.map(|p| p.to_string()),
        "--smtp-security" => security_flag(args.smtp_security),
        "--cert-sha256" => args.cert_sha256.map(|p| p.to_string()).or_else(|| {
            let path = args.bridge_cert.as_ref()?;
            Some(format!("file:{}", path.to_string_lossy()))
        }),
        _ => None,
    };
    let session = match Session::new(&PROTON, ui.interactive(), flag) {
        Ok(session) => session,
        Err(StartError::Missing(flags)) => {
            let flags: Vec<&str> = flags
                .into_iter()
                .map(|f| match f {
                    "--cert-sha256" => {
                        "--cert-sha256 or --bridge-cert (the certificate cannot be confirmed \
                         here: run `neomail account add proton` on a terminal to see and confirm \
                         it)"
                    }
                    f => f,
                })
                .collect();
            return Err(secret::cannot_ask(input, &flags.join(" and ")));
        }
        Err(StartError::Bad(flag, fix)) => return Err(crate::usage(format!("{flag}: {fix}"))),
    };
    let command = args.password_command.filter(|c| !c.trim().is_empty());
    let mut session = match command {
        Some(_) => session.provided("password"),
        None => session,
    };
    let keyring: RefCell<Option<Keyring>> = RefCell::new(None);
    let presented: Cell<Option<Certs>> = Cell::new(None);
    let pinned: Cell<Option<Certs>> = Cell::new(None);
    let port = |answers: &Answers<'_>, id: &str| {
        answers
            .text(id)
            .and_then(|p| p.parse().ok())
            .unwrap_or_default()
    };
    let imap = |answers: &Answers<'_>| ImapConfig {
        host: HOST.to_owned(),
        port: port(answers, "imap-port"),
        security: security(answers.text("imap-security")),
        username: answers.text("username").unwrap_or_default().to_owned(),
        cert_pin: pinned.get().map(|c| c.imap),
    };
    // Bridge keeps a copy of sent mail itself, so neomail appends none (config::preset_smtp).
    let smtp = |answers: &Answers<'_>| SmtpSettings {
        host: HOST.to_owned(),
        port: port(answers, "smtp-port"),
        security: security(answers.text("smtp-security")),
        cert_pin: pinned.get().map(|c| c.smtp),
        save_sent: false,
    };
    let block = |answers: &Answers<'_>| {
        config::password_account_toml(
            answers.text("name").unwrap_or_default(),
            &imap(answers),
            command.as_deref(),
            Some(&smtp(answers)),
        )
    };
    let check =
        |id: &str, answers: &Answers<'_>, _: &mut cli::Ask<'_, '_>| -> Result<String, CheckError> {
            match id {
                "name-free" => {
                    let name = answers.text("name").unwrap_or_default();
                    if name_taken(config_path, name) {
                        return Err(CheckError {
                            kind: NAME_TAKEN,
                            detail: format!(
                                "{} already has an account named {name:?}.",
                                config_path.display()
                            ),
                        });
                    }
                    Ok(String::new())
                }
                "bridge" if !args.no_check => {
                    let (certs, said) = bridge_ready(answers, deps)?;
                    presented.set(Some(certs));
                    Ok(said)
                }
                "pin" => {
                    let answer = answers.text("certificate").unwrap_or("no");
                    let (certs, said) = pin(answer, presented.get())?;
                    pinned.set(Some(certs));
                    Ok(said)
                }
                "ready" => {
                    let name = answers.text("name").unwrap_or_default();
                    config::check_append(config_path, name, &block(answers)).map_err(other)?;
                    if command.is_none() {
                        *keyring.borrow_mut() = Some((deps.open_keyring)().map_err(other)?);
                    }
                    Ok(String::new())
                }
                "login" if !args.no_check => {
                    let password = match &command {
                        Some(c) => secret::run_password_command(c).map_err(other)?,
                        None => match answers.secret("password") {
                            Some(p) => Password::new(p.expose().to_owned()),
                            None => return Err(other(anyhow::anyhow!("no password was given"))),
                        },
                    };
                    check_login(&imap(answers), &smtp(answers), &password).map_err(|mut e| {
                        if e.kind == REFUSED && command.is_some() {
                            e.detail.push_str(
                                "\nThe password came from --password-command: it must print the \
                             password Bridge shows.",
                            );
                        }
                        e
                    })
                }
                "bridge" | "login" => Ok(cli::SKIPPED.to_owned()),
                _ => Ok(String::new()),
            }
        };
    let write = |answers: &Answers<'_>| -> Result<Saved> {
        let name = answers.text("name").unwrap_or_default();
        let stored = match (keyring.borrow().as_ref(), answers.secret("password")) {
            (Some(keyring), Some(password)) => {
                keyring.set(name, password)?;
                format!(
                    "Its Bridge password is in {} (service {:?}, username {name:?}).",
                    keyring.name(),
                    keyring::SERVICE
                )
            }
            _ => "It runs its password_command for the Bridge password.".to_owned(),
        };
        config::append_account(config_path, name, &block(answers)).with_context(|| {
            if command.is_none() {
                "the Bridge password is stored in the keyring, but the config file could not be \
                 updated"
            } else {
                "the config file could not be updated"
            }
        })?;
        writeln!(
            out,
            "Added {name:?} ({}) to {}. {stored}\nNext: neomail sync --account {name}",
            answers.text("username").unwrap_or_default(),
            config_path.display(),
        )?;
        out.flush()?;
        Ok(Saved {
            said: String::new(),
            todo: vec!["Keep Bridge running: neomail syncs and sends through it.".to_owned()],
        })
    };
    cli::drive(&mut session, ui, check, write)
}

fn security_name(security: Security) -> &'static str {
    match security {
        Security::Tls => "tls",
        Security::StartTls => "starttls",
    }
}

/// Reads the certificate on both ports, secured as answered. Says what was found, the
/// certificate's SHA-256 included, so the user can compare it before pinning it.
fn bridge_ready(answers: &Answers<'_>, deps: &Deps<'_>) -> Result<(Certs, String), CheckError> {
    let imap = presented_on(answers, Protocol::Imap, deps)?;
    let smtp = presented_on(answers, Protocol::Smtp, deps)?;
    let mut said = format!(
        "Bridge answers on its IMAP and SMTP ports. Its certificate has the SHA-256\n  {}",
        colon_hex(&imap)
    );
    if smtp != imap {
        said.push_str(&format!(
            "\nThe SMTP port presents another certificate, pinned on its own, with the \
             SHA-256\n  {}",
            colon_hex(&smtp)
        ));
    }
    Ok((Certs { imap, smtp }, said))
}

fn presented_on(
    answers: &Answers<'_>,
    protocol: Protocol,
    deps: &Deps<'_>,
) -> Result<CertPin, CheckError> {
    let (name, port_id, security_id, [refused, speaks, other]) = match protocol {
        Protocol::Imap => (
            "IMAP",
            "imap-port",
            "imap-security",
            [IMAP_REFUSED, IMAP_SPEAKS, IMAP_OTHER],
        ),
        Protocol::Smtp => (
            "SMTP",
            "smtp-port",
            "smtp-security",
            [SMTP_REFUSED, SMTP_SPEAKS, SMTP_OTHER],
        ),
    };
    let port: u16 = answers
        .text(port_id)
        .and_then(|p| p.parse().ok())
        .unwrap_or_default();
    let answered = security(answers.text(security_id));
    probe::certificate(HOST, port, protocol, answered).map_err(|e| match e {
        ProbeError::Refused => {
            let mut detail = format!(
                "Nothing answers on {HOST}:{port}, the {name} port: the connection was refused."
            );
            if (deps.find_bridge)().is_none() {
                detail.push_str(
                    "\nWarning: neomail finds neither protonmail-bridge nor protonmail-bridge-core \
                     on PATH. If Bridge is not installed, see step 2 (packages may name it \
                     otherwise).",
                );
            }
            CheckError {
                kind: refused,
                detail,
            }
        }
        ProbeError::Speaks(spoken) => CheckError {
            kind: speaks,
            detail: format!(
                "{HOST}:{port}, the {name} port, {}, but the {name} security answered is {}.",
                match spoken {
                    Security::Tls => "speaks TLS from the first byte (Bridge's SSL)",
                    Security::StartTls => "greets in plain text, for STARTTLS",
                },
                security_name(answered)
            ),
        },
        ProbeError::Other(why) => CheckError {
            kind: other,
            detail: format!("{HOST}:{port}, the {name} port: {why}."),
        },
    })
}

/// Checks the certificate answer against what Bridge presented (`None` with --no-check, when
/// the answer itself is pinned).
fn pin(answer: &str, presented: Option<Certs>) -> Result<(Certs, String), CheckError> {
    let not_pinned = || CheckError {
        kind: NOT_PINNED,
        detail: "The certificate was not pinned, and neomail does not connect to Bridge \
                 without it."
            .to_owned(),
    };
    let (expected, source, mut said) = if answer == "no" {
        return Err(not_pinned());
    } else if answer == "yes" {
        return presented.map(|p| (p, String::new())).ok_or_else(not_pinned);
    } else if let Some(hex) = answer.strip_prefix("sha256:") {
        let pin: CertPin = hex.parse().map_err(|e| other(anyhow::Error::new(e)))?;
        (pin, "The SHA-256 given is".to_owned(), String::new())
    } else {
        let path = expand_home(answer.strip_prefix("file:").unwrap_or(answer));
        let pin = pem_pin(&path).map_err(|detail| CheckError {
            kind: CERT_FILE,
            detail,
        })?;
        let key = path.with_file_name("key.pem");
        let said = if key.exists() {
            format!(
                "Delete {}: the export wrote Bridge's private key there, and nothing needs that \
                 copy.",
                key.display()
            )
        } else {
            "If the export wrote key.pem, Bridge's private key, next to it, delete it.".to_owned()
        };
        (pin, format!("{} has the SHA-256", path.display()), said)
    };
    let Some(presented) = presented else {
        return Ok((
            Certs {
                imap: expected,
                smtp: expected,
            },
            said,
        ));
    };
    if expected != presented.imap {
        return Err(CheckError {
            kind: MISMATCH,
            detail: format!(
                "{source}\n  {}\nbut Bridge presented\n  {}\nso it is not the certificate of \
                 the Bridge that answers.",
                colon_hex(&expected),
                colon_hex(&presented.imap)
            ),
        });
    }
    said.insert_str(0, "It matches the certificate Bridge presented. ");
    Ok((presented, said.trim_end().to_owned()))
}

/// `~/x` as a path under the home directory, as a shell would have it.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// The SHA-256 of the first certificate in a PEM file, as Bridge's export writes it.
fn pem_pin(path: &Path) -> Result<CertPin, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {e}.", path.display()))?;
    let body = text
        .split_once("-----BEGIN CERTIFICATE-----")
        .and_then(|(_, rest)| rest.split_once("-----END CERTIFICATE-----"))
        .map(|(body, _)| body)
        .ok_or_else(|| format!("{} holds no PEM certificate.", path.display()))?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| format!("{} holds a damaged certificate.", path.display()))?;
    Ok(CertPin::of(&der))
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
