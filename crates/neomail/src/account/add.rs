//! `neomail account add gmail` and `outlook`: walks the provider's setup wizard
//! ([`oauth::steps`]) and runs its checks. Nothing reaches the keyring or the config until every
//! check has passed, the sign-in included, with one exception: a sign-in left in the keyring under
//! that name with no account in the config is revoked (where the provider can) and deleted right
//! before the new sign-in, since revoking it afterwards would end the new sign-in too. If the
//! sign-in or a later check then fails, nothing replaces it. Everything that reaches outside the
//! process is in [`Deps`], so the tests use fakes.

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use nm_imap::smtp::{self, SmtpConfig};
use nm_imap::{ImapConfig, ImapError};

use super::{Flow, entry};
use crate::config::{self, Config};
use crate::http::Http;
use crate::keyring::{self, Keyring, OAuthEntry};
use crate::oauth::check::{self, Verdict};
use crate::oauth::steps::{self, *};
use crate::oauth::{self, ClientKind, EndpointError, Provider, Secret, SignInError, SignedIn};
use crate::secret::{self, Input};
use crate::wizard::cli::{self, Saved, Ui};
use crate::wizard::gmail::Servers;
use crate::wizard::term::Tty;
use crate::wizard::{Answers, CheckError, Session, StartError, Wizard};

/// The flags of `neomail account add gmail|outlook`.
#[derive(Debug, Default)]
pub struct AddArgs {
    pub name: Option<String>,
    pub email: Option<String>,
    pub client_id: Option<String>,
    /// gmail: the Google Cloud project, so the console links open it.
    pub project: Option<String>,
    /// outlook: `personal` or `work`.
    pub account_type: Option<String>,
    pub no_browser: bool,
    pub device_code: bool,
    /// Skip the checks the sign-in does not need: the client check before it and the
    /// read-only mail check after it.
    pub no_check: bool,
}

/// What reaches outside the process: plain function values and the HTTP client, so the tests
/// give fakes.
pub struct Deps<'a, H> {
    pub open_keyring: &'a dyn Fn() -> Result<Keyring>,
    /// Google's and Microsoft's endpoints, for the client check and a leftover's revocation.
    pub http: &'a H,
    /// The sign-in, in the browser or with a device code, for this client and address.
    pub sign_in: &'a dyn Fn(&oauth::Client, &str, Flow) -> Result<SignedIn>,
    /// Where Gmail's mail check logs in.
    pub gmail: Servers,
    /// INBOX's message count through Microsoft Graph.
    pub graph_count: &'a dyn Fn(&Secret) -> Result<u64>,
}

/// The wizard of `provider`.
pub fn wizard(provider: &Provider) -> &'static Wizard {
    match provider.client {
        ClientKind::Confidential => &steps::GMAIL,
        ClientKind::Public => &steps::OUTLOOK,
    }
}

/// `neomail account add gmail|outlook` with the real keyring, endpoints and servers.
pub fn add(
    provider: &'static Provider,
    config_path: &Path,
    args: AddArgs,
    input: Input,
    paint: crate::style::Paint,
    out: &mut impl Write,
) -> Result<()> {
    let flow = super::flow_here(provider, args.device_code)?;
    let http = crate::http::Https::new()?;
    let no_browser = args.no_browser;
    let sign_in = |client: &oauth::Client, email: &str, flow: Flow| {
        super::sign_in_as(provider, client, email, no_browser, flow)
    };
    let graph_count = |token: &Secret| super::runtime()?.block_on(crate::graph::check(token));
    let deps = Deps {
        open_keyring: &Keyring::open_for_oauth,
        http: &http,
        sign_in: &sign_in,
        gmail: Servers::gmail(),
        graph_count: &graph_count,
    };
    let mut tty = Tty::new(input, "client secret");
    let mut ui = Ui::real(&mut tty, paint, no_browser, wizard(provider).id);
    run(
        provider,
        flow,
        config_path,
        args,
        input,
        &mut ui,
        &deps,
        out,
    )
}

