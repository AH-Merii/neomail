//! `neomail account add`, `login` and `logout`: OAuth accounts through the user's own client.
//!
//! Keyring calls stay outside the async runtime (zbus starts one of its own); only the sign-in,
//! the token refresh and the revocation run inside it.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::config::{self, Account, Login, OAuthSettings};
use crate::keyring::{self, Keyring, OAuthEntry};
use crate::oauth::http::Https;
use crate::oauth::{self, Provider, Secret, SignedIn};
use crate::{browser, secret};

const GUIDE: &str = "https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md";

/// The one-time setup, shown before the wizard asks for anything. The why of each step is in
/// the guide.
const GMAIL_STEPS: &str = "\
Gmail signs in through your own Google OAuth client: neomail ships none, so no one else's app
is ever granted access to your mail. Set one up once (about 5 minutes; why each step exists:
GUIDE):

 1. Create a Google Cloud project: https://console.cloud.google.com/projectcreate
    Why: a client belongs to a project; its own keeps it apart from anything else. No billing.
 2. Enable the Gmail API: https://console.cloud.google.com/apis/library/gmail.googleapis.com
    Why: so the Gmail scope is offered in step 4 (a later neomail also uses the API).
 3. Google Auth Platform > Branding > Get started: an app name, your address, Audience
    External, then Create. Then Audience > Test users: add your Gmail address.
    Why: External lets a personal Gmail account sign in; in Testing only test users can.
 4. Data Access > Add or remove scopes: add https://mail.google.com/ and save.
    Why: it is the scope Gmail's IMAP requires.
 5. Clients > Create client > Application type: Desktop app. Copy its client ID and its
    client secret now: Google shows the secret only once. Lost it? Open the client and click
    Add secret.
    Why: a Desktop app client may send the sign-in back to http://127.0.0.1 on any port,
    which is where neomail waits for it.

At sign-in Google warns that it hasn't verified the app: it is yours, so click Continue.
While the app's publishing status is Testing, Google ends the sign-in after 7 days and you
run `neomail account login NAME` again. To avoid that, publish it (Audience > Publish app): an
app for your own use (under 100 users) needs no verification.
";

pub struct AddArgs {
    pub name: Option<String>,
    pub email: Option<String>,
    pub client_id: Option<String>,
    pub no_browser: bool,
}

/// `neomail account add gmail`: asks for what is missing, signs in, stores the sign-in in the
/// keyring, then appends the account to the config file.
pub fn add(
    provider: &'static Provider,
    config_path: &Path,
    args: AddArgs,
    out: &mut impl Write,
) -> Result<()> {
    let interactive = io::stdin().is_terminal();
    if !interactive {
        let missing: Vec<&str> = [
            ("--email", args.email.is_none()),
            ("--client-id", args.client_id.is_none()),
        ]
        .into_iter()
        .filter_map(|(flag, missing)| missing.then_some(flag))
        .collect();
        if !missing.is_empty() {
            bail!(
                "standard input is not a terminal, so nothing can be asked: pass {} (the client \
                 secret is then read as one line of standard input)",
                missing.join(" and ")
            );
        }
    } else {
        eprintln!("{}", GMAIL_STEPS.replace("GUIDE", GUIDE));
    }

    let name = match args.name {
        Some(name) => name.trim().to_owned(),
        None if interactive => {
            let typed = secret::prompt_line("Account name in neomail [gmail]: ")?;
            if typed.is_empty() {
                "gmail".to_owned()
            } else {
                typed
            }
        }
        None => "gmail".to_owned(),
    };
    if name.is_empty() {
        bail!("the account name is empty");
    }
    config::check_can_add(config_path, &name)?;
    let email = match args.email {
        Some(email) => email.trim().to_owned(),
        None => secret::prompt_line("Gmail address: ")?,
    };
    if !email.contains('@') {
        bail!("{email:?} is not an email address");
    }
    let client_id = match args.client_id {
        Some(id) => id.trim().to_owned(),
        None => secret::prompt_line("Client ID (from step 5): ")?,
    };
    if client_id.is_empty() {
        bail!("the client ID is empty");
    }
    let client_secret =
        secret::read_secret("Client secret (from step 5, not shown): ", "client secret")?;
    // Before the browser: no point signing in when the sign-in cannot be kept. Opening the
    // store does not reach it yet; reading an entry does. The append is tried with the address
    // given, which the sign-in must match.
    let keyring = Keyring::open_for_oauth()?;
    let leftover = match keyring.get_oauth(&name) {
        Err(e) if e.is::<keyring::Malformed>() => None,
        other => other?,
    };
    config::check_append(
        config_path,
        &name,
        &config::oauth_account_toml(&name, &email, provider, &client_id),
    )?;
    if let Some(leftover) = leftover {
        revoke_leftover(provider, &keyring, &name, &leftover)?;
    }
    let client = oauth::Client {
        id: client_id,
        secret: Secret::new(client_secret),
    };

    let signed_in = sign_in(provider, &client, &email, args.no_browser)?;
    keyring.set_oauth(&name, &entry(&client, &signed_in))?;
    let block = config::oauth_account_toml(&name, &signed_in.email, provider, &client.id);
    config::append_account(config_path, &name, &block).context(
        "the sign-in is stored in the keyring, but the config file could not be updated",
    )?;
    writeln!(
        out,
        "Added {name:?} ({}) to {}. Its sign-in is in {} (service {:?}, user {name:?}).\n\
         Next: neomail sync --account {name}",
        signed_in.email,
        config_path.display(),
        keyring.name(),
        keyring::OAUTH_SERVICE,
    )?;
    Ok(())
}

