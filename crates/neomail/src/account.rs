//! `neomail account add`, `login` and `logout`: OAuth accounts through the user's own client,
//! signed in through the browser or with a device code.
//!
//! Keyring calls stay outside the async runtime (zbus starts one of its own); only the sign-in,
//! the token refresh and the revocation run inside it.

use std::io::Write;

use anyhow::{Context, Result, bail};

mod add;

pub use add::{AddArgs, add};

use crate::config::{Account, Login, OAuthSettings};
use crate::http::Https;
use crate::keyring::{self, Keyring, OAuthEntry};
use crate::oauth::{self, ClientKind, Provider, Refreshed, Secret, SignedIn};
use crate::secret::{Input, SavedSignIn};
use crate::{browser, secret};

/// How the user signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// The authorization code flow with a loopback redirect, in a browser.
    Browser,
    /// The device code flow, asked for with `--device-code`.
    DeviceCode,
    /// The device code flow, chosen because this is SSH without a display.
    AutoDeviceCode,
}

/// The sign-in flow: a device code when asked for, or over SSH without a display when the
/// provider has a device code endpoint; the browser otherwise. Asking a provider without one
/// for a device code is a usage error that says why.
fn choose_flow(provider: &Provider, asked: bool, ssh_without_display: bool) -> Result<Flow> {
    let has_device_code = provider.device_code_url.is_some();
    Ok(match (asked, has_device_code) {
        (true, true) => Flow::DeviceCode,
        (true, false) => {
            return Err(crate::usage(format!(
                "--device-code: {}'s device code sign-in does not allow the mail scope \
                 ({}); sign in through the browser, with --no-browser to only print the URL",
                provider.name, provider.setup.guide
            )));
        }
        (false, true) if ssh_without_display => Flow::AutoDeviceCode,
        (false, _) => Flow::Browser,
    })
}

