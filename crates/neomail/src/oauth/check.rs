//! Checks a client ID, and Google's client secret, with the provider before anyone signs in, so
//! `neomail account add` catches a wrong one at the step that made it. Neither request needs a
//! user or grants anything.
//!
//! - Google: a token request with a made-up authorization code. RFC 6749 section 5.2 has a bad
//!   client answered with `invalid_client` and a bad code with `invalid_grant`
//!   (<https://www.rfc-editor.org/rfc/rfc6749.txt>). A live check on 2026-10-13, run twice,
//!   got `400 invalid_grant` for the test Desktop client with its secret, `401 invalid_client`
//!   "The OAuth client was not found." for a made-up ID, `401 invalid_client` "The provided
//!   client secret is invalid." for a wrong secret, and `400 invalid_request` "client_secret is
//!   missing." without one. Google documents neither text
//!   (<https://developers.google.com/identity/protocols/oauth2/native-app>), so the error code
//!   decides and the text only says which of the two values is wrong.
//! - Microsoft: a device code request on `/common`, which a public client sends with its ID
//!   alone. The same live check got `200` for the kept app, and `400` with AADSTS50059 for a
//!   made-up ID, GUID or not. Microsoft documents 50059 as tenant information missing from the
//!   request (<https://learn.microsoft.com/en-us/entra/identity-platform/reference-error-codes>),
//!   so reading it on `/common` as "no such app" is the live check's inference. The device code
//!   that comes back is never shown or polled, and expires unused.
//!
//! Any other answer cannot tell a wrong value from a right one, so the setup goes on and the
//! sign-in shows what is wrong.

use anyhow::Result;

use super::{Client, ClientKind, EndpointError, Pkce, Provider, endpoint_error, printable};
use crate::http::Http;

/// What the provider said of the client.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It knows the client, and for Google the secret is right.
    Known,
    /// It knows no client with this ID: the provider's words.
    UnknownId(String),
    /// Google: the ID is known, but the secret is wrong or missing.
    WrongSecret(String),
    /// Google: `invalid_client` with a text that names neither value.
    BadClient(String),
    /// Microsoft: the app's supported account types leave out `/common` (AADSTS50194).
    NotMultitenant(String),
    /// An answer that tells nothing about the client.
    Unclear(String),
}

/// Never a valid code: it only has to reach the client check.
const MADE_UP_CODE: &str = "neomail-client-check";
/// No one listens there; the code is refused before the redirect URI matters.
const NOWHERE: &str = "http://127.0.0.1:1/";

/// AADSTS50059 on `/common` (see the module comment), and 700016, "application not found".
const UNKNOWN_APP: [u64; 2] = [50059, 700016];
const NOT_MULTITENANT: u64 = 50194;

/// Asks the provider about `client`: a token request with a made-up code for a confidential
/// client (Google), a device code request for a public one (Microsoft). Fails only when the
/// provider could not be reached.
pub async fn check_client<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
) -> Result<Verdict> {
    let response = match (provider.client, provider.device_code_url) {
        (ClientKind::Confidential, _) => {
            let pkce = Pkce::new()?;
            let mut form = vec![
                ("grant_type", "authorization_code"),
                ("code", MADE_UP_CODE),
                ("code_verifier", pkce.verifier.expose()),
                ("redirect_uri", NOWHERE),
            ];
            form.extend(client.credentials(provider)?);
            http.post_form(provider.token_url, &form).await?
        }
        (ClientKind::Public, Some(url)) => {
            let mut form = client.credentials(provider)?;
            form.push(("scope", provider.scope));
            http.post_form(url, &form).await?
        }
        (ClientKind::Public, None) => {
            return Ok(Verdict::Unclear(format!(
                "{} has no request that checks a public client",
                provider.name
            )));
        }
    };
    if response.status == 200 {
        return Ok(Verdict::Known);
    }
    let error = endpoint_error(provider, &response.body, response.status);
    let Some(e) = error.downcast_ref::<EndpointError>() else {
        return Ok(Verdict::Unclear(format!("{error:#}")));
    };
    let words = match &e.description {
        Some(d) => format!("{}: {d}", printable(&e.code, 100)),
        None => printable(&e.code, 100),
    };
    let description = e.description.as_deref().unwrap_or("").to_ascii_lowercase();
    Ok(match (provider.client, e.code.as_str()) {
        (ClientKind::Confidential, "invalid_grant") => Verdict::Known,
        (ClientKind::Confidential, "invalid_client") if description.contains("not found") => {
            Verdict::UnknownId(words)
        }
        (ClientKind::Confidential, "invalid_client") if description.contains("secret") => {
            Verdict::WrongSecret(words)
        }
        (ClientKind::Confidential, "invalid_client") => Verdict::BadClient(words),
        (ClientKind::Confidential, "invalid_request") if description.contains("client_secret") => {
            Verdict::WrongSecret(words)
        }
        (ClientKind::Confidential, "deleted_client") => Verdict::UnknownId(words),
        (ClientKind::Public, _) if e.codes.iter().any(|c| UNKNOWN_APP.contains(c)) => {
            Verdict::UnknownId(words)
        }
        (ClientKind::Public, _) if e.codes.contains(&NOT_MULTITENANT) => {
            Verdict::NotMultitenant(words)
        }
        _ => Verdict::Unclear(words),
    })
}
