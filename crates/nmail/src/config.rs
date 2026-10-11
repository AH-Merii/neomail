//! The config file and the data directory: where they are and what the config holds.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nm_imap::{CertPin, ImapConfig, Security};
use serde::Deserialize;

/// Where the config file is: `--config` or `NMAIL_CONFIG` (both handled by clap) win, then
/// `$XDG_CONFIG_HOME/neomail/config.toml`, then `~/.config/neomail/config.toml`.
pub fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
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
    /// Run through `sh -c`; its output is the password. The command is not secret, its output is.
    pub password_command: String,
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
    password_command: String,
    imap: RawImap,
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
        let raw: RawConfig = toml::from_str(text)?;
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
            if a.imap.host.trim().is_empty() {
                bail!("account {name:?}: imap.host is empty");
            }
            if a.password_command.trim().is_empty() {
                bail!("account {name:?}: password_command is empty");
            }
            let security = match a.imap.security {
                RawSecurity::Tls => Security::Tls,
                RawSecurity::Starttls => Security::StartTls,
            };
            let port = a.imap.port.unwrap_or(match security {
                Security::Tls => 993,
                Security::StartTls => 143,
            });
            let cert_pin = a
                .imap
                .cert_sha256
                .as_deref()
                .map(str::parse::<CertPin>)
                .transpose()
                .with_context(|| format!("account {name:?}: imap.cert_sha256"))?;
            accounts.push(Account {
                imap: ImapConfig {
                    host: a.imap.host.trim().to_owned(),
                    port,
                    security,
                    username: a.username,
                    cert_pin,
                },
                password_command: a.password_command,
                name,
            });
        }
        Ok(Self { accounts })
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
        ] {
            assert!(Config::parse(&bad).is_err(), "{bad}");
        }
        assert!(Config::parse("").unwrap().select(None).is_err());
    }
}
