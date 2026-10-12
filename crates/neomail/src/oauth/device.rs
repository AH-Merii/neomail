//! The device authorization grant (RFC 8628) for a machine without a browser: neomail shows a code
//! and a URL, the user signs in on another device, and neomail polls the token endpoint until the
//! sign-in ends. Every request is a form POST, so the [`Http`] seam covers it.
//!
//! Polling follows <https://www.rfc-editor.org/rfc/rfc8628> section 3.5 and Microsoft's
//! <https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code>.

use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use tokio::time::Instant;

use super::{
    Client, EndpointError, Provider, Secret, SignedIn, check_sign_in, endpoint_error, printable,
};
use crate::http::Http;

/// The polling interval when the answer names none (RFC 8628 section 3.2).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
/// How much `slow_down` adds to the interval, for that and every later request (section 3.5).
const SLOW_DOWN: Duration = Duration::from_secs(5);
/// How long the code lives when the answer does not say: 15 minutes, Microsoft's default.
const DEFAULT_EXPIRES_IN: u64 = 900;

/// The device authorization response. No `Debug` derive: it holds the device code.
#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    message: Option<String>,
}

impl fmt::Debug for DeviceCodeResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceCodeResponse(<redacted>)")
    }
}

/// Signs in with a device code: asks for one, hands the instructions to `show` (which prints them
/// on standard error), then polls until the user finishes, declines or the code expires.
pub async fn sign_in<H: Http>(
    http: &H,
    provider: &Provider,
    client: &Client,
    show: impl FnOnce(&str),
) -> Result<SignedIn> {
    let url = provider
        .device_code_url
        .with_context(|| format!("{} has no device code sign-in", provider.name))?;
    let mut form = client.credentials(provider)?;
    form.push(("scope", provider.scope));
    let response = http.post_form(url, &form).await?;
    if response.status != 200 {
        return Err(endpoint_error(provider, &response.body, response.status)
            .context(format!("{} refused the device code request", provider.name)));
    }
    // serde_json's errors can quote the input, which holds the device code: never pass them on.
    let answer: DeviceCodeResponse = serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("the device code answer is not the expected JSON"))?;
    let device_code = Secret::new(answer.device_code);
    let instructions = match (answer.message, answer.verification_uri, answer.user_code) {
        (Some(message), _, _) if !message.is_empty() => message,
        (_, Some(uri), Some(code)) => format!("To sign in, open {uri} and enter the code {code}."),
        _ => bail!("the device code answer has neither a message nor a URL and code"),
    };
    show(&printable(&instructions, 1000));

    let lifetime = Duration::from_secs(answer.expires_in.unwrap_or(DEFAULT_EXPIRES_IN));
    let deadline = Instant::now() + lifetime;
    let mut interval = answer
        .interval
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL);
    let mut poll = client.credentials(provider)?;
    poll.push(("grant_type", "urn:ietf:params:oauth:grant-type:device_code"));
    poll.push(("device_code", device_code.expose()));
    loop {
        tokio::time::sleep(interval).await;
        if Instant::now() >= deadline {
            bail!(
                "the sign-in did not finish within {} minutes; run the command again",
                lifetime.as_secs() / 60
            );
        }
        let error = match super::token_request(http, provider, &poll).await {
            Ok(tokens) => return check_sign_in(provider, tokens),
            Err(e) => e,
        };
        let code = error
            .downcast_ref::<EndpointError>()
            .map(|e| e.code.as_str());
        match code {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += SLOW_DOWN,
            Some("authorization_declined" | "access_denied") => {
                bail!("the sign-in was declined; nothing was saved")
            }
            Some("expired_token") => {
                bail!("the code expired before the sign-in finished; run the command again")
            }
            Some("bad_verification_code") => bail!(
                "{} did not recognise the device code; run the command again",
                provider.name
            ),
            _ => return Err(error.context(format!("{} refused the sign-in", provider.name))),
        }
    }
}
