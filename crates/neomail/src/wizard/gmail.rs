//! `neomail account add gmail-app-password`: Gmail over IMAP and SMTP with an app password
//! (issue #49). Google's facts come from <https://support.google.com/accounts/answer/185833>
//! unless another page is linked.

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use nm_imap::smtp::{self, SmtpConfig};
use nm_imap::{ImapConfig, ImapError, Security};

use super::cli::{self, Term};
use super::{
    Answers, Case, CheckError, Cleaned, Kind, Mistake, Question, Session, StartError, Step, Wizard,
};
use crate::config::{self, Config, SmtpSettings};
use crate::keyring::{self, Keyring};
use crate::secret::{self, Input, Password};

const APP_PASSWORDS_HELP: &str = "https://support.google.com/accounts/answer/185833";
const TWO_STEP_PAGE: &str = "https://myaccount.google.com/signinoptions/two-step-verification";

/// What a refused login can mean. Repeated at the password step, since the eligibility step is
/// skipped when nothing can be asked.
const REFUSED_FIX: &str = "\
Gmail refused this address and app password. Paste the app password from step 3, not your Google
Account password, and check the address (step 6; on a terminal, type < to go back to it). Google
revokes app passwords when the Google Account password changes. App passwords need 2-Step
Verification, and Google may offer none when it is set up only for security keys,
for a work, school or other organisation account (its administrator may not allow them),
or with Advanced Protection (https://support.google.com/accounts/answer/185833).";

pub static GMAIL_APP_PASSWORD: Wizard = Wizard {
    id: "gmail-app-password",
    steps: &[
        Step {
            id: "eligible",
            title: "Can this account have an app password?",
            why: "\
Google has app passwords only for accounts with 2-Step Verification on, and even then may offer
none in the cases 2 to 4 below. Pick the one that fits.",
            links: &[APP_PASSWORDS_HELP],
            kind: Kind::Gate(&[
                Case {
                    label: "None of these: a personal account, with 2-Step Verification on or \
                            about to be",
                    stop: None,
                },
                Case {
                    label: "2-Step Verification is set up only for security keys",
                    stop: Some(
                        "Google may offer no app password when 2-Step Verification is set up \
                         only for security keys (https://support.google.com/accounts/answer/185833). Add \
                         another second step, such as Google prompts or an authenticator app, at \
                         https://myaccount.google.com/signinoptions/two-step-verification, then \
                         run `neomail account add gmail-app-password` again.",
                    ),
                },
                // Google Workspace users can sign in to legacy apps with an app password, but
                // not where the administrator requires security keys
                // (https://knowledge.workspace.google.com/admin/security/how-2-step-verification-works-with-legacy-apps),
                // so this case goes on and step 3 says what to do if the option is missing.
                Case {
                    label: "A work, school or other organisation account: its administrator may \
                            not allow app passwords. If \
                            https://myaccount.google.com/apppasswords has no option to add one, \
                            ask them or use `neomail account add gmail`",
                    stop: None,
                },
                Case {
                    label: "Advanced Protection is on",
                    stop: Some(
                        "With Advanced Protection, apps that use app passwords are blocked, and \
                         only verified third-party apps can reach the account's data \
                         (https://support.google.com/accounts/answer/7539956). neomail has no way \
                         to read this account.",
                    ),
                },
            ]),
        },
        Step {
            id: "two-step",
            title: "Turn on 2-Step Verification",
            why: "\
Skip this if it is on. Google: \"App passwords can only be used with accounts that have 2-Step
Verification turned on.\" Use a second step other than security keys alone.",
            links: &[
                TWO_STEP_PAGE,
                "https://support.google.com/accounts/answer/185839",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "create",
            title: "Create an app password",
            why: "\
Name it (\"neomail\", say), create it and copy it. It is what neomail logs in with, so your
Google Account password never reaches neomail. If the page has no option to add one, Google may
offer none for this account: with security keys only, add another second step; for a work,
school or other organisation account, its administrator may not allow app passwords, so ask them or
use `neomail account add gmail`.",
            links: &[
                "https://myaccount.google.com/apppasswords",
                APP_PASSWORDS_HELP,
            ],
            kind: Kind::Console,
        },
        Step {
            id: "name",
            title: "Name the account",
            why: "What neomail calls it, as in `neomail sync --account NAME`.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--name"),
                prompt: "Account name in neomail: ",
                secret: false,
                default: Some("gmail"),
                clean: clean_name,
            }),
        },
        Step {
            id: "name-free",
            title: "Check that the name is free",
            why: "No account in the config has this name yet.",
            links: &[],
            kind: Kind::Check { reads: &["name"] },
        },
        Step {
            id: "address",
            title: "Your Gmail address",
            why: "IMAP and SMTP log in with the address and the app password.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--username"),
                prompt: "Gmail address: ",
                secret: false,
                default: None,
                clean: clean_address,
            }),
        },
        Step {
            id: "ready",
            title: "Check that the account can be saved",
            why: "The config can take the account and the keyring opens, before the password \
                  is asked.",
            links: &[],
            kind: Kind::Check {
                reads: &["name", "address"],
            },
        },
        Step {
            id: "password",
            title: "Paste the app password",
            why: "The app password from step 3, not your Google Account password. Spaces are \
                  removed.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: None,
                prompt: "App password (not shown): ",
                secret: true,
                default: None,
                clean: clean_app_password,
            }),
        },
        Step {
            id: "login",
            title: "Check the login",
            why: "neomail logs in to Gmail's IMAP, reading nothing, and to its SMTP, sending \
                  nothing, before anything is written.",
            links: &[],
            kind: Kind::Check {
                reads: &["address", "password"],
            },
        },
        Step {
            id: "write",
            title: "Save the account",
            why: "The app password goes to the keyring and the account to the config.",
            links: &[],
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
            fix: REFUSED_FIX,
        },
    ],
};

