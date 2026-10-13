//! The config file and the data directory: where they are and what the config holds.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nm_core::compose::Mailbox;
use nm_core::{GroupRules, LabelPrefix};
use nm_imap::smtp::SmtpConfig;
use nm_imap::{CertPin, ImapConfig, Security};
use serde::Deserialize;

use crate::oauth::{self, Provider};

/// Where the config file is: `--config`, then `NEOMAIL_CONFIG`, then
/// `$XDG_CONFIG_HOME/neomail/config.toml`, then `~/.config/neomail/config.toml`. An empty
/// `NEOMAIL_CONFIG` counts as unset, like an empty `NEOMAIL_DATA_DIR`.
pub fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match named_path(explicit) {
        Some(p) => Ok(p),
        None => Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("neomail/config.toml")),
    }
}

/// The config path the user named: `--config`, then a non-empty `NEOMAIL_CONFIG`; `None` for the
/// default path.
pub fn named_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(|| {
        env::var_os("NEOMAIL_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// Where the cache lives: `NEOMAIL_DATA_DIR`, then `$XDG_DATA_HOME/neomail`, then
/// `~/.local/share/neomail`.
pub fn data_dir() -> Result<PathBuf> {
    match env::var_os("NEOMAIL_DATA_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?.join("neomail")),
    }
}

/// The README's "Try it", for messages: the release tarball ships no docs/.
pub const GUIDE_URL: &str = "https://github.com/AH-Merii/neomail#try-it";

/// What every "no config yet" message says next.
pub const ADD_HINT: &str = "add one with `neomail account add`";

/// The config for a command that works without one (`list`, `redact`, `classify`, `key`): `None`
/// when the default file does not exist. With `strict`, a file the user named
/// (`--config`, `NEOMAIL_CONFIG`) must exist, so a typo never quietly drops a setting.
pub fn load_optional(explicit: Option<PathBuf>, strict: bool) -> Result<Option<Config>> {
    let named = named_path(explicit.clone()).is_some();
    let path = config_path(explicit)?;
    if (strict && named) || exists(&path) {
        Config::load(&path).map(Some)
    } else {
        Ok(None)
    }
}

/// Whether the config file exists, without reading or parsing it.
pub fn exists(path: &Path) -> bool {
    path.exists()
}

/// A TOML error with its place, never quoting the file.
#[derive(Debug)]
pub struct SyntaxError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for SyntaxError {}

/// A TOML error as its message and position only. toml's own `Display` quotes the offending
/// line, and serde's type, value and variant errors quote the value; either could echo a
/// secret pasted into the config.
fn syntax_error(text: &str, e: &toml::de::Error) -> anyhow::Error {
    let message = without_value(e.message().trim_end());
    match e.span() {
        Some(span) => {
            let before = text.get(..span.start).unwrap_or(text);
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            anyhow::Error::new(SyntaxError {
                line,
                column,
                message,
            })
        }
        None => anyhow::anyhow!("{message}"),
    }
}

/// serde's "invalid type: string \"x\", expected u16", "invalid value: integer `x`, expected
/// ..." and "unknown variant `x`, expected ..." with the value left out: "invalid type: string,
/// expected u16". The expected part comes from neomail's own types and never holds ", expected ".
fn without_value(message: &str) -> String {
    for prefix in ["invalid type: ", "invalid value: ", "unknown variant "] {
        let Some(rest) = message.strip_prefix(prefix) else {
            continue;
        };
        let (found, expected) = match rest.rfind(", expected ") {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        // The kind ("string", "integer") comes before the quoted or backticked value.
        let kind = found.split(['"', '`']).next().unwrap_or("").trim_end();
        let head = prefix.trim_end();
        return if kind.is_empty() {
            format!("{head}{expected}")
        } else {
            format!("{head} {kind}{expected}")
        };
    }
    message.to_owned()
}

/// An XDG base directory. Per the spec, a relative value is ignored.
fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(dir) = env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(dir);
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .with_context(|| format!("neither ${var} nor $HOME is set to an absolute path"))?;
    Ok(home.join(fallback))
}

/// One account, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub server: Server,
    pub login: Login,
    /// The address mail is sent from, when it is not the username.
    pub address: Option<String>,
    /// The name sent with the address.
    pub display_name: Option<String>,
    /// The SMTP server the config names; [`Account::outbound`] derives one for known servers.
    pub smtp: Option<SmtpSettings>,
}

/// An SMTP server for sending, as `[accounts.smtp]` says or a preset derives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpSettings {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub cert_pin: Option<CertPin>,
    /// Append a copy to the mailbox marked `\Sent` after sending: only for servers whose SMTP
    /// keeps none. Gmail and Bridge keep one, and a second would be a duplicate.
    pub save_sent: bool,
}

/// How an account sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// SMTP submission, logged in as the IMAP login is.
    Smtp { config: SmtpConfig, save_sent: bool },
    /// Microsoft Graph's sendMail, which keeps the copy in Sent Items itself.
    Graph,
}

/// The SMTP server of a known IMAP server, so an account added before neomail could send needs
/// no new config:
///
/// - Gmail: smtp.gmail.com, port 465, TLS
///   (<https://developers.google.com/workspace/gmail/imap/imap-smtp>). "Sent messages are
///   automatically copied to the Gmail/Sent folder if your email client uses SMTP"
///   (<https://support.google.com/mail/answer/78892>), so no copy is appended.
/// - Proton Mail Bridge on 127.0.0.1 port 1143 with a pinned certificate: its SMTP on port 1025
///   (<https://proton.me/support/port-already-occupied-error>) with STARTTLS and the same pin.
///   Proton tells Outlook users not to save copies of sent items
///   (<https://proton.me/support/protonmail-bridge-clients-windows-outlook-2019>), as Bridge
///   keeps them, so no copy is appended.
pub fn preset_smtp(imap: &ImapConfig) -> Option<SmtpSettings> {
    if imap.host.eq_ignore_ascii_case("imap.gmail.com") {
        return Some(SmtpSettings {
            host: "smtp.gmail.com".to_owned(),
            port: 465,
            security: Security::Tls,
            cert_pin: None,
            save_sent: false,
        });
    }
    if imap.host == "127.0.0.1" && imap.port == 1143 && imap.cert_pin.is_some() {
        return Some(SmtpSettings {
            host: "127.0.0.1".to_owned(),
            port: 1025,
            security: Security::StartTls,
            cert_pin: imap.cert_pin,
            save_sent: false,
        });
    }
    None
}

