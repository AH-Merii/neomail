//! OAuth 2.0 sign-in for an installed app (RFC 8252): the authorization code flow with PKCE
//! (RFC 7636) and a loopback redirect, against the user's own client registration. A provider is
//! data ([`Provider`]): its endpoints, scopes and IMAP server.
//!
//! Tokens and the client secret are [`Secret`]s, whose `Debug` is redacted; errors never quote
//! them, nor a token endpoint's response body.

pub mod http;
pub mod loopback;
#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use self::http::Http;
use self::loopback::Listener;

/// How long the browser has to come back to the loopback listener.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// An OAuth provider, as data: slice 6 adds Microsoft as another value, not another type.
#[derive(Debug, PartialEq, Eq)]
pub struct Provider {
    /// The `provider` value in the config.
    pub id: &'static str,
    /// For messages: "Google".
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub revoke_url: &'static str,
    /// Every scope asked for, space-separated.
    pub scope: &'static str,
    /// The scope mail access needs. A user can untick it on Google's granular consent screen,
    /// so the granted scopes are checked for it.
    pub mail_scope: &'static str,
    pub imap_host: &'static str,
    pub imap_port: u16,
    /// Extra query parameters for the authorization URL.
    pub auth_params: &'static [(&'static str, &'static str)],
}

/// Google, per <https://developers.google.com/identity/protocols/oauth2/native-app>.
/// `access_type=offline` asks for a refresh token; `prompt=consent` makes Google return one on
/// every sign-in, not only the first.
pub const GOOGLE: Provider = Provider {
    id: "google",
    name: "Google",
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    revoke_url: "https://oauth2.googleapis.com/revoke",
    scope: "https://mail.google.com/ openid email",
    mail_scope: "https://mail.google.com/",
    imap_host: "imap.gmail.com",
    imap_port: 993,
    auth_params: &[("access_type", "offline"), ("prompt", "consent")],
};

pub const PROVIDERS: &[&Provider] = &[&GOOGLE];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().copied().find(|p| p.id == id)
}

/// A token or client secret. `Debug` is redacted so it cannot leak through `{:?}`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The user's own client registration.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: String,
    pub secret: Secret,
}

/// PKCE's verifier and its S256 challenge (RFC 7636 section 4).
pub struct Pkce {
    verifier: Secret,
    challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pkce(<redacted>)")
    }
}

impl Pkce {
    /// A verifier of 32 random octets, base64url-encoded into 43 characters, as RFC 7636
    /// section 4.1 recommends.
    pub fn new() -> Result<Self> {
        Ok(Self::from_verifier(random_url_safe(32)?))
    }

    fn from_verifier(verifier: String) -> Self {
        let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier: Secret(verifier),
            challenge,
        }
    }
}

/// `n` octets from the operating system's CSPRNG, base64url-encoded without padding.
pub fn random_url_safe(n: usize) -> Result<String> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = vec![0u8; n];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("the system's random number generator failed"))?;
    Ok(B64URL.encode(bytes))
}

/// Percent-encodes everything but RFC 3986's unreserved characters, which is valid both in a
/// query string and in an `application/x-www-form-urlencoded` body.
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    fn encode(out: &mut String, s: &str) {
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                out.push(char::from(b));
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode(&mut out, k);
        out.push('=');
        encode(&mut out, v);
    }
    out
}

/// The URL that starts the sign-in in the browser.
pub fn auth_url(
    provider: &Provider,
    client_id: &str,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
    login_hint: Option<&str>,
) -> String {
    let mut pairs = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", provider.scope),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    pairs.extend_from_slice(provider.auth_params);
    if let Some(hint) = login_hint {
        pairs.push(("login_hint", hint));
    }
    format!("{}?{}", provider.auth_url, form_encode(&pairs))
}

/// What a sign-in leaves: the account's address and a refresh token.
#[derive(Debug)]
pub struct SignedIn {
    pub email: String,
    pub refresh_token: Secret,
}

/// Signs in: listens on the loopback, hands the authorization URL to `show` (which prints it and
/// opens the browser), waits for the redirect, then exchanges the code.
pub async fn sign_in<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    login_hint: Option<&str>,
    show: impl FnOnce(&str),
) -> Result<SignedIn> {
    let listener = Listener::bind().await?;
    let pkce = Pkce::new()?;
    let state = random_url_safe(24)?;
    let redirect_uri = listener.redirect_uri();
    show(&auth_url(
        provider,
        &client.id,
        &redirect_uri,
        &pkce,
        &state,
        login_hint,
    ));
    let code = listener.wait(&state, SIGN_IN_TIMEOUT).await?;
    exchange_code(http, provider, client, &code, &pkce, &redirect_uri).await
}

/// The token endpoint's answer. No `Debug` derive: it holds tokens.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
    id_token: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenResponse(<redacted>)")
    }
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