const NAME_TAKEN: &str = "name-taken";
const REFUSED: &str = "refused";
const OTHER: &str = "other";

fn clean_name(typed: &str) -> Result<Cleaned, &'static str> {
    let name = typed.trim();
    if name.is_empty() {
        return Err("The account name is empty.");
    }
    Ok(Cleaned::ok(name))
}

/// An address: something on each side of one `@`, and no spaces.
fn clean_address(typed: &str) -> Result<Cleaned, &'static str> {
    let address = typed.trim();
    match address.split_once('@') {
        Some((local, domain))
            if !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !address.contains(char::is_whitespace) =>
        {
            Ok(Cleaned::ok(address))
        }
        _ => Err("That is not an address: give the whole Gmail address, such as you@gmail.com."),
    }
}

/// Removes spaces and no-break spaces, which a copied app password can carry between its groups.
/// Google calls it "a 16-digit passcode" (support.google.com/accounts/answer/185833), so a value
/// that is not 16 letters or digits warns but still goes to the login check.
fn clean_app_password(typed: &str) -> Result<Cleaned, &'static str> {
    let value: String = typed
        .chars()
        .filter(|&c| c != ' ' && c != '\u{a0}')
        .collect();
    if value.is_empty() {
        return Err("The app password is empty.");
    }
    let warning = (value.chars().count() != 16
        || !value.chars().all(|c| c.is_ascii_alphanumeric()))
    .then_some(
        "that is not 16 letters or digits, as Google's app passwords are \
         (https://support.google.com/accounts/answer/185833). If the login fails, paste the app \
         password, not your Google Account password.",
    );
    Ok(Cleaned { value, warning })
}

/// The flags `account add gmail-app-password` takes.
#[derive(Debug, Default)]
pub struct Args {
    pub name: Option<String>,
    pub username: Option<String>,
    pub password_command: Option<String>,
    pub no_check: bool,
}

/// Where the login checks go and what the config gets: Gmail's servers, or fakes in tests.
#[derive(Debug, Clone)]
pub struct Servers {
    /// Its `username` is replaced by the address.
    pub imap: ImapConfig,
    pub smtp: SmtpSettings,
}

impl Servers {
    /// imap.gmail.com 993 and smtp.gmail.com 465, both TLS
    /// (<https://developers.google.com/workspace/gmail/imap/imap-smtp>).
    pub fn gmail() -> Self {
        let imap = ImapConfig {
            host: "imap.gmail.com".to_owned(),
            port: 993,
            security: Security::Tls,
            username: String::new(),
            cert_pin: None,
        };
        // As config::preset_smtp has it: Gmail keeps its own copy in Sent.
        let smtp = SmtpSettings {
            host: "smtp.gmail.com".to_owned(),
            port: 465,
            security: Security::Tls,
            cert_pin: None,
            save_sent: false,
        };
        Self { imap, smtp }
    }
}

/// What reaches outside the process besides the servers: the keyring.
pub struct Deps<'a> {
    pub open_keyring: &'a dyn Fn() -> Result<Keyring>,
    pub servers: Servers,
}

/// `neomail account add gmail-app-password`.
pub fn add(config_path: &Path, args: Args, input: Input, out: &mut impl Write) -> Result<()> {
    let deps = Deps {
        open_keyring: &Keyring::open,
        servers: Servers::gmail(),
    };
    run(config_path, args, input, &mut cli::Stdio(input), &deps, out)
}

