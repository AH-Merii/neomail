//! `neomail account add`, `login` and `logout`: OAuth accounts through the user's own client.
//!
//! Keyring calls stay outside the async runtime (zbus starts one of its own); only the sign-in,
//! the token refresh and the revocation run inside it.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::config::{self, Account, Login, OAuthSettings};
use crate::keyring::{self, Keyring, OAuthEntry};
use crate::oauth::http::Https;
use crate::oauth::{self, ClientKind, Provider, Refreshed, Secret, SignedIn};
use crate::secret::Input;
use crate::{browser, secret};

pub struct AddArgs {
    pub name: Option<String>,
    pub email: Option<String>,
    pub client_id: Option<String>,
    pub no_browser: bool,
}

/// `neomail account add gmail`: shows the provider's setup steps, asks for what is missing,
/// signs in, stores the sign-in in the keyring, then appends the account to the config file.
pub fn add(
    provider: &'static Provider,
    config_path: &Path,
    args: AddArgs,
    input: Input,
    out: &mut impl Write,
) -> Result<()> {
    let interactive = input.can_prompt();
    let setup = &provider.setup;
    let confidential = provider.client == ClientKind::Confidential;
    if !interactive {
        let missing: Vec<&str> = [
            ("--email", args.email.is_none()),
            ("--client-id", args.client_id.is_none()),
        ]
        .into_iter()
        .filter_map(|(flag, missing)| missing.then_some(flag))
        .collect();
        if !missing.is_empty() {
            let secret_note = if confidential {
                " (the client secret is then read as one line of standard input)"
            } else {
                ""
            };
            return Err(secret::cannot_ask(
                input,
                &format!("{}{secret_note}", missing.join(" and ")),
            ));
        }
    } else {
        eprintln!("{}", setup.steps.replace("GUIDE", setup.guide));
    }

    let name = match args.name {
        Some(name) => name.trim().to_owned(),
        None if interactive => {
            let typed = secret::prompt_line(&format!(
                "Account name in neomail [{}]: ",
                setup.default_name
            ))?;
            if typed.is_empty() {
                setup.default_name.to_owned()
            } else {
                typed
            }
        }
        None => setup.default_name.to_owned(),
    };
    if name.is_empty() {
        bail!("the account name is empty");
    }
    config::check_can_add(config_path, &name)?;
    let email = match args.email {
        Some(email) => email.trim().to_owned(),
        None => secret::prompt_line(setup.address_prompt)?,
    };
    if !email.contains('@') {
        bail!("{email:?} is not an email address");
    }
    let client_id = match args.client_id {
        Some(id) => id.trim().to_owned(),
        None => secret::prompt_line(setup.client_id_prompt)?,
    };
    if client_id.is_empty() {
        bail!("the client ID is empty");
    }
    let client_secret = if confidential {
        Some(Secret::new(secret::read_secret(
            setup.client_secret_prompt,
            "client secret",
            input,
        )?))
    } else {
        None
    };
    // Before the browser: no point signing in when the sign-in cannot be kept. On Linux,
    // opening the store connects to the Secret Service; reading the entry checks it answers.
    // The append is tried with the address given, which the sign-in must match.
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
        secret: client_secret,
    };

    let signed_in = sign_in(provider, &client, &email, args.no_browser)?;
    keyring.set_oauth(&name, &entry(&client, &signed_in))?;
    let block = config::oauth_account_toml(&name, &signed_in.email, provider, &client.id);
    config::append_account(config_path, &name, &block).context(
        "the sign-in is stored in the keyring, but the config file could not be updated",
    )?;
    writeln!(
        out,
        "Added {name:?} ({}) to {}. Its sign-in is in {} (service {:?}, username {name:?}).\n\
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
    let revoke = provider.revoke_url.map(|_| {
        |token: &Secret| {
            Https::new().and_then(|http| runtime()?.block_on(oauth::revoke(&http, provider, token)))
        }
    });
    let line = clear_leftover(keyring, name, leftover, provider.name, revoke)?;
    eprintln!("{line}");
    Ok(())
}

/// Revokes the leftover's refresh token with `revoke` (a failure is only a warning; `None` when
/// the provider has no revocation endpoint), deletes the entry, and returns the line that says
/// which of those happened.
fn clear_leftover(
    keyring: &Keyring,
    name: &str,
    leftover: &OAuthEntry,
    provider: &str,
    revoke: Option<impl FnOnce(&Secret) -> Result<()>>,
) -> Result<String> {
    let token = leftover.refresh_token();
    let can_revoke = revoke.is_some();
    let revoked = token.zip(revoke).map(|(token, revoke)| revoke(&token));
    keyring.delete_oauth(name)?;
    let held = format!(
        "{} held an earlier sign-in for {name:?} with no account in the config",
        keyring.name()
    );
    Ok(match revoked {
        Some(Ok(())) => format!("neomail: {held}; revoked and deleted it"),
        Some(Err(e)) => format!(
            "neomail: warning: {held}; deleted it, but {provider} did not revoke it ({e:#})"
        ),
        None if !can_revoke => {
            format!("neomail: {held}; deleted it ({provider} has no token revocation endpoint)")
        }
        None => format!("neomail: {held}; deleted it (it had no refresh token to revoke)"),
    })
}