/// The keyring once it opened, and the sign-in it held under the account's name with no
/// account in the config.
type Opened = (Keyring, Option<(String, OAuthEntry)>);

/// The client ID in the config block that [`config::check_append`] tries before the client ID
/// is asked; the write uses the real one.
const SOME_CLIENT_ID: &str = "client-id";

/// The wizard for `provider`, with `ui` for the user and `deps` for the outside. `flow` is
/// the sign-in [`super::flow_here`] chose from `--device-code` and the environment.
#[allow(clippy::too_many_arguments)]
pub fn run<H: Http>(
    provider: &'static Provider,
    flow: Flow,
    config_path: &Path,
    args: AddArgs,
    input: Input,
    ui: &mut Ui<'_>,
    deps: &Deps<'_, H>,
    out: &mut impl Write,
) -> Result<()> {
    let wizard = wizard(provider);
    let device_code = matches!(flow, Flow::DeviceCode | Flow::AutoDeviceCode);
    let flag = |name: &str| match name {
        "--name" => args.name.clone(),
        "--email" => args.email.clone(),
        "--client-id" => args.client_id.clone(),
        "--project" => args.project.clone(),
        "--account-type" => args.account_type.clone(),
        "--device-code" => device_code.then(|| DEVICE_CODE.to_owned()),
        _ => None,
    };
    let mut session = match Session::new(wizard, ui.interactive(), flag) {
        Ok(session) => session,
        Err(StartError::Missing(flags)) => {
            let secret_note = match provider.client {
                ClientKind::Confidential => {
                    " (the client secret is then read as one line of standard input)"
                }
                ClientKind::Public => "",
            };
            return Err(secret::cannot_ask(
                input,
                &format!("{}{secret_note}", flags.join(" and ")),
            ));
        }
        Err(StartError::Bad(flag, fix)) => return Err(crate::usage(format!("{flag}: {fix}"))),
    };
    // The keyring once it opened, and the sign-in it held under the name with no account in
    // the config, to revoke right before the new sign-in.
    let keyring: RefCell<Option<Opened>> = RefCell::new(None);
    let signed_in: RefCell<Option<SignedIn>> = RefCell::new(None);
    let client = |answers: &Answers<'_>| oauth::Client {
        id: answers.text("client-id").unwrap_or_default().to_owned(),
        secret: answers
            .secret("secret")
            .map(|s| Secret::new(s.expose().to_owned())),
    };
    let check = |id: &str,
                 answers: &Answers<'_>,
                 ask: &mut cli::Ask<'_, '_>|
     -> Result<String, CheckError> {
        match id {
            "ready" => {
                let name = answers.text("name").unwrap_or_default();
                let email = answers.text("email").unwrap_or_default();
                if name_taken(config_path, name) {
                    return Err(CheckError {
                        kind: NAME_TAKEN,
                        detail: format!(
                            "{} already has an account named {name:?}.",
                            config_path.display()
                        ),
                    });
                }
                // Before the sign-in: no point signing in when it cannot be kept. On Linux,
                // opening the store connects to the Secret Service; reading the entry checks
                // that it answers.
                let store = (deps.open_keyring)().map_err(other)?;
                let leftover = match store.get_oauth(name) {
                    Err(e) if e.is::<keyring::Malformed>() => None,
                    other_result => other_result.map_err(other)?,
                };
                let block = config::oauth_account_toml(name, email, provider, SOME_CLIENT_ID);
                config::check_append(config_path, name, &block).map_err(other)?;
                *keyring.borrow_mut() = Some((store, leftover.map(|l| (name.to_owned(), l))));
                Ok(String::new())
            }
            "client-check" | "mail-check" if args.no_check => Ok(cli::SKIPPED.to_owned()),
            "client-check" => client_check(deps.http, provider, &client(answers)),
            "sign-in" => {
                let email = answers.text("email").unwrap_or_default();
                let flow = match answers.text("sign-in-method") {
                    Some(DEVICE_CODE) if flow == Flow::AutoDeviceCode => Flow::AutoDeviceCode,
                    Some(DEVICE_CODE) => Flow::DeviceCode,
                    _ => Flow::Browser,
                };
                let work = answers.text("account-type") == Some(WORK);
                // Revoked before the new sign-in, not after: Google's revocation ends every
                // token the user granted the project, the new one too.
                let leftover = keyring.borrow_mut().as_mut().and_then(|(_, l)| l.take());
                let mut said = String::new();
                if let (Some((name, leftover)), Some((store, _))) =
                    (leftover, keyring.borrow().as_ref())
                {
                    let what = format!(
                        "{} holds a sign-in named {name:?} that no account in the config uses, \
                         left by an earlier setup. Signing in again replaces it: neomail revokes it \
                         with {} where it can and deletes it.",
                        store.name(),
                        provider.name
                    );
                    match ask.confirm("Replace the old sign-in", &what, "Revoke and delete it?") {
                        Ok(Some(true)) => {}
                        Ok(None) => {
                            return Err(CheckError {
                                kind: cli::QUIT,
                                detail: "quit".into(),
                            });
                        }
                        Ok(Some(false)) => {
                            return Err(CheckError {
                                kind: KEPT_LEFTOVER,
                                detail: format!("The old sign-in named {name:?} was kept."),
                            });
                        }
                        Err(e) => return Err(other(e)),
                    }
                    said = revoke_leftover(deps.http, provider, store, &name, &leftover)?;
                    said.push('\n');
                }
                let client = client(answers);
                let done = (deps.sign_in)(&client, email, flow)
                    .map_err(|e| sign_in_failure(wizard, provider, &e, flow, work))?;
                if !done.email.eq_ignore_ascii_case(email) {
                    return Err(CheckError {
                        kind: WRONG_ACCOUNT,
                        detail: format!(
                            "You signed in as {}, not {email}; nothing was saved.",
                            done.email
                        ),
                    });
                }
                said.push_str(&format!("Signed in as {}.", done.email));
                *signed_in.borrow_mut() = Some(done);
                Ok(said)
            }
            "mail-check" => {
                let signed = signed_in.borrow();
                let signed = signed
                    .as_ref()
                    .ok_or_else(|| other(anyhow!("not signed in")))?;
                let token = signed
                    .access_token
                    .as_ref()
                    .ok_or_else(|| other(anyhow!("{} returned no access token", provider.name)))?;
                match provider.mail {
                    oauth::MailApi::Graph => {
                        let count = (deps.graph_count)(token).map_err(other)?;
                        Ok(format!(
                            "Checked {} through Microsoft Graph: its INBOX holds {count} message{}.",
                            signed.email,
                            if count == 1 { "" } else { "s" }
                        ))
                    }
                    oauth::MailApi::Imap { .. } => gmail_check(&deps.gmail, &signed.email, token),
                }
            }
            _ => Ok(String::new()),
        }
    };
    let write = |answers: &Answers<'_>| -> Result<Saved> {
        let name = answers.text("name").unwrap_or_default();
        let guard = keyring.borrow();
        let (store, _) = guard.as_ref().context("the keyring was not opened")?;
        let signed = signed_in.borrow();
        let signed = signed.as_ref().context("not signed in")?;
        let client = client(answers);
        store.set_oauth(name, &entry(&client, signed))?;
        let block = config::oauth_account_toml(name, &signed.email, provider, &client.id);
        config::append_account(config_path, name, &block).context(
            "the sign-in is stored in the keyring, but the config file could not be updated",
        )?;
        writeln!(
            out,
            "Added {name:?} ({}) to {}. Its sign-in is in {} (service {:?}, username {name:?}).\n\
             Next: neomail sync --account {name}",
            signed.email,
            config_path.display(),
            store.name(),
            keyring::OAUTH_SERVICE,
        )?;
        out.flush()?;
        Ok(Saved {
            saved: vec![
                format!(
                    "the sign-in in {} (service {:?}, username {name:?})",
                    store.name(),
                    keyring::OAUTH_SERVICE
                ),
                format!("the account {name:?} in {}", config_path.display()),
            ],
            next: Some(format!("neomail sync --account {name}")),
            said: String::new(),
            todo: Vec::new(),
        })
    };
    cli::drive(&mut session, ui, check, write)
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

