//! OAuth 2.0 sign-in for an installed app (RFC 8252): the authorization code flow with PKCE
//! (RFC 7636) and a loopback redirect, or the device code flow (RFC 8628, [`device`]), against the
//! user's own client registration. A provider is data ([`Provider`]): its endpoints, client kind,
//! scopes and mail API.
//!
//! Tokens and the client secret are [`Secret`]s, whose `Debug` is redacted; errors never quote
//! them, nor a token endpoint's response body.

pub mod device;
pub mod loopback;
mod provider;
#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use self::loopback::Listener;
pub use self::provider::{ClientKind, GOOGLE, MICROSOFT, MailApi, PROVIDERS, Provider, provider};
use crate::http::Http;

/// How long the browser has to come back to the loopback listener.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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

/// The user's own client registration. A public client has no secret.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: String,
    pub secret: Option<Secret>,
}

impl Client {
    /// `client_id`, and `client_secret` for a confidential provider only: a public client that
    /// sends one is refused (AADSTS700025,
    /// <https://learn.microsoft.com/en-us/entra/identity-platform/reference-error-codes>).
    fn credentials(&self, provider: &Provider) -> Result<Vec<(&'static str, &str)>> {
        let mut pairs = vec![("client_id", self.id.as_str())];
        if provider.client == ClientKind::Confidential {
            let secret = self
                .secret
                .as_ref()
                .with_context(|| format!("the {} client secret is missing", provider.name))?;
            pairs.push(("client_secret", secret.expose()));
        }
        Ok(pairs)
    }
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

/// What a sign-in leaves: the account's address, a refresh token, and the access token that came
/// with it (for a check before anything is saved; never stored).
#[derive(Debug)]
pub struct SignedIn {
    pub email: String,
    pub refresh_token: Secret,
    pub access_token: Option<Secret>,
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
    // Microsoft's `localhost` may resolve to [::1] in the browser.
    let listener = Listener::bind(provider.redirect_host == "localhost").await?;
    let pkce = Pkce::new()?;
    let state = random_url_safe(24)?;
    let redirect_uri = listener.redirect_uri(provider.redirect_host);
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
    /// Seconds the access token lives (RFC 6749, section 5.1). Some endpoints send it as a
    /// string, so it is read leniently rather than failing the whole answer.
    expires_in: Option<serde_json::Value>,
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
    /// Microsoft's numeric codes (the AADSTS number).
    #[serde(default)]
    error_codes: Vec<u64>,
}

/// Exchanges the authorization code for tokens and checks what came back.
pub async fn exchange_code<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    code: &Secret,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<SignedIn> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.expose()),
        ("code_verifier", pkce.verifier.expose()),
        ("redirect_uri", redirect_uri),
    ];
    form.extend(client.credentials(provider)?);
    let tokens = token_request(http, provider, &form)
        .await
        .map_err(|e| e.context(format!("{} refused the sign-in", provider.name)))?;
    check_sign_in(provider, tokens)
}

/// The checks every sign-in's tokens pass, whichever flow got them: a mail grant, a refresh
/// token, and an address in the ID token.
fn check_sign_in(provider: &Provider, tokens: TokenResponse) -> Result<SignedIn> {
    if !grants_mail(provider, tokens.scope.as_deref()) {
        bail!(
            "{} did not grant mail access ({}); sign in again and, if the consent screen shows \
             a checkbox for mail access, tick it",
            provider.name,
            provider
                .mail_scopes
                .iter()
                .map(|s| format!("{}{s}", provider.scope_prefix))
                .collect::<Vec<_>>()
                .join(" or ")
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
        access_token: tokens.access_token.filter(|t| !t.is_empty()).map(Secret),
    })
}

/// Whether the granted scopes include one of the provider's mail grants. A missing `scope`
/// means the scopes requested ("if omitted, the token is for the scopes requested on the initial
/// leg", <https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>).
/// Microsoft returns full URLs in any case, so the prefix is stripped and case is ignored.
pub fn grants_mail(provider: &Provider, granted: Option<&str>) -> bool {
    let prefix = provider.scope_prefix;
    granted
        .unwrap_or(provider.scope)
        .split_ascii_whitespace()
        .any(|scope| {
            let bare = match scope.get(..prefix.len()) {
                Some(head) if head.eq_ignore_ascii_case(prefix) => &scope[prefix.len()..],
                _ => scope,
            };
            provider
                .mail_scopes
                .iter()
                .any(|m| m.eq_ignore_ascii_case(bare))
        })
}

/// What a refresh brings: an access token and, when the provider rotates it, a new refresh
/// token.
#[derive(Debug)]
pub struct Refreshed {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    /// How long the access token lives from when the answer arrived, when the provider said
    /// (`expires_in`, RFC 6749 section 5.1, RECOMMENDED rather than required).
    pub expires_in: Option<std::time::Duration>,
}

impl Refreshed {
    /// The new refresh token to write back, when the provider sent one that differs from
    /// `saved`. The old one is not revoked
    /// (<https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens>).
    pub fn rotated(&self, saved: &Secret) -> Option<&Secret> {
        self.refresh_token.as_ref().filter(|new| *new != saved)
    }
}