impl Account {
    /// The login name: the IMAP username, or the address signed in to Graph.
    pub fn username(&self) -> &str {
        match &self.server {
            Server::Imap(imap) => &imap.username,
            Server::Graph { username } => username,
        }
    }

    /// Who mail is sent as: `address` (else the username) and `display_name`.
    pub fn identity(&self) -> Result<Mailbox> {
        let address = self.address.as_deref().unwrap_or(self.username());
        Mailbox::new(self.display_name.as_deref(), address).map_err(|e| {
            anyhow::anyhow!(
                "account {:?} cannot send: {e}; set address = \"you@example.com\" in its \
                 [[accounts]] entry",
                self.name
            )
        })
    }

    /// How the account sends: SMTP as `[accounts.smtp]` says or as [`preset_smtp`] derives, or
    /// Graph.
    pub fn outbound(&self) -> Result<Outbound> {
        let imap = match &self.server {
            Server::Graph { .. } => return Ok(Outbound::Graph),
            Server::Imap(imap) => imap,
        };
        let Some(smtp) = self.smtp.clone().or_else(|| preset_smtp(imap)) else {
            bail!(
                "account {name} has no smtp; add an [accounts.smtp] table with its host to the \
                 config file (`neomail config path` shows where)",
                name = self.name
            );
        };
        Ok(Outbound::Smtp {
            config: SmtpConfig {
                host: smtp.host,
                port: smtp.port,
                security: smtp.security,
                username: imap.username.clone(),
                cert_pin: smtp.cert_pin,
            },
            save_sent: smtp.save_sent,
        })
    }

    /// The IMAP server, for an IMAP account. Only the tests need it; code matches on `server`.
    #[cfg(test)]
    pub fn imap(&self) -> Option<&ImapConfig> {
        match &self.server {
            Server::Imap(imap) => Some(imap),
            Server::Graph { .. } => None,
        }
    }
}

/// Where the account's mail is read. A password account is always IMAP; an OAuth account's
/// provider decides ([`oauth::MailApi`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    Imap(ImapConfig),
    /// Microsoft Graph, for the signed-in user's own mailbox.
    Graph {
        username: String,
    },
}

/// How an account logs in: with a password, or with OAuth through the user's own client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    Password(PasswordSource),
    OAuth(OAuthSettings),
}

/// An OAuth account's settings. The client secret and refresh token are in the OS keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthSettings {
    pub provider: &'static Provider,
    pub client_id: String,
}

/// Where an account's password comes from. Exactly one source per account: with
/// `password_command` set the keyring is never touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordSource {
    /// Run through `sh -c`; its output is the password. The command is not secret, its output is.
    Command(String),
    /// The OS keyring entry with service `neomail` and the account name as its `username`
    /// attribute.
    Keyring,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub accounts: Vec<Account>,
    pub classify: ClassifySettings,
}

/// The classifier jev's base URL when the config names none.
pub const DEFAULT_CLASSIFY_URL: &str = "https://api.typesafe.ai";
/// The model asked when the config names none: pinned, so stored probabilities stay comparable
/// (DEC-2026-10-13-classify).
pub const DEFAULT_CLASSIFY_MODEL: &str = "jev-1.13.0";

/// The optional `[classify]` table: where the classifier is, which model, and the thresholds
/// that turn its answers into groups.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifySettings {
    /// The backend's base URL, without a trailing `/`: `https://`, or `http://` to this machine.
    pub url: String,
    pub model: String,
    pub rules: GroupRules,
    /// Whether `neomail daemon` classifies new mail and writes groups to the server in the
    /// background: off unless the config sets it (DEC-2026-10-14-classify-when-asked). Once on,
    /// it still needs a key and the consent `neomail classify` stores.
    pub daemon: bool,
    /// The prefix of a group's name on the server, `nm-` by default; never shown inside neomail
    /// (DEC-2026-10-13-labels).
    pub label_prefix: LabelPrefix,
    /// Whether groups are written to the server: Outlook categories (DEC-2026-10-13-categories).
    pub labels: bool,
}

impl Default for ClassifySettings {
    fn default() -> Self {
        ClassifySettings {
            url: DEFAULT_CLASSIFY_URL.to_owned(),
            model: DEFAULT_CLASSIFY_MODEL.to_owned(),
            rules: GroupRules::default(),
            daemon: false,
            label_prefix: LabelPrefix::default(),
            labels: true,
        }
    }
}