/// A sign-in left in the keyring under `name` with no account in the config (a config edited by
/// hand, or an `add` whose append failed). It is revoked before the new sign-in, not after:
/// Google's revocation invalidates every token the user granted the project, the new one too
/// (<https://developers.google.com/identity/protocols/oauth2/native-app#tokenrevoke>).
fn revoke_leftover(
    provider: &Provider,
    keyring: &Keyring,
    name: &str,
    leftover: &OAuthEntry,
) -> Result<()> {
    if let Some(token) = leftover.refresh_token() {
        let result = Https::new()
            .and_then(|http| runtime()?.block_on(oauth::revoke(&http, provider, &token)));
        if let Err(e) = result {
            eprintln!(
                "neomail: warning: {} did not revoke the earlier sign-in for {name:?} ({e:#})",
                provider.name
            );
        }
    }
    keyring.delete_oauth(name)?;
    eprintln!(
        "neomail: {} held an earlier sign-in for {name:?} with no account in the config; \
         revoked and deleted it",
        keyring.name()
    );
    Ok(())
}

/// `neomail account login NAME`: signs in again and replaces the refresh token.
pub fn login(account: &Account, no_browser: bool, out: &mut impl Write) -> Result<()> {
    let settings = oauth_settings(account, "set-password")?;
    let keyring = Keyring::open_for_oauth()?;
    let client_secret = match keyring.get_oauth(&account.name)? {
        Some(entry) => entry.client_secret(),
        None => Secret::new(secret::read_secret(
            &format!("Client secret for {}: ", settings.client_id),
            "client secret",
        )?),
    };
    let client = oauth::Client {
        id: settings.client_id.clone(),
        secret: client_secret,
    };
    let signed_in = sign_in(
        settings.provider,
        &client,
        &account.imap.username,
        no_browser,
    )?;
    keyring.set_oauth(&account.name, &entry(&client, &signed_in))?;
    writeln!(
        out,
        "Signed in {:?} as {}; the sign-in in {} was replaced.",
        account.name,
        signed_in.email,
        keyring.name()
    )?;
    Ok(())
}