/// The flow for this process's environment.
fn flow_here(provider: &Provider, asked: bool) -> Result<Flow> {
    choose_flow(
        provider,
        asked,
        browser::ssh_without_display(|name| std::env::var(name).ok()),
    )
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
    device_code: bool,
    input: Input,
    out: &mut impl Write,
) -> Result<()> {
    let settings = oauth_settings(account, "set-password")?;
    let flow = flow_here(settings.provider, device_code)?;
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
        account.username(),
        no_browser,
        flow,
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
    if settings.provider.revoke_url.is_none() {
        let line = delete_unrevocable(&keyring, &account.name, settings.provider.name)?;
        writeln!(out, "{line}")?;
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

/// Logout for a provider with no revocation endpoint (Microsoft): deletes the sign-in from the
/// keyring and returns the line that says only that happened.
fn delete_unrevocable(keyring: &Keyring, name: &str, provider: &str) -> Result<String> {
    keyring.delete_oauth(name)?;
    Ok(format!(
        "Signed out {name:?}: {provider} has no token revocation endpoint, so the sign-in was \
         deleted from {} only.",
        keyring.name()
    ))
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
        mail_grant: Some(signed_in.grant),
    }
}

/// Signs in through the browser or with a device code, and checks the address is `email`.
fn sign_in(
    provider: &Provider,
    client: &oauth::Client,
    email: &str,
    no_browser: bool,
    flow: Flow,
) -> Result<SignedIn> {
    let signed_in = sign_in_as(provider, client, email, no_browser, flow)?;
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

/// Signs in through the browser or with a device code, with `email` as the hint.
fn sign_in_as(
    provider: &Provider,
    client: &oauth::Client,
    email: &str,
    no_browser: bool,
    flow: Flow,
) -> Result<SignedIn> {
    let http = Https::new()?;
    if flow == Flow::AutoDeviceCode {
        eprintln!("neomail: over SSH without a display: using device code sign-in");
    }
    let signed_in = if flow == Flow::Browser {
        browser_sign_in(&http, provider, client, email, no_browser)?
    } else {
        runtime()?.block_on(oauth::device::sign_in(
            &http,
            provider,
            client,
            |instructions| {
                eprintln!("\n{instructions}\n\nSign in as {email}. Waiting for the sign-in...");
            },
        ))?
    };
    Ok(signed_in)
}

/// The authorization code flow: prints the URL, opens the browser unless `no_browser`, and waits
/// on the loopback for the redirect.
fn browser_sign_in(
    http: &Https,
    provider: &Provider,
    client: &oauth::Client,
    email: &str,
    no_browser: bool,
) -> Result<SignedIn> {
    runtime()?.block_on(oauth::sign_in(http, provider, client, Some(email), |url| {
        let opening = if no_browser {
            "Open this URL"
        } else {
            "If it does not open, open this URL"
        };
        eprintln!(
            "\nSign in with {} in your browser. {opening}:\n\n{url}\n",
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
    }))
}

/// A fresh access token for an OAuth account, for no more than the saved sign-in grants, and the
/// refresh token if the provider rotated it, inside the caller's runtime. A confidential
/// provider's entry without a client secret cannot refresh: the error says to log in again,
/// which asks for it.
pub async fn refresh(
    account: &Account,
    settings: &OAuthSettings,
    saved: &SavedSignIn,
) -> Result<Refreshed> {
    if settings.provider.client == ClientKind::Confidential && saved.client_secret.is_none() {
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
        secret: saved.client_secret.clone(),
    };
    oauth::refresh(
        &http,
        settings.provider,
        &client,
        &saved.refresh_token,
        saved.grant,
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
                mail_grant: None,
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
        let config = crate::config::Config::parse(
            "[[accounts]]\nname = \"gmail\"\nusername = \"you@example.com\"\n\
             oauth = { provider = \"google\", client_id = \"1234-abc.apps.googleusercontent.com\" }\n",
        )
        .unwrap();
        let account = &config.accounts[0];
        let Login::OAuth(settings) = &account.login else {
            panic!("{:?}", account.login)
        };
        let saved = SavedSignIn {
            client_secret: None,
            refresh_token: Secret::new("rt".into()),
            grant: oauth::MailGrant::ReadWrite,
        };
        // The check comes before any request, so this never reaches the network.
        let e = runtime()
            .unwrap()
            .block_on(refresh(account, settings, &saved))
            .unwrap_err();
        assert!(
            e.to_string()
                .ends_with("needs; run `neomail account login gmail`"),
            "{e}"
        );
    }

    #[test]
    fn device_code_is_used_when_asked_or_over_ssh_and_refused_for_google() {
        use oauth::{GOOGLE, MICROSOFT};
        // (provider, --device-code, SSH without a display, expected)
        let cases = [
            (&MICROSOFT, false, false, Flow::Browser),
            (&MICROSOFT, true, false, Flow::DeviceCode),
            (&MICROSOFT, true, true, Flow::DeviceCode),
            (&MICROSOFT, false, true, Flow::AutoDeviceCode),
            (&GOOGLE, false, false, Flow::Browser),
            // Google has no device code for Gmail: over SSH the URL is printed instead.
            (&GOOGLE, false, true, Flow::Browser),
        ];
        for (provider, asked, ssh, want) in cases {
            assert_eq!(
                choose_flow(provider, asked, ssh).unwrap(),
                want,
                "{}",
                provider.id
            );
        }
        for ssh in [false, true] {
            let e = choose_flow(&GOOGLE, true, ssh).unwrap_err();
            assert!(e.is::<crate::UsageError>());
            assert!(
                e.to_string().starts_with(
                    "--device-code: Google's device code sign-in does not allow the mail scope"
                ),
                "{e}"
            );
        }
    }

    #[test]
    fn logout_without_a_revocation_endpoint_deletes_and_says_so() {
        let keyring = Keyring::with_store(mock::Store::new().unwrap(), "the mock", false);
        let saved = OAuthEntry {
            client_secret: None,
            refresh_token: Some("rt".into()),
            mail_grant: None,
        };
        keyring.set_oauth("outlook", &saved).unwrap();
        let line = delete_unrevocable(&keyring, "outlook", "Microsoft").unwrap();
        assert_eq!(
            line,
            "Signed out \"outlook\": Microsoft has no token revocation endpoint, so the sign-in \
             was deleted from the mock only."
        );
        assert!(keyring.get_oauth("outlook").unwrap().is_none(), "deleted");
    }

    #[test]
    fn a_leftover_without_a_revocation_endpoint_is_only_deleted() {
        let keyring = Keyring::with_store(mock::Store::new().unwrap(), "the mock", false);
        let leftover = OAuthEntry {
            client_secret: None,
            refresh_token: Some("rt".into()),
            mail_grant: None,
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