impl ClassifySettings {
    /// The backend's host, which the key's keyring entry and the consent are named after.
    pub fn host(&self) -> Result<String> {
        crate::http::url_host(&self.url)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    accounts: Vec<RawAccount>,
    classify: Option<RawClassify>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClassify {
    url: Option<String>,
    model: Option<String>,
    other_below: Option<f64>,
    follow_up_old_days: Option<u32>,
    daemon: Option<bool>,
    label_prefix: Option<String>,
    labels: Option<bool>,
}

/// Checks a `[classify]` table and fills in the defaults.
fn classify_settings(raw: Option<RawClassify>) -> Result<ClassifySettings> {
    let mut settings = ClassifySettings::default();
    let Some(raw) = raw else {
        return Ok(settings);
    };
    if let Some(url) = raw.url {
        let url = url.trim().trim_end_matches('/').to_owned();
        crate::http::url_host(&url).context("classify.url")?;
        if url.contains('?') {
            bail!("classify.url must not have a query: neomail adds /v1/systemone to it");
        }
        settings.url = url;
    }
    if let Some(model) = raw.model {
        if model.trim().is_empty() {
            bail!("classify.model is empty; remove it to use {DEFAULT_CLASSIFY_MODEL}");
        }
        settings.model = model.trim().to_owned();
    }
    if let Some(other_below) = raw.other_below {
        if !(0.0..=1.0).contains(&other_below) {
            bail!("classify.other_below must be from 0 to 1");
        }
        settings.rules.other_below = other_below;
    }
    if let Some(days) = raw.follow_up_old_days {
        if days == 0 {
            bail!("classify.follow_up_old_days must be at least 1");
        }
        settings.rules.follow_up_old_days = days;
    }
    if let Some(daemon) = raw.daemon {
        settings.daemon = daemon;
    }
    if let Some(prefix) = raw.label_prefix {
        settings.label_prefix = LabelPrefix::new(&prefix)
            .map_err(|e| anyhow::anyhow!("classify.label_prefix {e}; remove it to use nm-"))?;
    }
    if let Some(labels) = raw.labels {
        settings.labels = labels;
    }
    Ok(settings)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    name: String,
    username: String,
    password_command: Option<String>,
    oauth: Option<RawOAuth>,
    imap: Option<RawImap>,
    address: Option<String>,
    display_name: Option<String>,
    smtp: Option<RawSmtp>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSmtp {
    host: String,
    port: Option<u16>,
    #[serde(default)]
    security: RawSecurity,
    cert_sha256: Option<String>,
    save_sent: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOAuth {
    provider: String,
    client_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImap {
    host: String,
    port: Option<u16>,
    #[serde(default)]
    security: RawSecurity,
    cert_sha256: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawSecurity {
    #[default]
    Tls,
    Starttls,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "no config file at {}: no accounts yet; {ADD_HINT} (guide: {GUIDE_URL}), or pass \
                 --config",
                path.display()
            ),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("invalid config file {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| syntax_error(text, &e))?;
        let mut seen = HashSet::new();
        let mut accounts = Vec::with_capacity(raw.accounts.len());
        for a in raw.accounts {
            let name = a.name.trim().to_owned();
            if name.is_empty() {
                bail!("an account has an empty name");
            }
            if !seen.insert(name.clone()) {
                bail!("two accounts are named {name:?}");
            }
            if a.username.trim().is_empty() {
                bail!("account {name:?}: username is empty");
            }
            let (address, display_name) = identity(&name, a.address, a.display_name)?;
            if let Some(raw) = a.oauth {
                if a.password_command.is_some() {
                    bail!("account {name:?}: set either oauth or password_command, not both");
                }
                for (set, key) in [(a.imap.is_some(), "imap"), (a.smtp.is_some(), "smtp")] {
                    if set {
                        bail!(
                            "account {name:?}: an oauth account takes its mail server from the \
                             provider; remove {key}"
                        );
                    }
                }
                let mut account = oauth_account(name, a.username, raw)?;
                account.address = address;
                account.display_name = display_name;
                accounts.push(account);
                continue;
            }
            let smtp = a.smtp.map(|raw| smtp_settings(&name, raw)).transpose()?;
            let Some(imap) = a.imap else {
                bail!("account {name:?}: imap is missing");
            };
            if imap.host.trim().is_empty() {
                bail!("account {name:?}: imap.host is empty");
            }
            let password = match a.password_command {
                Some(command) if command.trim().is_empty() => bail!(
                    "account {name:?}: password_command is empty; remove it to use the OS keyring"
                ),
                Some(command) => PasswordSource::Command(command),
                None => PasswordSource::Keyring,
            };
            let security = match imap.security {
                RawSecurity::Tls => Security::Tls,
                RawSecurity::Starttls => Security::StartTls,
            };
            let port = imap.port.unwrap_or(match security {
                Security::Tls => 993,
                Security::StartTls => 143,
            });
            let cert_pin = imap
                .cert_sha256
                .as_deref()
                .map(str::parse::<CertPin>)
                .transpose()
                .with_context(|| format!("account {name:?}: imap.cert_sha256"))?;
            accounts.push(Account {
                server: Server::Imap(ImapConfig {
                    host: imap.host.trim().to_owned(),
                    port,
                    security,
                    username: a.username,
                    cert_pin,
                }),
                login: Login::Password(password),
                name,
                address,
                display_name,
                smtp,
            });
        }
        Ok(Self {
            accounts,
            classify: classify_settings(raw.classify)?,
        })
    }

    pub fn find(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.name == name)
    }

    /// The named account, or every account when `name` is `None`.
    pub fn select(&self, name: Option<&str>) -> Result<Vec<&Account>> {
        if self.accounts.is_empty() {
            bail!("the config file has no [[accounts]]");
        }
        match name {
            None => Ok(self.accounts.iter().collect()),
            Some(n) => match self.accounts.iter().find(|a| a.name == n) {
                Some(a) => Ok(vec![a]),
                None => bail!(
                    "no account named {n:?}; configured: {}",
                    self.accounts
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        }
    }
}

/// An OAuth account: the provider fixes the server, IMAP (TLS, the system's trust store) or
/// Microsoft Graph.
fn oauth_account(name: String, username: String, raw: RawOAuth) -> Result<Account> {
    let Some(provider) = oauth::provider(&raw.provider) else {
        bail!(
            "account {name:?}: unknown oauth provider (the value is not shown, in case it is a \
             pasted secret); known: {}",
            oauth::PROVIDERS
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let client_id = raw.client_id.trim().to_owned();
    if client_id.is_empty() {
        bail!("account {name:?}: oauth.client_id is empty");
    }
    let username = username.trim().to_owned();
    let server = match provider.mail {
        oauth::MailApi::Imap { host, port } => Server::Imap(ImapConfig {
            host: host.to_owned(),
            port,
            security: Security::Tls,
            username,
            cert_pin: None,
        }),
        oauth::MailApi::Graph => Server::Graph { username },
    };
    Ok(Account {
        server,
        login: Login::OAuth(OAuthSettings {
            provider,
            client_id,
        }),
        name,
        address: None,
        display_name: None,
        smtp: None,
    })
}

/// An account's `address` and `display_name`, checked: an address neomail can send from, and a
/// name without line breaks.
fn identity(
    name: &str,
    address: Option<String>,
    display_name: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let address = address.map(|a| a.trim().to_owned());
    let display_name = display_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    let checked = Mailbox::new(
        display_name.as_deref(),
        address.as_deref().unwrap_or("x@example.com"),
    );
    if let Err(e) = checked {
        bail!("account {name:?}: {e}");
    }
    Ok((address, display_name))
}

/// `[accounts.smtp]`, checked. The port defaults to 465 for TLS and 587 for STARTTLS (RFC 8314
/// section 7.3, RFC 6409 section 3.1); a server named here keeps no copy of sent mail unless
/// `save_sent = false` says it does, or it is one [`keeps_sent_copy`] knows.
fn smtp_settings(name: &str, raw: RawSmtp) -> Result<SmtpSettings> {
    let host = raw.host.trim().to_owned();
    if host.is_empty() {
        bail!("account {name:?}: smtp.host is empty");
    }
    let security = match raw.security {
        RawSecurity::Tls => Security::Tls,
        RawSecurity::Starttls => Security::StartTls,
    };
    let cert_pin = raw
        .cert_sha256
        .as_deref()
        .map(str::parse::<CertPin>)
        .transpose()
        .with_context(|| format!("account {name:?}: smtp.cert_sha256"))?;
    Ok(SmtpSettings {
        port: raw.port.unwrap_or(match security {
            Security::Tls => 465,
            Security::StartTls => 587,
        }),
        security,
        cert_pin,
        save_sent: raw.save_sent.unwrap_or(!keeps_sent_copy(&host)),
        host,
    })
}

/// Whether the SMTP server at `host` keeps its own copy of what it sends, so neomail appends
/// none: Gmail's ("Sent messages are automatically copied to the Gmail/Sent folder if your email
/// client uses SMTP", <https://support.google.com/mail/answer/78892>).
pub fn keeps_sent_copy(host: &str) -> bool {
    host.eq_ignore_ascii_case("smtp.gmail.com")
}

/// The TOML for a new OAuth account, ready to append to the config file.
pub fn oauth_account_toml(
    name: &str,
    username: &str,
    provider: &Provider,
    client_id: &str,
) -> String {
    let quote = |s: &str| toml::Value::String(s.to_owned()).to_string();
    format!(
        "[[accounts]]\nname = {}\nusername = {}\noauth = {{ provider = {}, client_id = {} }}\n",
        quote(name),
        quote(username),
        quote(provider.id),
        quote(client_id)
    )
}

/// The TOML for a new password account, ready to append to the config file. Without
/// `password_command` the password is in the OS keyring. `smtp`, when given, is written out in
/// full, so the config shows how the account sends.
pub fn password_account_toml(
    name: &str,
    imap: &ImapConfig,
    password_command: Option<&str>,
    smtp: Option<&SmtpSettings>,
) -> String {
    let quote = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let mut block = format!(
        "[[accounts]]\nname = {}\nusername = {}\n",
        quote(name),
        quote(&imap.username)
    );
    if let Some(command) = password_command {
        block.push_str(&format!("password_command = {}\n", quote(command)));
    }
    block.push_str(&format!(
        "[accounts.imap]\nhost = {}\nport = {}\nsecurity = {}\n",
        quote(&imap.host),
        imap.port,
        quote(security_name(imap.security))
    ));
    if let Some(pin) = imap.cert_pin {
        block.push_str(&format!("cert_sha256 = {}\n", quote(&pin.to_string())));
    }
    if let Some(smtp) = smtp {
        block.push_str(&format!(
            "[accounts.smtp]\nhost = {}\nport = {}\nsecurity = {}\nsave_sent = {}\n",
            quote(&smtp.host),
            smtp.port,
            quote(security_name(smtp.security)),
            smtp.save_sent
        ));
        if let Some(pin) = smtp.cert_pin {
            block.push_str(&format!("cert_sha256 = {}\n", quote(&pin.to_string())));
        }
    }
    block
}

fn security_name(security: Security) -> &'static str {
    match security {
        Security::Tls => "tls",
        Security::StartTls => "starttls",
    }
}

/// The config file's text, or `None` when there is no file yet.
fn read_existing(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Fails unless an account named `name` could be added to the config file at `path`: the file
/// is missing, or it parses and has no account by that name.
pub fn check_can_add(path: &Path, name: &str) -> Result<()> {
    if let Some(text) = read_existing(path)? {
        let config = Config::parse(&text)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        if config.find(name).is_some() {
            bail!(
                "{} already has an account named {name:?}; choose another --name",
                path.display()
            );
        }
    }
    Ok(())
}

/// Appends `block` (one `[[accounts]]` table named `name`) to the config file, keeping its text
/// and comments. A missing file is created with mode 0600 in a directory created with mode 0700.
/// The result is parsed before it is written, and written atomically: to a temporary file in the
/// same directory, then renamed over the old one, keeping its permissions. A symlinked config
/// (stow, home-manager, chezmoi) is written at its target, so the link stays.
pub fn append_account(path: &Path, name: &str, block: &str) -> Result<()> {
    write_account(path, name, block, true)
}

/// Fails unless [`append_account`] would succeed: everything but the write itself, so a
/// config that cannot take the account (an inline `accounts = [...]`, say) or a directory that
/// cannot be written is found before the sign-in.
pub fn check_append(path: &Path, name: &str, block: &str) -> Result<()> {
    write_account(path, name, block, false)
}

/// The file a path names, following symlinks; the path itself when it is not a link. Unlike
/// `canonicalize`, a link to a file that does not exist yet resolves too.
fn resolve_links(path: &Path) -> Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&path)
                    .with_context(|| format!("cannot read the link {}", path.display()))?;
                path = match path.parent() {
                    Some(dir) => dir.join(target),
                    None => target,
                };
            }
            _ => return Ok(path),
        }
    }
    bail!("{}: too many levels of symbolic links", path.display())
}

fn write_account(path: &Path, name: &str, block: &str, commit: bool) -> Result<()> {
    let path = &resolve_links(path)?;
    check_can_add(path, name)?;
    let existing = read_existing(path)?;
    let mut text = existing.clone().unwrap_or_default();
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(block);
    let parsed = Config::parse(&text).context("the new account does not parse")?;
    if parsed.find(name).is_none() {
        bail!("the new account does not parse as {name:?}");
    }

    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;

    let file_name = path
        .file_name()
        .context("the config path has no file name")?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    let write = || -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        if existing.is_some() {
            let perms = std::fs::metadata(path)?.permissions();
            file.set_permissions(perms)?;
        }
        if !commit {
            drop(file);
            std::fs::remove_file(&tmp)?;
            return Ok(());
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(())
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[[accounts]]
name = "work"
username = "user@mail.example"
password_command = "pass show mail/work"
imap = { host = "imap.mail.example" }

[[accounts]]
name = "bridge"
username = "user@proton.example"
password_command = "secret-tool lookup service bridge"
[accounts.imap]
host = "127.0.0.1"
port = 1143
security = "starttls"
cert_sha256 = "00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff"
"#;

    #[test]
    fn parses_accounts_with_defaults() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.accounts.len(), 2);
        let work = &c.accounts[0];
        assert_eq!(
            (work.imap().unwrap().port, work.imap().unwrap().security),
            (993, Security::Tls)
        );
        assert!(work.imap().unwrap().cert_pin.is_none());
        let bridge = &c.accounts[1];
        assert_eq!(
            (bridge.imap().unwrap().port, bridge.imap().unwrap().security),
            (1143, Security::StartTls)
        );
        assert!(bridge.imap().unwrap().cert_pin.is_some());
        assert_eq!(
            work.login,
            Login::Password(PasswordSource::Command("pass show mail/work".into()))
        );
        assert_eq!(c.select(Some("bridge")).unwrap()[0].name, "bridge");
        assert_eq!(c.select(None).unwrap().len(), 2);
        assert!(c.select(Some("nope")).is_err());
    }

    #[test]
    fn rejects_bad_configs() {
        let base = "[[accounts]]\nname = \"a\"\nusername = \"u\"\npassword_command = \"true\"\n";
        for bad in [
            format!("{base}imap = {{ host = \"h\", security = \"plain\" }}"),
            format!("{base}imap = {{ host = \"h\", cert_sha256 = \"abc\" }}"),
            format!("{base}imap = {{ host = \"h\" }}\nverify = false"),
            format!("{base}imap = {{ host = \"h\" }}\n{base}imap = {{ host = \"h\" }}"),
            format!("{base}imap = {{ host = \"\" }}"),
            base.to_owned(),
            format!(
                "{}imap = {{ host = \"h\" }}",
                base.replace("\"true\"", "\" \"")
            ),
        ] {
            assert!(Config::parse(&bad).is_err(), "{bad}");
        }
        assert!(Config::parse("").unwrap().select(None).is_err());
    }

    #[test]
    fn no_password_command_means_the_keyring() {
        let c = Config::parse(
            "[[accounts]]\nname = \"a\"\nusername = \"u\"\nimap = { host = \"h\" }\n",
        )
        .unwrap();
        assert_eq!(
            c.accounts[0].login,
            Login::Password(PasswordSource::Keyring)
        );
    }

    #[test]
    fn syntax_errors_name_the_place_but_never_quote_the_file() {
        let secret = "hunter2-not-a-real-secret";
        let text = format!("[[accounts]]\nname = \"a\"\npassword = {secret}\n");
        let message = format!("{:#}", Config::parse(&text).unwrap_err());
        assert!(!message.contains(secret), "{message}");
        assert!(message.starts_with("line 3, column "), "{message}");
    }

    #[test]
    fn type_value_and_variant_errors_never_quote_the_value() {
        let secret = "hunter2-not-a-real-secret";
        let base = "[[accounts]]\nname = \"a\"\nusername = \"u\"\n[accounts.imap]\nhost = \"h\"\n";
        for (field, expected) in [
            (
                format!("port = \"{secret}\""),
                "invalid type: string, expected u16",
            ),
            (
                "port = 70000".to_owned(),
                "invalid value: integer, expected u16",
            ),
            (
                format!("security = \"{secret}\""),
                "unknown variant, expected `tls` or `starttls`",
            ),
            (
                format!("cert_sha256 = [\"{secret}\"]"),
                "invalid type: sequence, expected a string",
            ),
        ] {
            let text = format!("{base}{field}\n");
            let message = format!("{:#}", Config::parse(&text).unwrap_err());
            assert!(!message.contains(secret), "{message}");
            assert!(message.ends_with(expected), "{field}: {message}");
        }
        assert_eq!(
            without_value("missing field `host`"),
            "missing field `host`"
        );
    }

    const GMAIL: &str = "[[accounts]]\nname = \"gmail\"\nusername = \"you@example.com\"\n\
                         oauth = { provider = \"google\", client_id = \"1234-abc.apps.googleusercontent.com\" }\n";

    #[test]
    fn oauth_accounts_take_imap_from_the_provider() {
        let c = Config::parse(GMAIL).unwrap();
        let a = &c.accounts[0];
        assert_eq!(
            (
                a.imap().unwrap().host.as_str(),
                a.imap().unwrap().port,
                a.imap().unwrap().security
            ),
            ("imap.gmail.com", 993, Security::Tls)
        );
        assert_eq!(a.imap().unwrap().username, "you@example.com");
        let Login::OAuth(o) = &a.login else {
            panic!("{:?}", a.login)
        };
        assert_eq!(
            (o.provider.id, o.client_id.as_str()),
            ("google", "1234-abc.apps.googleusercontent.com")
        );
        for (bad, why) in [
            (
                GMAIL.replace("oauth =", "password_command = \"true\"\noauth ="),
                "not both",
            ),
            (
                GMAIL.replace("google", "yahoo"),
                "unknown oauth provider (the value is not shown, in case it is a pasted secret); \
                 known: google, microsoft",
            ),
            (format!("{GMAIL}imap = {{ host = \"h\" }}\n"), "remove imap"),
            (
                GMAIL.replace("1234-abc.apps.googleusercontent.com", " "),
                "client_id is empty",
            ),
            (GMAIL.replace("provider", "kind"), "unknown field"),
        ] {
            let e = format!("{:#}", Config::parse(&bad).unwrap_err());
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    const OUTLOOK: &str = "[[accounts]]\nname = \"outlook\"\nusername = \"you@outlook.example\"\n\
                           oauth = { provider = \"microsoft\", client_id = \"00000000-0000-0000-0000-000000000000\" }\n";

    #[test]
    fn a_microsoft_account_reads_through_graph_without_an_imap_server() {
        let c = Config::parse(OUTLOOK).unwrap();
        let a = &c.accounts[0];
        assert!(
            matches!(&a.server, Server::Graph { username } if username == "you@outlook.example"),
            "{:?}",
            a.server
        );
        assert_eq!(a.username(), "you@outlook.example");
        assert!(a.imap().is_none());
        let Login::OAuth(o) = &a.login else {
            panic!("{:?}", a.login)
        };
        assert_eq!(o.provider.id, "microsoft");
        // A password account is always IMAP: a password with the microsoft provider, or a
        // Graph account given an IMAP server, is refused.
        for (bad, why) in [
            (
                OUTLOOK.replace("oauth =", "password_command = \"true\"\noauth ="),
                "not both",
            ),
            (
                format!("{OUTLOOK}imap = {{ host = \"outlook.office365.com\" }}\n"),
                "remove imap",
            ),
        ] {
            let e = format!("{:#}", Config::parse(&bad).unwrap_err());
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn a_graph_account_round_trips_through_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let block = oauth_account_toml(
            "outlook",
            "you@outlook.example",
            &crate::oauth::MICROSOFT,
            "00000000-0000-0000-0000-000000000000",
        );
        append_account(&path, "outlook", &block).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, OUTLOOK);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn writer_creates_a_private_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("neomail/config.toml");
        let block = oauth_account_toml(
            "gmail",
            "you@example.com",
            &oauth::GOOGLE,
            "1234-abc.apps.googleusercontent.com",
        );
        append_account(&path, "gmail", &block).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), GMAIL);
        #[cfg(unix)]
        assert_eq!((mode(&path), mode(path.parent().unwrap())), (0o600, 0o700));
    }

    #[test]
    fn writer_keeps_text_and_comments_and_refuses_a_taken_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let before = "# my accounts\n[[accounts]]\nname = \"work\" # the office\nusername = \"u\"\n\
                      imap = { host = \"h\" }";
        std::fs::write(&path, before).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o640))
            .unwrap();
        let block = oauth_account_toml("gmail", "you@example.com", &oauth::GOOGLE, "id \"quoted\"");
        append_account(&path, "gmail", &block).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.starts_with(&format!("{before}\n\n[[accounts]]\nname = \"gmail\"")),
            "{after}"
        );
        let c = Config::parse(&after).unwrap();
        let Login::OAuth(o) = &c.find("gmail").unwrap().login else {
            panic!()
        };
        assert_eq!(o.client_id, "id \"quoted\"");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o640, "an existing file keeps its mode");

        for name in ["work", "gmail"] {
            let e = format!(
                "{:#}",
                append_account(&path, name, &block.replace("gmail", name)).unwrap_err()
            );
            assert!(
                e.contains(&format!("already has an account named {name:?}")),
                "{e}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
        // A block that would not parse is never written.
        assert!(append_account(&path, "x", "[[accounts]]\nname = \"x\"\n").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temporary file left"
        );
    }

    #[cfg(unix)]
    #[test]
    fn writer_follows_a_symlinked_config_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("dotfiles/neomail.toml");
        std::fs::create_dir(managed.parent().unwrap()).unwrap();
        std::fs::write(&managed, "# managed\n").unwrap();
        let link = dir.path().join("config/neomail/config.toml");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../dotfiles/neomail.toml", &link).unwrap();
        let block = oauth_account_toml(
            "gmail",
            "you@example.com",
            &oauth::GOOGLE,
            "1234-abc.apps.googleusercontent.com",
        );
        append_account(&link, "gmail", &block).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&managed).unwrap(),
            format!("# managed\n\n{GMAIL}")
        );
        assert_eq!(
            std::fs::read_dir(link.parent().unwrap()).unwrap().count(),
            1
        );
        assert_eq!(
            std::fs::read_dir(managed.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn check_append_finds_what_the_write_would_hit_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let block = oauth_account_toml("gmail", "you@example.com", &oauth::GOOGLE, "id");
        check_append(&path, "gmail", &block).unwrap();
        assert!(!path.exists());
        let inline =
            "accounts = [{ name = \"work\", username = \"u\", imap = { host = \"h\" } }]\n";
        std::fs::write(&path, inline).unwrap();
        assert!(check_append(&path, "gmail", &block).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), inline);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn password_account_toml_round_trips_through_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let pin: CertPin = "ab".repeat(32).parse().unwrap();
        let imap = ImapConfig {
            host: "127.0.0.1".into(),
            port: 1143,
            security: Security::StartTls,
            username: "you@example.com".into(),
            cert_pin: Some(pin),
        };
        let smtp = preset_smtp(&imap);
        let block =
            password_account_toml("proton", &imap, Some("pass show \"bridge\""), smtp.as_ref());
        assert_eq!(
            block,
            format!(
                "[[accounts]]\nname = \"proton\"\nusername = \"you@example.com\"\n\
                 password_command = 'pass show \"bridge\"'\n[accounts.imap]\nhost = \"127.0.0.1\"\n\
                 port = 1143\nsecurity = \"starttls\"\ncert_sha256 = \"{pin}\"\n\
                 [accounts.smtp]\nhost = \"127.0.0.1\"\nport = 1025\nsecurity = \"starttls\"\n\
                 save_sent = false\ncert_sha256 = \"{pin}\"\n",
                pin = "ab".repeat(32)
            )
        );
        append_account(&path, "proton", &block).unwrap();
        let plain = ImapConfig {
            host: "imap.mail.example".into(),
            port: 993,
            security: Security::Tls,
            username: "user@mail.example".into(),
            cert_pin: None,
        };
        let block = password_account_toml("work", &plain, None, None);
        append_account(&path, "work", &block).unwrap();
        let c = Config::load(&path).unwrap();
        let proton = c.find("proton").unwrap();
        assert_eq!(
            (
                proton.imap().unwrap().port,
                proton.imap().unwrap().security,
                proton.imap().unwrap().cert_pin
            ),
            (1143, Security::StartTls, Some(pin))
        );
        assert_eq!(
            proton.login,
            Login::Password(PasswordSource::Command("pass show \"bridge\"".into()))
        );
        let work = c.find("work").unwrap();
        assert_eq!(
            (
                work.imap().unwrap().host.as_str(),
                work.imap().unwrap().port,
                work.imap().unwrap().cert_pin
            ),
            ("imap.mail.example", 993, None)
        );
        assert_eq!(work.login, Login::Password(PasswordSource::Keyring));
        assert_eq!(proton.smtp, smtp);
    }

    /// One `[[accounts]]` entry: `extra` lines after the name, then `tables`.
    fn account(extra: &str, tables: &str) -> Result<Account> {
        let text = format!("[[accounts]]\nname = \"a\"\n{extra}\n{tables}");
        Ok(Config::parse(&text)?.accounts.remove(0))
    }

    const IMAP_ACCOUNT: &str = "username = \"me@example.com\"\npassword_command = \"true\"";

    #[test]
    fn mail_is_sent_as_the_address_else_the_username() {
        let imap = "[accounts.imap]\nhost = \"imap.example.com\"";
        let me = account(IMAP_ACCOUNT, imap).unwrap().identity().unwrap();
        assert_eq!(me.to_string(), "me@example.com");
        let named = "display_name = \" Zoë Ødegård \"\naddress = \"zoe@example.com\"";
        let me = account(&format!("{IMAP_ACCOUNT}\n{named}"), imap)
            .unwrap()
            .identity()
            .unwrap();
        assert_eq!(me.to_string(), "Zoë Ødegård <zoe@example.com>");
        // A login name that is not an address needs `address`, and says so when sending.
        let bare = "username = \"me\"\npassword_command = \"true\"";
        let e = account(bare, imap).unwrap().identity().unwrap_err();
        assert!(e.to_string().contains("set address = "), "{e}");
        for bad in [
            "address = \"not an address\"",
            "address = \"me@example.com\\r\\nBcc: x@example.net\"",
            "display_name = \"Me\\r\\nBcc: x@example.net\"",
        ] {
            let e = account(&format!("{IMAP_ACCOUNT}\n{bad}"), imap).unwrap_err();
            assert!(format!("{e:#}").contains("account \"a\""), "{bad}: {e:#}");
        }
    }

    #[test]
    fn each_account_sends_through_its_server_or_its_providers_preset() {
        let pin = "ab".repeat(32);
        let smtp = |host: &str, port, security, cert_pin, save_sent| Outbound::Smtp {
            config: SmtpConfig {
                host: host.into(),
                port,
                security,
                username: "me@example.com".into(),
                cert_pin,
            },
            save_sent,
        };
        let gmail = smtp("smtp.gmail.com", 465, Security::Tls, None, false);
        let bridge_imap = format!(
            "[accounts.imap]\nhost = \"127.0.0.1\"\nport = 1143\nsecurity = \"starttls\"\n\
             cert_sha256 = \"{pin}\""
        );
        let google =
            "username = \"me@example.com\"\noauth = { provider = \"google\", client_id = \"i\" }";
        let microsoft = "username = \"me@example.com\"\noauth = { provider = \"microsoft\", client_id = \"i\" }";
        let cases = [
            (google, String::new(), gmail.clone()),
            (
                IMAP_ACCOUNT,
                "[accounts.imap]\nhost = \"imap.gmail.com\"".into(),
                gmail,
            ),
            (
                IMAP_ACCOUNT,
                bridge_imap,
                smtp(
                    "127.0.0.1",
                    1025,
                    Security::StartTls,
                    pin.parse().ok(),
                    false,
                ),
            ),
            (
                IMAP_ACCOUNT,
                "[accounts.imap]\nhost = \"imap.example.com\"\n\
                 [accounts.smtp]\nhost = \"smtp.example.com\"\nsecurity = \"starttls\""
                    .into(),
                smtp("smtp.example.com", 587, Security::StartTls, None, true),
            ),
            (
                IMAP_ACCOUNT,
                format!(
                    "[accounts.imap]\nhost = \"imap.gmail.com\"\n[accounts.smtp]\n\
                     host = \"mail.example.com\"\nsave_sent = false\ncert_sha256 = \"{pin}\""
                ),
                smtp(
                    "mail.example.com",
                    465,
                    Security::Tls,
                    pin.parse().ok(),
                    false,
                ),
            ),
            // Gmail keeps its own copy, so an smtp table naming it appends none, unless the
            // config says otherwise.
            (
                IMAP_ACCOUNT,
                "[accounts.imap]\nhost = \"imap.example.com\"\n\
                 [accounts.smtp]\nhost = \"SMTP.gmail.com\"\nport = 587\nsecurity = \"starttls\""
                    .into(),
                smtp("SMTP.gmail.com", 587, Security::StartTls, None, false),
            ),
            (
                IMAP_ACCOUNT,
                "[accounts.imap]\nhost = \"imap.gmail.com\"\n\
                 [accounts.smtp]\nhost = \"smtp.gmail.com\"\nsave_sent = true"
                    .into(),
                smtp("smtp.gmail.com", 465, Security::Tls, None, true),
            ),
            (microsoft, String::new(), Outbound::Graph),
        ];
        for (extra, tables, want) in cases {
            let a = account(extra, &tables).unwrap();
            assert_eq!(a.outbound().unwrap(), want, "{extra} {tables}");
        }
    }

    #[test]
    fn an_account_with_no_known_smtp_says_how_to_add_one() {
        let pin = "ab".repeat(32);
        for tables in [
            "[accounts.imap]\nhost = \"imap.example.com\"".to_owned(),
            // Bridge's port without a pin is not Bridge.
            "[accounts.imap]\nhost = \"127.0.0.1\"\nport = 1143\nsecurity = \"starttls\"".into(),
            format!("[accounts.imap]\nhost = \"127.0.0.2\"\nport = 1143\ncert_sha256 = \"{pin}\""),
        ] {
            let e = account(IMAP_ACCOUNT, &tables)
                .unwrap()
                .outbound()
                .unwrap_err();
            assert!(
                e.to_string()
                    .starts_with("account a has no smtp; add an [accounts.smtp]"),
                "{e}"
            );
        }
        let google =
            "username = \"me@example.com\"\noauth = { provider = \"google\", client_id = \"i\" }";
        let e = account(google, "[accounts.smtp]\nhost = \"smtp.example.com\"").unwrap_err();
        assert!(e.to_string().contains("remove smtp"), "{e}");
        let e = account(
            IMAP_ACCOUNT,
            "[accounts.imap]\nhost = \"h\"\n[accounts.smtp]\nhost = \" \"",
        )
        .unwrap_err();
        assert!(e.to_string().contains("smtp.host is empty"), "{e}");
    }

    #[test]
    fn the_classify_table_is_optional_and_checked() {
        let accounts = "[[accounts]]\nname = \"a\"\nusername = \"u\"\npassword_command = \"true\"\n\
                        [accounts.imap]\nhost = \"h\"\n";
        let c = Config::parse(accounts).unwrap();
        assert_eq!(c.classify.url, "https://api.typesafe.ai");
        assert_eq!(c.classify.model, "jev-1.13.0");
        assert_eq!(
            (
                c.classify.rules.other_below,
                c.classify.rules.follow_up_old_days
            ),
            (0.4, 90)
        );
        assert_eq!(c.classify.host().unwrap(), "api.typesafe.ai");
        assert!(
            !c.classify.daemon,
            "the daemon classifies only when the config says so"
        );
        assert_eq!(c.classify.label_prefix.as_str(), "nm-");
        assert!(c.classify.labels, "groups are written unless told not to");

        let c = Config::parse(&format!(
            "{accounts}[classify]\nurl = \"http://127.0.0.1:8080/\"\nmodel = \"jev-preview\"\n\
             other_below = 0.55\nfollow_up_old_days = 30\ndaemon = true\n\
             label_prefix = \"neo/\"\nlabels = false\n"
        ))
        .unwrap();
        assert!(c.classify.daemon);
        assert_eq!(c.classify.label_prefix.as_str(), "neo/");
        assert!(!c.classify.labels);
        assert_eq!(c.classify.url, "http://127.0.0.1:8080");
        assert_eq!(c.classify.model, "jev-preview");
        assert_eq!(
            (
                c.classify.rules.other_below,
                c.classify.rules.follow_up_old_days
            ),
            (0.55, 30)
        );
        assert_eq!(c.classify.host().unwrap(), "127.0.0.1");
        assert!(
            Config::parse("[classify]\n").is_ok(),
            "an empty table is the defaults"
        );

        for (bad, says) in [
            ("url = \"http://jev.example\"", "classify.url"),
            ("url = \"jev.example\"", "classify.url"),
            (
                "url = \"https://jev.example/x?y=1\"",
                "must not have a query",
            ),
            ("model = \" \"", "classify.model is empty"),
            ("other_below = 1.5", "from 0 to 1"),
            ("other_below = -0.1", "from 0 to 1"),
            ("follow_up_old_days = 0", "at least 1"),
            ("threshold = 0.5", "unknown field"),
            ("daemon = \"no\"", "invalid type"),
            (
                "label_prefix = \"\"",
                "classify.label_prefix must not be empty",
            ),
            (
                "label_prefix = \"nm\\t\"",
                "must not hold control characters",
            ),
            ("labels = \"yes\"", "invalid type"),
        ] {
            let e = format!(
                "{:#}",
                Config::parse(&format!("{accounts}[classify]\n{bad}\n")).unwrap_err()
            );
            assert!(e.contains(says), "{bad}: {e}");
        }
    }
}