/// `neomail account logout NAME`: revokes the refresh token (a failure is only a warning) and
/// deletes the keyring entry.
pub fn logout(account: &Account, out: &mut impl Write) -> Result<()> {
    let settings = oauth_settings(account, "delete-password")?;
    let keyring = Keyring::open_for_oauth()?;
    let entry = match keyring.get_oauth(&account.name) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            writeln!(
                out,
                "There was no sign-in for {:?} in {}.",
                account.name,
                keyring.name()
            )?;
            return Ok(());
        }
        Err(e) => {
            // Unreadable (malformed, say): delete it anyway. An unreachable keyring fails the
            // delete too, with the same one-line error.
            keyring.delete_oauth(&account.name)?;
            if e.is::<keyring::Malformed>() {
                eprintln!(
                    "neomail: warning: the keyring entry for {:?} was malformed; deleted it \
                     without revoking",
                    account.name
                );
            } else {
                eprintln!("neomail: warning: {e:#}; deleted it without revoking");
            }
            writeln!(
                out,
                "Signed out {:?}: deleted from {}.",
                account.name,
                keyring.name()
            )?;
            return Ok(());
        }
    };
    let mut revoked = false;
    if let Some(token) = entry.refresh_token() {
        let result = Https::new()
            .and_then(|http| runtime()?.block_on(oauth::revoke(&http, settings.provider, &token)));
        match result {
            Ok(()) => revoked = true,
            Err(e) => eprintln!(
                "neomail: warning: {} did not revoke the sign-in ({e:#}); deleting it here anyway",
                settings.provider.name
            ),
        }
    }
    keyring.delete_oauth(&account.name)?;
    writeln!(
        out,
        "Signed out {:?}: {}deleted from {}.",
        account.name,
        if revoked {
            format!("revoked at {}, ", settings.provider.name)
        } else {
            String::new()
        },
        keyring.name()
    )?;
    Ok(())
}

/// The account's OAuth settings, or an error pointing a password account to `password_command`.
fn oauth_settings<'a>(account: &'a Account, password_command: &str) -> Result<&'a OAuthSettings> {
    match &account.login {
        Login::OAuth(settings) => Ok(settings),
        Login::Password(_) => bail!(
            "account {:?} logs in with a password, not OAuth; use `neomail account {password_command} {}`",
            account.name,
            account.name
        ),
    }
}

fn entry(client: &oauth::Client, signed_in: &SignedIn) -> OAuthEntry {
    OAuthEntry {
        client_secret: client.secret.expose().to_owned(),
        refresh_token: Some(signed_in.refresh_token.expose().to_owned()),
    }
}

/// Signs in through the browser and checks the address is `email`.
fn sign_in(
    provider: &Provider,
    client: &oauth::Client,
    email: &str,
    no_browser: bool,
) -> Result<SignedIn> {
    let http = Https::new()?;
    let signed_in = runtime()?.block_on(oauth::sign_in(
        &http,
        provider,
        client,
        Some(email),
        |url| {
            eprintln!(
                "\nSign in with {} in your browser. If it does not open, open this URL:\n\n{url}\n",
                provider.name
            );
            if no_browser {
                // Printing the URL is all.
            } else if let Err(e) = browser::open(url) {
                eprintln!("neomail: cannot open a browser ({e:#}); open the URL yourself");
            }
            eprintln!(
                "Waiting up to {} minutes for the sign-in...",
                oauth::SIGN_IN_TIMEOUT.as_secs() / 60
            );
        },
    ))?;
    if !signed_in.email.eq_ignore_ascii_case(email) {
        bail!(
            "you signed in as {}, not {email}; nothing was saved. Sign in with {email}, or use \
             --email {} for a new account",
            signed_in.email,
            signed_in.email
        );
    }
    Ok(signed_in)
}

/// A fresh access token for an OAuth account, inside the caller's runtime.
pub async fn access_token(
    account: &Account,
    settings: &OAuthSettings,
    client_secret: Secret,
    refresh_token: &Secret,
) -> Result<Secret> {
    let http = Https::new()?;
    let client = oauth::Client {
        id: settings.client_id.clone(),
        secret: client_secret,
    };
    oauth::refresh(
        &http,
        settings.provider,
        &client,
        refresh_token,
        &account.name,
    )
    .await
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")
}
