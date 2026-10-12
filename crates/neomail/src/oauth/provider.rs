//! The OAuth providers, as data: endpoints, client kind, scopes, the mail API and the setup text.
//! Slice 6 adds Microsoft as another value, not another type.

/// Whether the client registration has a secret (RFC 6749 section 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// Google's Desktop app client: it has a secret, sent with every token request.
    Confidential,
    /// Microsoft's public client: it has none, and sending one is an error (AADSTS700025).
    Public,
}

/// Where the account's mail is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailApi {
    /// IMAP over TLS with XOAUTH2.
    Imap { host: &'static str, port: u16 },
    /// Microsoft Graph, which part 3 of slice 6 adds.
    Graph,
}

/// What `neomail account add` shows and asks for this provider.
#[derive(Debug, PartialEq, Eq)]
pub struct Setup {
    /// The account name when none is given: "gmail".
    pub default_name: &'static str,
    pub address_prompt: &'static str,
    pub client_id_prompt: &'static str,
    /// Asked only of a [`ClientKind::Confidential`] client.
    pub client_secret_prompt: &'static str,
    /// The one-time setup, shown before the wizard asks for anything; `GUIDE` stands for
    /// [`Setup::guide`].
    pub steps: &'static str,
    /// Where the why of each step is.
    pub guide: &'static str,
    /// Why a saved sign-in stops working, for the `invalid_grant` message.
    pub expired_hint: &'static str,
}

/// An OAuth provider.
#[derive(Debug, PartialEq, Eq)]
pub struct Provider {
    /// The `provider` value in the config.
    pub id: &'static str,
    /// For messages: "Google".
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    /// `None` when the provider has no token revocation endpoint.
    pub revoke_url: Option<&'static str>,
    /// The device authorization endpoint (RFC 8628), when the provider's device flow allows the
    /// mail scopes.
    pub device_code_url: Option<&'static str>,
    pub client: ClientKind,
    /// The host in the loopback `redirect_uri`; the listener is always on 127.0.0.1.
    pub redirect_host: &'static str,
    /// Every scope asked for, space-separated.
    pub scope: &'static str,
    /// The grants that give mail access, without [`Provider::scope_prefix`]. A user can untick
    /// mail on Google's granular consent screen, so the granted scopes are checked for one.
    pub mail_scopes: &'static [&'static str],
    /// Stripped from a granted scope before it is matched against `mail_scopes`.
    pub scope_prefix: &'static str,
    /// The `scope` sent with a refresh, when the provider needs one.
    pub refresh_scope: Option<&'static str>,
    pub mail: MailApi,
    /// Extra query parameters for the authorization URL.
    pub auth_params: &'static [(&'static str, &'static str)],
    /// Hints for the provider's numeric error codes (Microsoft's `error_codes`).
    pub error_code_hints: &'static [(u64, &'static str)],
    pub setup: Setup,
}

/// Google, per <https://developers.google.com/identity/protocols/oauth2/native-app>.
/// `access_type=offline` asks for a refresh token; `prompt=consent` makes Google return one on
/// every sign-in, not only the first. Its device flow allows only some scopes, not Gmail's, so it
/// has no `device_code_url`
/// (<https://developers.google.com/identity/protocols/oauth2/limited-input-device>).
pub const GOOGLE: Provider = Provider {
    id: "google",
    name: "Google",
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    revoke_url: Some("https://oauth2.googleapis.com/revoke"),
    device_code_url: None,
    client: ClientKind::Confidential,
    redirect_host: "127.0.0.1",
    scope: "https://mail.google.com/ openid email",
    mail_scopes: &["https://mail.google.com/"],
    scope_prefix: "",
    refresh_scope: None,
    mail: MailApi::Imap {
        host: "imap.gmail.com",
        port: 993,
    },
    auth_params: &[("access_type", "offline"), ("prompt", "consent")],
    error_code_hints: &[],
    setup: Setup {
        default_name: "gmail",
        address_prompt: "Gmail address: ",
        client_id_prompt: "Client ID (from step 5): ",
        client_secret_prompt: "Client secret (from step 5, not shown): ",
        steps: GMAIL_STEPS,
        guide: "https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md",
        expired_hint: "an app in Testing mode gets sign-ins that last 7 days; see docs/gmail.md",
    },
};

