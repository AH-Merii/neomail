//! `neomail account add imap` on a terminal: the same questions as the flags, on the wizard
//! model, so they look and behave like the other setups. Off a terminal and with `--no-input`,
//! [`crate::setup`] runs from the flags as before; this shares its plan, checks and save.

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use nm_imap::Security;

use super::cli::{self, Saved, Ui};
use super::gmail::{OTHER, clean_name, name_taken};
use super::{
    Answers, CheckError, Choice, Cleaned, Kind, Mistake, Question, Session, StartError, Step,
    Wizard,
};
use crate::keyring::Keyring;
use crate::secret::{Input, Password};
use crate::setup::{self, Args, Deps, Plan, Preset};

const NAME_TAKEN: &str = "name-taken";
const REFUSED: &str = "refused";

pub static IMAP: Wizard = Wizard {
    id: "imap",
    title: "an account on any IMAP server, with a password",
    steps: &[
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
                default: None,
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
            title: "Your username",
            why: "What the server logs in with: often your whole address, such as you@example.com.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--username"),
                prompt: "Username: ",
                secret: false,
                default: None,
                choices: &[],
                clean: clean_word,
            }),
        },
        Step {
            id: "host",
            title: "The IMAP server",
            why: "\
Its host name, such as imap.example.com, from your provider's help for mail apps.
Note: --port picks a port other than the security's usual one, 993 or 143.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--host"),
                prompt: "IMAP server: ",
                secret: false,
                default: None,
                choices: &[],
                clean: clean_host,
            }),
        },
        Step {
            id: "security",
            title: "How the connection is secured",
            why: "Most servers take TLS from the first byte on port 993; some want STARTTLS on 143.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: Some("--security"),
                prompt: "Security: ",
                secret: false,
                default: Some("tls"),
                choices: &[
                    Choice {
                        value: "tls",
                        label: "TLS from the first byte (port 993)",
                    },
                    Choice {
                        value: "starttls",
                        label: "STARTTLS (port 143)",
                    },
                ],
                clean: clean_security,
            }),
        },
        Step {
            id: "ready",
            title: "Check that the account can be saved",
            why: "The config can take the account and the keyring opens, before the password \
                  is asked.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &["name", "username", "host", "security"],
            },
        },
        Step {
            id: "password",
            title: "Type the password",
            why: "The password the server logs in with. It goes to the keyring, never the config.",
            links: &[],
            page: None,
            kind: Kind::Ask(Question {
                flag: None,
                prompt: "Password (not shown): ",
                secret: true,
                default: None,
                choices: &[],
                clean: clean_password,
            }),
        },
        Step {
            id: "login",
            title: "Check the login",
            why: "neomail logs in and opens INBOX read-only before anything is written.",
            links: &[],
            page: None,
            kind: Kind::Check {
                reads: &["username", "host", "security", "password"],
            },
        },
        Step {
            id: "write",
            title: "Save the account",
            why: "The password goes to the keyring and the account to the config.",
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
            check: "login",
            kind: REFUSED,
            step: "password",
            fix: "Check the password, or press Esc to go back and change the username or server.",
        },
    ],
};

fn clean_word(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim() {
        "" => Err("The username is empty."),
        w if w.contains(char::is_whitespace) => Err("A username has no spaces."),
        w => Ok(Cleaned::ok(w)),
    }
}

fn clean_host(typed: &str) -> Result<Cleaned, &'static str> {
    let host = typed.trim();
    if host.is_empty()
        || host.contains(char::is_whitespace)
        || host.contains("://")
        || host.contains('/')
    {
        return Err("Give the server's host name alone, such as imap.example.com.");
    }
    Ok(Cleaned::ok(host))
}

fn clean_security(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim().to_ascii_lowercase().as_str() {
        "tls" | "ssl" => Ok(Cleaned::ok("tls")),
        "starttls" => Ok(Cleaned::ok("starttls")),
        _ => Err("Pick tls or starttls."),
    }
}

fn clean_password(typed: &str) -> Result<Cleaned, &'static str> {
    if typed.is_empty() {
        return Err("The password is empty.");
    }
    Ok(Cleaned::ok(typed))
}