/// Exchanges the authorization code for tokens and checks what came back: the mail scope, a
/// refresh token, and the address in the ID token.
pub async fn exchange_code<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    code: &Secret,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<SignedIn> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.expose()),
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.expose()),
        ("code_verifier", pkce.verifier.expose()),
        ("redirect_uri", redirect_uri),
    ];
    let tokens = token_request(http, provider, &form)
        .await
        .map_err(|e| e.context(format!("{} refused the sign-in", provider.name)))?;
    let granted = tokens.scope.as_deref().unwrap_or("");
    if !granted.split(' ').any(|s| s == provider.mail_scope) {
        bail!(
            "{} did not grant mail access ({}); sign in again and, if the consent screen shows \
             a checkbox for mail access, tick it",
            provider.name,
            provider.mail_scope
        );
    }
    let refresh_token = tokens
        .refresh_token
        .filter(|t| !t.is_empty())
        .with_context(|| format!("{} returned no refresh token", provider.name))?;
    let id_token = tokens
        .id_token
        .with_context(|| format!("{} returned no ID token", provider.name))?;
    Ok(SignedIn {
        email: id_token_email(&id_token)?,
        refresh_token: Secret(refresh_token),
    })
}

/// A fresh access token from the refresh token. `account` names the account in the hint for an
/// expired or revoked sign-in.
pub async fn refresh<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    refresh_token: &Secret,
    account: &str,
) -> Result<Secret> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
        ("client_id", client.id.as_str()),
        ("client_secret", client.secret.expose()),
    ];
    match token_request(http, provider, &form).await {
        Ok(tokens) => tokens
            .access_token
            .filter(|t| !t.is_empty())
            .map(Secret)
            .with_context(|| format!("{} returned no access token", provider.name)),
        Err(e) if e.downcast_ref::<InvalidGrant>().is_some() => bail!(
            "the saved {} sign-in expired or was revoked (an app in Testing mode gets sign-ins \
             that last 7 days; see docs/gmail.md); run `neomail account login {account}`",
            provider.name
        ),
        Err(e) if e.is::<BadClient>() => bail!(
            "cannot refresh the {} sign-in: {e:#}; run `neomail account logout {account}`, then \
             `neomail account login {account}`",
            provider.name
        ),
        Err(e) => Err(e.context(format!("cannot refresh the {} sign-in", provider.name))),
    }
}

/// Revokes a token, so it stops working even where a copy survived.
pub async fn revoke<H: Http>(http: &H, provider: &Provider, token: &Secret) -> Result<()> {
    let response = http
        .post_form(provider.revoke_url, &[("token", token.expose())])
        .await?;
    if response.status == 200 {
        return Ok(());
    }
    Err(endpoint_error(&response.body, response.status))
}

/// `invalid_grant` from the token endpoint: the code or refresh token is no longer valid.
#[derive(Debug)]
struct InvalidGrant;

impl fmt::Display for InvalidGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid_grant")
    }
}

impl std::error::Error for InvalidGrant {}

/// `invalid_client` or `unauthorized_client`: the client ID or secret is wrong.
#[derive(Debug)]
struct BadClient(String);

impl fmt::Display for BadClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}; check the client ID and secret", self.0)
    }
}

impl std::error::Error for BadClient {}

async fn token_request<H: Http>(
    http: &H,
    provider: &Provider,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let response = http.post_form(provider.token_url, form).await?;
    if response.status != 200 {
        return Err(endpoint_error(&response.body, response.status));
    }
    // serde_json's errors can quote the input, which holds tokens: never pass them on.
    serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("the token endpoint's answer is not the expected JSON"))
}

/// The OAuth error code and description from an error response (RFC 6749 section 5.2); both are
/// defined by the server and hold no secret, but are cleaned of control characters.
fn endpoint_error(body: &[u8], status: u16) -> anyhow::Error {
    let Ok(e) = serde_json::from_slice::<ErrorResponse>(body) else {
        return anyhow!("HTTP status {status}");
    };
    if e.error == "invalid_grant" {
        return anyhow::Error::new(InvalidGrant);
    }
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect::<String>()
    };
    let message = match e.error_description.as_deref().map(clean) {
        Some(d) if !d.is_empty() => format!("{}: {d}", clean(&e.error)),
        _ => clean(&e.error),
    };
    match e.error.as_str() {
        "invalid_client" | "unauthorized_client" => anyhow::Error::new(BadClient(message)),
        _ => anyhow!("{message}"),
    }
}

/// The `email` claim of an ID token. The signature is not checked: the token came straight from
/// the token endpoint over TLS, and OpenID Connect Core 1.0 section 3.1.3.7, step 6, allows TLS
/// server validation in place of the signature then.
pub fn id_token_email(id_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Claims {
        email: Option<String>,
        email_verified: Option<bool>,
    }
    let malformed = || anyhow!("the ID token is malformed");
    let payload = id_token.split('.').nth(1).ok_or_else(malformed)?;
    let json = B64URL
        .decode(payload.trim_end_matches('='))
        .map_err(|_| malformed())?;
    let claims: Claims = serde_json::from_slice(&json).map_err(|_| malformed())?;
    if claims.email_verified == Some(false) {
        bail!("the account's address is not verified");
    }
    claims
        .email
        .filter(|e| !e.is_empty())
        .context("the ID token holds no address; the \"email\" scope is missing")
}