/// "the step Add yourself as a test user": a step of `wizard` as the user sees it, by its
/// title, since its number depends on the steps the flags answered.
fn step_ref(wizard: &Wizard, id: &str) -> String {
    match wizard.position(id) {
        Some(i) => format!("the step {:?}", wizard.steps[i].title),
        None => id.to_owned(),
    }
}

/// Revokes a leftover sign-in with `http` and deletes it; returns the line that says so.
fn revoke_leftover<H: Http>(
    http: &H,
    provider: &Provider,
    store: &Keyring,
    name: &str,
    leftover: &OAuthEntry,
) -> Result<String, CheckError> {
    let revoke = provider
        .revoke_url
        .map(|_| |token: &Secret| super::runtime()?.block_on(oauth::revoke(http, provider, token)));
    super::clear_leftover(store, name, leftover, provider.name, revoke).map_err(other)
}

/// Asks the provider about the client before the sign-in, and maps its answer to the step to
/// change. An answer that tells nothing lets the setup go on.
fn client_check<H: Http>(
    http: &H,
    provider: &Provider,
    client: &oauth::Client,
) -> Result<String, CheckError> {
    let verdict = super::runtime()
        .and_then(|rt| rt.block_on(check::check_client(http, provider, client)))
        .map_err(|e| {
            other(e.context(format!(
                "cannot reach {} to check the client (--no-check skips this check)",
                provider.name
            )))
        })?;
    let failed = |kind, words: String| CheckError {
        kind,
        detail: format!("{} answered: {words}", provider.name),
    };
    match verdict {
        Verdict::Known => Ok(format!("{} knows this client.", provider.name)),
        Verdict::UnknownId(words) => Err(failed(UNKNOWN_ID, words)),
        Verdict::WrongSecret(words) => Err(failed(WRONG_SECRET, words)),
        Verdict::BadClient(words) => Err(failed(BAD_CLIENT, words)),
        Verdict::NotMultitenant(words) => Err(failed(NOT_MULTITENANT, words)),
        Verdict::Unclear(words) => Ok(format!(
            "neomail: {}'s answer does not tell whether the client is right ({words}); going on",
            provider.name
        )),
    }
}