/// A fresh access token from the refresh token. `account` names the account in the hint for an
/// expired or revoked sign-in.
pub async fn refresh<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    refresh_token: &Secret,
    account: &str,
) -> Result<Refreshed> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    form.extend(client.credentials(provider)?);
    if let Some(scope) = provider.refresh_scope {
        form.push(("scope", scope));
    }
    let tokens = match token_request(http, provider, &form).await {
        Ok(tokens) => tokens,
        Err(e) => {
            return Err(match e.downcast_ref::<EndpointError>() {
                Some(err) if err.code == "invalid_grant" => anyhow!(
                    "the saved {} sign-in expired or was revoked ({}); run `neomail account \
                     login {account}`",
                    provider.name,
                    provider.setup.expired_hint
                ),
                Some(err) if err.is_bad_client() => anyhow!(
                    "cannot refresh the {} sign-in: {e:#}; run `neomail account logout {account}`, \
                     then `neomail account login {account}`",
                    provider.name
                ),
                _ => e.context(format!("cannot refresh the {} sign-in", provider.name)),
            });
        }
    };
    let access_token = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .map(Secret)
        .with_context(|| format!("{} returned no access token", provider.name))?;
    Ok(Refreshed {
        access_token,
        refresh_token: tokens.refresh_token.filter(|t| !t.is_empty()).map(Secret),
        expires_in: tokens.expires_in.as_ref().and_then(seconds),
    })
}

/// `expires_in` as a number of seconds, or a string of digits.
fn seconds(value: &serde_json::Value) -> Option<std::time::Duration> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .map(std::time::Duration::from_secs)
}

/// Revokes a token, so it stops working even where a copy survived. Fails for a provider with no
/// revocation endpoint; callers check [`Provider::revoke_url`] first.
pub async fn revoke<H: Http>(http: &H, provider: &Provider, token: &Secret) -> Result<()> {
    let url = provider
        .revoke_url
        .with_context(|| format!("{} has no token revocation endpoint", provider.name))?;
    let response = http.post_form(url, &[("token", token.expose())]).await?;
    if response.status == 200 {
        return Ok(());
    }
    Err(endpoint_error(provider, &response.body, response.status))
}

/// An OAuth error response (RFC 6749 section 5.2): the code, the line to show and a hint. The
/// code and description are defined by the server and hold no secret, but are cleaned of control
/// characters.
#[derive(Debug)]
pub struct EndpointError {
    pub code: String,
    message: String,
    hint: Option<&'static str>,
}

impl EndpointError {
    /// `invalid_client` or `unauthorized_client`: the client ID or secret is wrong.
    fn is_bad_client(&self) -> bool {
        matches!(self.code.as_str(), "invalid_client" | "unauthorized_client")
    }
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        match self.hint {
            Some(hint) => write!(f, "; {hint}"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for EndpointError {}

async fn token_request<H: Http>(
    http: &H,
    provider: &Provider,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let response = http.post_form(provider.token_url, form).await?;
    if response.status != 200 {
        return Err(endpoint_error(provider, &response.body, response.status));
    }
    // serde_json's errors can quote the input, which holds tokens: never pass them on.
    serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("the token endpoint's answer is not the expected JSON"))
}

/// Text from the server, fit for the terminal: control characters (escape sequences included)
/// dropped, and at most `max` characters.
fn printable(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// The error from a response that is not 200: an [`EndpointError`] when it is OAuth's JSON.
fn endpoint_error(provider: &Provider, body: &[u8], status: u16) -> anyhow::Error {
    let Ok(e) = serde_json::from_slice::<ErrorResponse>(body) else {
        return anyhow!("HTTP status {status}");
    };
    let clean = |s: &str| printable(s, 200);
    let message = match e.error_description.as_deref().map(clean) {
        Some(d) if !d.is_empty() => format!("{}: {d}", clean(&e.error)),
        _ => clean(&e.error),
    };
    let coded = e.error_codes.iter().find_map(|code| {
        provider
            .error_code_hints
            .iter()
            .find(|(c, _)| c == code)
            .map(|(_, hint)| *hint)
    });
    let mut error = EndpointError {
        code: e.error,
        message,
        hint: coded,
    };
    if error.hint.is_none() && error.is_bad_client() {
        error.hint = Some(match provider.client {
            ClientKind::Confidential => "check the client ID and secret",
            ClientKind::Public => "check the client ID",
        });
    }
    anyhow::Error::new(error)
}

/// The account's address from an ID token: its `email` claim, else `preferred_username`, since
/// Microsoft's `email` "isn't always" there
/// (<https://learn.microsoft.com/en-us/entra/identity-platform/scopes-oidc>). The signature is
/// not checked: the token came straight from the token endpoint over TLS, and OpenID Connect
/// Core 1.0 section 3.1.3.7, step 6, allows TLS server validation in place of the signature then.
pub fn id_token_email(id_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Claims {
        email: Option<String>,
        email_verified: Option<bool>,
        preferred_username: Option<String>,
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
        .or(claims.preferred_username.filter(|u| u.contains('@')))
        .context("the ID token holds no address; the \"email\" scope is missing")
}
