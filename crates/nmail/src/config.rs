//! The config file and the data directory: where they are and what the config holds.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nm_imap::{CertPin, ImapConfig, Security};
use serde::Deserialize;

use crate::oauth::{self, Provider};

/// Where the config file is: `--config`, then `NMAIL_CONFIG`, then
/// `$XDG_CONFIG_HOME/neomail/config.toml`, then `~/.config/neomail/config.toml`. An empty
/// `NMAIL_CONFIG` counts as unset, like an empty `NMAIL_DATA_DIR`.
pub fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit.or_else(|| {
        env::var_os("NMAIL_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }) {
        Some(p) => Ok(p),
        None => Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("neomail/config.toml")),
    }
}

/// Where the cache lives: `NMAIL_DATA_DIR`, then `$XDG_DATA_HOME/neomail`, then
/// `~/.local/share/neomail`.
pub fn data_dir() -> Result<PathBuf> {
    match env::var_os("NMAIL_DATA_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?.join("neomail")),
    }
}

/// A TOML error as its message and position only. toml's own `Display` quotes the offending
/// line, which could echo a secret pasted into the config.
fn syntax_error(text: &str, e: &toml::de::Error) -> anyhow::Error {
    let message = e.message().trim_end();
    match e.span() {
        Some(span) => {
            let before = text.get(..span.start).unwrap_or(text);
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            anyhow::anyhow!("line {line}, column {column}: {message}")
        }
        None => anyhow::anyhow!("{message}"),
    }
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
#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub imap: ImapConfig,
    pub login: Login,
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
    /// The OS keyring entry with service `neomail` and the account name as the user.
    Keyring,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub accounts: Vec<Account>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    accounts: Vec<RawAccount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    name: String,
    username: String,
    password_command: Option<String>,
    oauth: Option<RawOAuth>,
    imap: Option<RawImap>,
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
                "no config file at {}; create one (see \"Try it\" in the README) or pass --config",
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
            if let Some(raw) = a.oauth {
                if a.password_command.is_some() {
                    bail!("account {name:?}: set either oauth or password_command, not both");
                }
                if a.imap.is_some() {
                    bail!(
                        "account {name:?}: an oauth account takes its IMAP server from the \
                         provider; remove imap"
                    );
                }
                accounts.push(oauth_account(name, a.username, raw)?);
                continue;
            }
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
                imap: ImapConfig {
                    host: imap.host.trim().to_owned(),
                    port,
                    security,
                    username: a.username,
                    cert_pin,
                },
                login: Login::Password(password),
                name,
            });
        }
        Ok(Self { accounts })
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

/// An OAuth account: the provider fixes the IMAP server (TLS, the system's trust store).
fn oauth_account(name: String, username: String, raw: RawOAuth) -> Result<Account> {
    let Some(provider) = oauth::provider(&raw.provider) else {
        bail!(
            "account {name:?}: unknown oauth provider {:?}; known: {}",
            raw.provider,
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
    Ok(Account {
        imap: ImapConfig {
            host: provider.imap_host.to_owned(),
            port: provider.imap_port,
            security: Security::Tls,
            username: username.trim().to_owned(),
            cert_pin: None,
        },
        login: Login::OAuth(OAuthSettings {
            provider,
            client_id,
        }),
        name,
    })
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
/// same directory, then renamed over the old one, keeping its permissions.
pub fn append_account(path: &Path, name: &str, block: &str) -> Result<()> {
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
        assert_eq!((work.imap.port, work.imap.security), (993, Security::Tls));
        assert!(work.imap.cert_pin.is_none());
        let bridge = &c.accounts[1];
        assert_eq!(
            (bridge.imap.port, bridge.imap.security),
            (1143, Security::StartTls)
        );
        assert!(bridge.imap.cert_pin.is_some());
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

    const GMAIL: &str = "[[accounts]]\nname = \"gmail\"\nusername = \"you@example.com\"\n\
                         oauth = { provider = \"google\", client_id = \"1234-abc.apps.googleusercontent.com\" }\n";

    #[test]
    fn oauth_accounts_take_imap_from_the_provider() {
        let c = Config::parse(GMAIL).unwrap();
        let a = &c.accounts[0];
        assert_eq!(
            (a.imap.host.as_str(), a.imap.port, a.imap.security),
            ("imap.gmail.com", 993, Security::Tls)
        );
        assert_eq!(a.imap.username, "you@example.com");
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
                "unknown oauth provider \"yahoo\"; known: google",
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
}