/// Microsoft's codes (<https://learn.microsoft.com/en-us/entra/identity-platform/reference-error-codes>):
/// the app is single-tenant, admits work accounts only, or is not found.
const AADSTS_NOT_MULTITENANT: u64 = 50194;
const AADSTS_WORK_ONLY: u64 = 9002332;
const AADSTS_UNKNOWN: [u64; 2] = [700016, 50059];
const AADSTS_CONFIDENTIAL: u64 = 7000218;
/// No consent (65001), admin consent required (90094, 90095), user declined (65004).
const AADSTS_CONSENT: [u64; 4] = [65001, 65004, 90094, 90095];

/// Which step a failed sign-in names: from the redirect's error, the token endpoint's code, or
/// a timeout, whose likely causes Google and Microsoft show only on their own pages.
fn sign_in_failure(
    wizard: &Wizard,
    provider: &Provider,
    e: &anyhow::Error,
    flow: Flow,
    work: bool,
) -> CheckError {
    let detail = format!("{e:#}");
    let at = |id| step_ref(wizard, id);
    let failed = |kind| CheckError {
        kind,
        detail: detail.clone(),
    };
    let codes: Vec<u64> = if let Some(s) = e.downcast_ref::<SignInError>() {
        match s {
            SignInError::TimedOut(_) => {
                let causes = match (provider.client, flow) {
                    (ClientKind::Confidential, _) => format!(
                        "an address that is not a test user ({}), a client ID that is not a \
                         Desktop app client ({}), or the mail scope left out ({})",
                        at("test-user"),
                        at("client"),
                        at("scope")
                    ),
                    (ClientKind::Public, Flow::Browser) => format!(
                        "a redirect URI other than http://localhost under Mobile and desktop \
                         applications ({}, AADSTS50011), or supported account types other than \
                         Any Entra ID Tenant + Personal Microsoft accounts ({}, AADSTS50194)",
                        at("redirect"),
                        at("register")
                    ),
                    (ClientKind::Public, _) => format!(
                        "supported account types other than Any Entra ID Tenant + Personal \
                         Microsoft accounts ({}, AADSTS50194), or the code entered too late",
                        at("register")
                    ),
                };
                return CheckError {
                    kind: TIMED_OUT,
                    detail: format!(
                        "{detail}. If {}'s page showed an error, the likely causes are {causes}.",
                        provider.name
                    ),
                };
            }
            SignInError::NoMailGrant(_) => return failed(NO_MAIL),
            SignInError::Declined { .. } if provider.client == ClientKind::Confidential => {
                return failed(DECLINED);
            }
            SignInError::Redirected { error, .. } if error == "redirect_uri_mismatch" => {
                return failed(REDIRECT_MISMATCH);
            }
            SignInError::Declined { .. } | SignInError::Redirected { .. } => {
                s.aadsts().into_iter().collect()
            }
        }
    } else if let Some(endpoint) = e.downcast_ref::<EndpointError>() {
        match endpoint.code.as_str() {
            "invalid_client" if provider.client == ClientKind::Confidential => {
                return failed(BAD_CLIENT);
            }
            "redirect_uri_mismatch" => return failed(REDIRECT_MISMATCH),
            _ => endpoint.codes.clone(),
        }
    } else {
        return failed(OTHER);
    };
    let declined = matches!(
        e.downcast_ref::<SignInError>(),
        Some(SignInError::Declined { .. })
    );
    let kind = if codes.contains(&AADSTS_CONFIDENTIAL) {
        match flow {
            Flow::Browser => CONFIDENTIAL_BROWSER,
            Flow::DeviceCode | Flow::AutoDeviceCode => CONFIDENTIAL_DEVICE,
        }
    } else if codes.contains(&AADSTS_NOT_MULTITENANT) {
        NOT_MULTITENANT
    } else if codes.contains(&AADSTS_WORK_ONLY) {
        WORK_ONLY
    } else if codes.iter().any(|c| AADSTS_UNKNOWN.contains(c)) {
        UNKNOWN_ID
    } else if work && (declined || codes.iter().any(|c| AADSTS_CONSENT.contains(c))) {
        DECLINED
    } else {
        OTHER
    };
    failed(kind)
}