/// The wizard with `term` for the user and `deps` for the outside.
pub fn run(
    config_path: &Path,
    args: Args,
    input: Input,
    term: &mut dyn Term,
    deps: &Deps<'_>,
    out: &mut impl Write,
) -> Result<()> {
    let flag = |name: &str| match name {
        "--name" => args.name.clone(),
        "--username" => args.username.clone(),
        _ => None,
    };
    let session = match Session::new(&GMAIL_APP_PASSWORD, term.interactive(), flag) {
        Ok(session) => session,
        Err(StartError::Missing(flags)) => {
            return Err(secret::cannot_ask(input, &flags.join(" and ")));
        }
        Err(StartError::Bad(flag, fix)) => return Err(crate::usage(format!("{flag}: {fix}"))),
    };
    let address_flag = args.username.is_some();
    let command = args.password_command.filter(|c| !c.trim().is_empty());
    let mut session = match command {
        Some(_) => session.provided("password"),
        None => session,
    };
    let keyring: RefCell<Option<Keyring>> = RefCell::new(None);
    let imap = |answers: &Answers<'_>| ImapConfig {
        username: answers.text("address").unwrap_or_default().to_owned(),
        ..deps.servers.imap.clone()
    };
    let block = |answers: &Answers<'_>| {
        config::password_account_toml(
            answers.text("name").unwrap_or_default(),
            &imap(answers),
            command.as_deref(),
            Some(&deps.servers.smtp),
        )
    };
    let check = |id: &str, answers: &Answers<'_>| -> Result<String, CheckError> {
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
                        None => return Err(other(anyhow::anyhow!("no app password was given"))),
                    },
                };
                check_login(&imap(answers), &deps.servers.smtp, &password).map_err(|mut e| {
                    if e.kind == REFUSED {
                        e.detail
                            .push_str(&refused_from_flags(address_flag, command.is_some()));
                    }
                    e
                })
            }
            _ => Ok(String::new()),
        }
    };
    let write = |answers: &Answers<'_>| -> Result<String> {
        let name = answers.text("name").unwrap_or_default();
        let stored = match (keyring.borrow().as_ref(), answers.secret("password")) {
            (Some(keyring), Some(password)) => {
                keyring.set(name, password)?;
                format!(
                    "Its app password is in {} (service {:?}, username {name:?}).",
                    keyring.name(),
                    keyring::SERVICE
                )
            }
            _ => "It runs its password_command for the app password.".to_owned(),
        };
        config::append_account(config_path, name, &block(answers)).with_context(|| {
            if command.is_none() {
                "the app password is stored in the keyring, but the config file could not be updated"
            } else {
                "the config file could not be updated"
            }
        })?;
        writeln!(
            out,
            "Added {name:?} ({}) to {}. {stored}\nNext: neomail sync --account {name}",
            answers.text("address").unwrap_or_default(),
            config_path.display(),
        )?;
        Ok(format!(
            "Google revokes app passwords when the Google Account password changes ({APP_PASSWORDS_HELP}): \
             then create a new one and run `neomail account set-password {name}`."
        ))
    };
    cli::drive(&mut session, term, check, write)
}

/// What a refused login adds when the address or the app password came from a flag, which the
/// wizard cannot ask again: run it again with another value.
fn refused_from_flags(address_flag: bool, command: bool) -> String {
    let mut text = String::new();
    if command {
        text.push_str(
            "\nThe app password came from --password-command: it must print the app password \
             without spaces, as they are not removed from it.",
        );
    }
    if address_flag {
        text.push_str("\nThe address came from --username: run the command again to change it.");
    }
    text
}

fn other(e: anyhow::Error) -> CheckError {
    CheckError {
        kind: OTHER,
        detail: format!("{e:#}"),
    }
}

/// Whether the config at `path` parses and already has an account named `name`. A config that
/// does not parse is [`config::check_append`]'s to report.
fn name_taken(path: &Path, name: &str) -> bool {
    config::exists(path) && Config::load(path).is_ok_and(|c| c.find(name).is_some())
}

/// Logs in to IMAP (EXAMINE INBOX, read-only) and SMTP (AUTH, then QUIT): a refused login on
/// either is [`REFUSED`].
fn check_login(
    imap: &ImapConfig,
    smtp: &SmtpSettings,
    password: &Password,
) -> Result<String, CheckError> {
    let runtime = crate::account::runtime().map_err(other)?;
    let login = || nm_imap::Login::Password(password.expose());
    let refused = |detail: String| CheckError {
        kind: REFUSED,
        detail,
    };
    let count = runtime
        .block_on(nm_imap::check_login(imap, login()))
        .map_err(|e| match e {
            ImapError::Login { .. } => refused(format!("{:#}", anyhow::Error::new(e))),
            e => other(anyhow::Error::new(e).context("the IMAP check failed")),
        })?;
    let smtp_config = SmtpConfig {
        host: smtp.host.clone(),
        port: smtp.port,
        security: smtp.security,
        username: imap.username.clone(),
        cert_pin: smtp.cert_pin,
    };
    runtime
        .block_on(smtp::check_auth(&smtp_config, login()))
        .map_err(|e| match e {
            smtp::SmtpError::Auth { .. } => refused(e.to_string()),
            e => other(anyhow::Error::new(e).context("the SMTP check failed")),
        })?;
    Ok(format!(
        "Logged in to {}:{} as {} and opened INBOX read-only: {count} messages.\n\
         Logged in to {}:{} and left without sending anything.",
        imap.host, imap.port, imap.username, smtp.host, smtp.port
    ))
}