/// The IMAP wizard with `ui` for the user; `deps` opens the keyring and checks the login. The
/// flags given answer their steps, and the rest of `args` (port, SMTP, password command, pin)
/// goes into the plan as [`setup::gather`] makes it.
pub fn run(
    preset: &'static Preset,
    config_path: &Path,
    args: Args,
    ui: &mut Ui<'_>,
    deps: &Deps<'_>,
    out: &mut impl Write,
) -> Result<()> {
    let security_flag = args.security.map(|s| match s {
        Security::Tls => "tls".to_owned(),
        Security::StartTls => "starttls".to_owned(),
    });
    let flag = |name: &str| match name {
        "--name" => args.name.clone(),
        "--username" => args.username.clone(),
        "--host" => args.host.clone(),
        "--security" => security_flag.clone(),
        _ => None,
    };
    let session = match Session::new(&IMAP, ui.interactive(), flag) {
        Ok(session) => session,
        Err(StartError::Missing(flags)) => anyhow::bail!("missing {}", flags.join(" and ")),
        Err(StartError::Bad(flag, fix)) => return Err(crate::usage(format!("{flag}: {fix}"))),
    };
    let command = args
        .password_command
        .clone()
        .filter(|c| !c.trim().is_empty());
    let mut session = match command {
        Some(_) => session.provided("password"),
        None => session,
    };
    let plan: RefCell<Option<Plan>> = RefCell::new(None);
    let keyring: RefCell<Option<Keyring>> = RefCell::new(None);
    let password: RefCell<Option<Password>> = RefCell::new(None);
    let other = |e: anyhow::Error| CheckError {
        kind: OTHER,
        detail: format!("{e:#}"),
    };
    let check = |id: &str,
                 answers: &Answers<'_>,
                 _: &mut cli::Ask<'_, '_>|
     -> Result<String, CheckError> {
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
            "ready" => {
                let security = match answers.text("security") {
                    Some("starttls") => Security::StartTls,
                    _ => Security::Tls,
                };
                let given = Args {
                    name: answers.text("name").map(str::to_owned),
                    username: answers.text("username").map(str::to_owned),
                    host: answers.text("host").map(str::to_owned),
                    security: Some(security),
                    port: args.port,
                    password_command: command.clone(),
                    cert_sha256: args.cert_sha256,
                    no_check: args.no_check,
                    smtp_host: args.smtp_host.clone(),
                    smtp_port: args.smtp_port,
                    smtp_security: args.smtp_security,
                };
                let flags_only = Input {
                    terminal: false,
                    no_input: true,
                };
                let made = setup::gather(preset, config_path, given, flags_only).map_err(other)?;
                crate::config::check_append(config_path, &made.name, &setup::block(&made))
                    .map_err(other)?;
                if command.is_none() {
                    *keyring.borrow_mut() = Some((deps.open_keyring)().map_err(other)?);
                }
                *plan.borrow_mut() = Some(made);
                Ok(String::new())
            }
            "login" => {
                let guard = plan.borrow();
                let made = guard
                    .as_ref()
                    .ok_or_else(|| other(anyhow::anyhow!("no plan")))?;
                if !made.check && command.is_some() {
                    return Ok(cli::SKIPPED.to_owned());
                }
                let secret = match &command {
                    Some(c) => crate::secret::run_password_command(c).map_err(other)?,
                    None => match answers.secret("password") {
                        Some(p) => Password::new(p.expose().to_owned()),
                        None => return Err(other(anyhow::anyhow!("no password was given"))),
                    },
                };
                let said = if made.check {
                    let count =
                        (deps.check_login)(&made.imap, &secret).map_err(|e| CheckError {
                            kind: REFUSED,
                            detail: format!("{e:#}"),
                        })?;
                    format!(
                        "Logged in to {}:{} as {} and opened INBOX read-only: {count} messages.",
                        made.imap.host, made.imap.port, made.imap.username
                    )
                } else {
                    cli::SKIPPED.to_owned()
                };
                *password.borrow_mut() = command.is_none().then_some(secret);
                Ok(said)
            }
            _ => Ok(String::new()),
        }
    };
    let write = |_: &Answers<'_>| -> Result<Saved> {
        let guard = plan.borrow();
        let made = guard.as_ref().ok_or_else(|| anyhow::anyhow!("no plan"))?;
        let place = setup::save(
            made,
            keyring.borrow().as_ref(),
            password.borrow().as_ref(),
            config_path,
            out,
        )?;
        let mut saved: Vec<String> = place
            .map(|p| format!("the password in {p}"))
            .into_iter()
            .collect();
        saved.push(format!(
            "the account {:?} in {}",
            made.name,
            config_path.display()
        ));
        Ok(Saved {
            saved,
            next: Some(format!("neomail sync --account {}", made.name)),
            said: String::new(),
            todo: Vec::new(),
        })
    };
    cli::drive(&mut session, ui, check, write)
}