/// Gmail's mail check: an XOAUTH2 login to IMAP that opens INBOX read-only, and one to SMTP
/// that leaves without sending.
fn gmail_check(servers: &Servers, email: &str, token: &Secret) -> Result<String, CheckError> {
    let runtime = super::runtime().map_err(other)?;
    let login = || nm_imap::Login::XOAuth2 {
        access_token: token.expose(),
    };
    let imap = ImapConfig {
        username: email.to_owned(),
        ..servers.imap.clone()
    };
    let count = runtime
        .block_on(nm_imap::check_login(&imap, login()))
        .map_err(|e: ImapError| other(anyhow::Error::new(e).context("the IMAP check failed")))?;
    let smtp_config = SmtpConfig {
        host: servers.smtp.host.clone(),
        port: servers.smtp.port,
        security: servers.smtp.security,
        username: email.to_owned(),
        cert_pin: servers.smtp.cert_pin,
    };
    runtime
        .block_on(smtp::check_auth(&smtp_config, login()))
        .map_err(|e| other(anyhow::Error::new(e).context("the SMTP check failed")))?;
    Ok(format!(
        "Logged in to {}:{} as {email} and opened INBOX read-only: {count} messages.\n\
         Logged in to {}:{} and left without sending anything.",
        imap.host, imap.port, servers.smtp.host, servers.smtp.port
    ))
}
