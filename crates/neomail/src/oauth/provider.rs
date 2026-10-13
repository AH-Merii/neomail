//! The OAuth providers, as data: endpoints, client kind, scopes, the mail API and the setup text.
//! Microsoft is another value, not another type.

use super::MailGrant;

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
    /// Microsoft Graph (`nm-graph`).
    Graph,
}

/// What `neomail account add` shows and asks for this provider.
#[derive(Debug, PartialEq, Eq)]
pub struct Setup {
    /// The account name when none is given: "gmail".
    pub default_name: &'static str,
    /// The guide to the setup, which `neomail account add` walks through step by step
    /// ([`crate::oauth::steps`]).
    pub guide: &'static str,
    /// Why a saved sign-in stops working, for the `invalid_grant` message.
    pub expired_hint: &'static str,
}

/// The `scope` a refresh sends, by what the sign-in granted. Microsoft's refresh scope must be the
/// original's or a subset
/// (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>), so a
/// sign-in that granted only reading is never refreshed with the scope that changes mail.
#[derive(Debug, PartialEq, Eq)]
pub struct RefreshScope {
    pub read: &'static str,
    pub read_write: &'static str,
    pub read_write_send: &'static str,
}

impl RefreshScope {
    pub fn for_grant(&self, grant: MailGrant) -> &'static str {
        match grant {
            MailGrant::Read => self.read,
            MailGrant::ReadWrite => self.read_write,
            MailGrant::ReadWriteSend => self.read_write_send,
        }
    }
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
    /// The grants among `mail_scopes` that also let neomail change mail (mark it read or
    /// unread), without [`Provider::scope_prefix`].
    pub write_scopes: &'static [&'static str],
    /// The grants that also let neomail send mail as the account, without
    /// [`Provider::scope_prefix`].
    pub send_scopes: &'static [&'static str],
    /// Stripped from a granted scope before it is matched against `mail_scopes`.
    pub scope_prefix: &'static str,
    /// The `scope` sent with a refresh, when the provider needs one.
    pub refresh_scope: Option<RefreshScope>,
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
    // Full access: IMAP's STORE works with it.
    write_scopes: &["https://mail.google.com/"],
    // The full mail scope also covers SMTP
    // (<https://developers.google.com/workspace/gmail/imap/xoauth2-protocol>).
    send_scopes: &["https://mail.google.com/"],
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
        guide: "https://github.com/AH-Merii/neomail/blob/main/docs/gmail.md",
        expired_hint: "an app in Testing mode gets sign-ins that last 7 days; see docs/gmail.md",
    },
};

/// Microsoft's identity platform, v2 endpoints on the `common` tenant, for work, school and
/// personal accounts, read through Microsoft Graph.
///
/// - No revocation endpoint: the v2 endpoints list none
///   (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc>).
/// - No `prompt=consent`: `offline_access` alone brings a refresh token, and the refresh scope
///   must be the original's or a subset, for the first scope's resource
///   (<https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>).
/// - Mail.ReadWrite, so neomail can mark mail read or unread
///   (<https://learn.microsoft.com/en-us/graph/api/message-update>), and Mail.Read, though
///   Mail.ReadWrite also reads: the previous release refreshes every sign-in with Mail.Read, which
///   the subset rule above then allows. A sign-in made when neomail asked for Mail.Read only
///   keeps Mail.Read: its refresh asks for no more than it granted.
/// - Mail.Send, so neomail can send with sendMail
///   (<https://learn.microsoft.com/en-us/graph/api/user-sendmail>). A sign-in from before it was
///   asked keeps Mail.ReadWrite only, for the same reason, until `neomail account login`.
pub const MICROSOFT: Provider = Provider {
    id: "microsoft",
    name: "Microsoft",
    auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    revoke_url: None,
    device_code_url: Some("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"),
    client: ClientKind::Public,
    redirect_host: "localhost",
    scope: "offline_access openid profile email https://graph.microsoft.com/Mail.Read \
            https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send",
    mail_scopes: &["Mail.Read", "Mail.ReadWrite"],
    write_scopes: &["Mail.ReadWrite"],
    send_scopes: &["Mail.Send"],
    scope_prefix: "https://graph.microsoft.com/",
    refresh_scope: Some(RefreshScope {
        read: "offline_access https://graph.microsoft.com/Mail.Read",
        read_write: "offline_access https://graph.microsoft.com/Mail.ReadWrite",
        read_write_send: "offline_access https://graph.microsoft.com/Mail.ReadWrite \
                          https://graph.microsoft.com/Mail.Send",
    }),
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
        (7000218, HINT_7000218),
    ],
    setup: Setup {
        default_name: "outlook",
        guide: "https://github.com/AH-Merii/neomail/blob/main/docs/outlook.md",
        expired_hint: "Microsoft ends a sign-in unused for 90 days, after a password change, or \
                       when access is revoked",
    },
};

/// AADSTS7000218 means Microsoft took the app for a confidential client, which has two causes
/// (<https://learn.microsoft.com/en-us/troubleshoot/entra/entra-id/app-integration/confidential-client-application-authentication-error-aadsts7000218>).
pub const HINT_7000218: &str = "the app registration counts as a confidential client: for the \
    device code sign-in, set Authentication > Advanced settings > Allow public client flows to \
    Yes; for the browser sign-in, add http://localhost under Mobile and desktop applications, \
    not Web";

pub const PROVIDERS: &[&Provider] = &[&GOOGLE, &MICROSOFT];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().copied().find(|p| p.id == id)
}