/// The one-time Gmail setup. The why of each step is in the guide.
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

At sign-in Google may warn that it hasn't verified the app: it is yours, so go on (Continue
while the app is in Testing; once published, Advanced, then Go to the app's name).
While the app's publishing status is Testing, Google ends the sign-in after 7 days and you
run `neomail account login NAME` again. To avoid that, publish it (Audience > Publish app): an
app for your own use (under 100 users) needs no verification.
";

/// Microsoft's identity platform, v2 endpoints on the `common` tenant, for work, school and
/// personal accounts. Not in [`PROVIDERS`] until part 3 of slice 6 adds the Graph source.
///
/// - No revocation endpoint: the v2 endpoints list none
///   (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc>).
/// - No `prompt=consent`: `offline_access` alone brings a refresh token, and the refresh scope
///   must be the original's or a subset, for the first scope's resource
///   (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>).
/// - Mail.Read only, pending the owner: Mail.ReadWrite changes only the scope strings.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "part 3 of slice 6 adds it to PROVIDERS")
)]
pub const MICROSOFT: Provider = Provider {
    id: "microsoft",
    name: "Microsoft",
    auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    revoke_url: None,
    device_code_url: Some("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"),
    client: ClientKind::Public,
    redirect_host: "localhost",
    scope: "offline_access openid profile email https://graph.microsoft.com/Mail.Read",
    mail_scopes: &["Mail.Read", "Mail.ReadWrite"],
    scope_prefix: "https://graph.microsoft.com/",
    refresh_scope: Some("offline_access https://graph.microsoft.com/Mail.Read"),
    mail: MailApi::Graph,
    auth_params: &[],
    // <https://learn.microsoft.com/en-us/entra/identity-platform/reference-error-codes>
    error_code_hints: &[
        (
            50194,
            "the app registration is not multitenant: set its supported account types to any \
             organizational directory and personal Microsoft accounts",
        ),
        (
            9002332,
            "the app registration is for Microsoft Entra (work or school) accounts only: allow \
             personal Microsoft accounts in its supported account types",
        ),
        (
            700025,
            "the app registration is a public client, which must not send a client secret",
        ),
        (
            7000218,
            "probably the app registration's public client flows are off: set Authentication > \
             Allow public client flows to Yes",
        ),
    ],
    setup: Setup {
        default_name: "outlook",
        address_prompt: "Outlook address: ",
        client_id_prompt: "Application (client) ID (from step 4): ",
        client_secret_prompt: "",
        steps: OUTLOOK_STEPS,
        guide: "https://github.com/AH-Merii/neomail/blob/main/docs/outlook.md",
        expired_hint: "Microsoft ends a sign-in unused for 90 days, after a password change, or \
                       when access is revoked",
    },
};

/// The one-time Outlook setup; part 3 of slice 6 writes the guide.
const OUTLOOK_STEPS: &str = "\
Outlook signs in through your own Microsoft Entra app registration: neomail ships none, so no
one else's app is ever granted access to your mail. Set one up once (why each step exists:
GUIDE):

 1. Sign in at https://entra.microsoft.com. A personal account (Outlook.com, Hotmail) needs a
    directory first: a free Azure account gives it one.
 2. App registrations > New registration. Supported account types: any organizational
    directory and personal Microsoft accounts. Redirect URI: Public client/native,
    http://localhost.
    Why: so both work and personal addresses can sign in, with no client secret.
 3. Authentication > Allow public client flows: Yes.
    Why: it allows the device code sign-in, for a machine without a browser.
 4. Overview: copy the Application (client) ID.
";

pub const PROVIDERS: &[&Provider] = &[&GOOGLE];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().copied().find(|p| p.id == id)
}