/// `neomail account login NAME`: signs in again and replaces the refresh token.
pub fn login(
    account: &Account,
    no_browser: bool,
    input: Input,
    out: &mut impl Write,
) -> Result<()> {
    let settings = oauth_settings(account, "set-password")?;
    let keyring = Keyring::open_for_oauth()?;
    let saved = keyring
        .get_oauth(&account.name)?
        .and_then(|entry| entry.client_secret());
    let client_secret = match (settings.provider.client, saved) {
        (ClientKind::Public, _) => None,
        (ClientKind::Confidential, Some(saved)) => Some(saved),
        (ClientKind::Confidential, None) => Some(Secret::new(secret::read_secret(
            &format!("Client secret for {}: ", settings.client_id),
            "client secret",
            input,
        )?)),
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
    let provider = settings.provider;
    if provider.revoke_url.is_none() {
        keyring.delete_oauth(&account.name)?;
        writeln!(
            out,
            "Signed out {:?}: {} has no token revocation endpoint, so the sign-in was deleted \
             from {} only.",
            account.name,
            provider.name,
            keyring.name()
        )?;
        return Ok(());
    }
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
        client_secret: client.secret.as_ref().map(|s| s.expose().to_owned()),
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

/// A fresh access token for an OAuth account, and the refresh token if the provider rotated it,
/// inside the caller's runtime. A confidential provider's entry without a client secret cannot
/// refresh: the error says to log in again, which asks for it.
pub async fn refresh(
    account: &Account,
    settings: &OAuthSettings,
    client_secret: Option<Secret>,
    refresh_token: &Secret,
) -> Result<Refreshed> {
    if settings.provider.client == ClientKind::Confidential && client_secret.is_none() {
        bail!(
            "the keyring entry holds no client secret, which {} needs; run `neomail account \
             login {}`",
            settings.provider.name,
            account.name
        );
    }
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use anyhow::anyhow;
    use keyring_core::mock;

    use super::*;

    #[test]
    fn a_leftover_sign_in_is_deleted_and_the_line_says_whether_it_was_revoked() {
        // (refresh token, revocation result, expected line end)
        let cases: [(Option<&str>, Result<(), &str>, &str); 3] = [
            (Some("rt"), Ok(()), "; revoked and deleted it"),
            (
                Some("rt"),
                Err("HTTP 503"),
                "; deleted it, but Google did not revoke it (HTTP 503)",
            ),
            (
                None,
                Ok(()),
                "; deleted it (it had no refresh token to revoke)",
            ),
        ];
        for (refresh_token, result, end) in cases {
            let keyring = Keyring::with_store(mock::Store::new().unwrap(), "the mock", false);
            let leftover = OAuthEntry {
                client_secret: Some("cs".into()),
                refresh_token: refresh_token.map(str::to_owned),
            };
            keyring.set_oauth("gmail", &leftover).unwrap();
            let called = Cell::new(false);
            let revoke = |token: &Secret| {
                called.set(true);
                assert_eq!(token.expose(), "rt");
                result.map_err(|e| anyhow!(e))
            };
            let line =
                clear_leftover(&keyring, "gmail", &leftover, "Google", Some(revoke)).unwrap();
            assert!(
                line.ends_with(end) && line.contains("the mock held an earlier sign-in"),
                "{line}"
            );
            assert_eq!(line.starts_with("neomail: warning:"), result.is_err());
            assert_eq!(called.get(), refresh_token.is_some());
            assert!(keyring.get_oauth("gmail").unwrap().is_none(), "deleted");
        }
    }

    #[test]
    fn a_google_entry_without_a_client_secret_says_to_log_in_again() {
        let config = config::Config::parse(
            "[[accounts]]\nname = \"gmail\"\nusername = \"you@example.com\"\n\
             oauth = { provider = \"google\", client_id = \"1234-abc.apps.googleusercontent.com\" }\n",
        )
        .unwrap();
        let account = &config.accounts[0];
        let Login::OAuth(settings) = &account.login else {
            panic!("{:?}", account.login)
        };
        let refresh_token = Secret::new("rt".into());
        // The check comes before any request, so this never reaches the network.
        let e = runtime()
            .unwrap()
            .block_on(refresh(account, settings, None, &refresh_token))
            .unwrap_err();
        assert!(
            e.to_string()
                .ends_with("needs; run `neomail account login gmail`"),
            "{e}"
        );
    }

    #[test]
    fn a_leftover_without_a_revocation_endpoint_is_only_deleted() {
        let keyring = Keyring::with_store(mock::Store::new().unwrap(), "the mock", false);
        let leftover = OAuthEntry {
            client_secret: None,
            refresh_token: Some("rt".into()),
        };
        keyring.set_oauth("outlook", &leftover).unwrap();
        let no_revoke: Option<fn(&Secret) -> Result<()>> = None;
        let line = clear_leftover(&keyring, "outlook", &leftover, "Microsoft", no_revoke).unwrap();
        assert!(
            line.ends_with("; deleted it (Microsoft has no token revocation endpoint)"),
            "{line}"
        );
        assert!(keyring.get_oauth("outlook").unwrap().is_none(), "deleted");
    }
}
