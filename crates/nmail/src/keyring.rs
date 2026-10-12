//! The OS keyring: the Secret Service over D-Bus on Linux (zbus, pure Rust), the login keychain
//! on macOS. A password account's entry has the service `neomail` and the account name as the
//! user; an OAuth account's has the service `neomail-oauth` and holds its client secret and
//! refresh token as JSON, so the two kinds can never overwrite each other.
//!
//! Errors are written here rather than passed through: keyring-core's `BadEncoding` and
//! `BadDataFormat` carry the stored bytes, so they are never formatted with `{:?}`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use keyring_core::{CredentialStore, Entry, Error};
use serde::{Deserialize, Serialize};

use crate::oauth::Secret;
use crate::secret::Password;

/// The keyring service passwords are stored under.
pub const SERVICE: &str = "neomail";
/// The keyring service OAuth sign-ins are stored under.
pub const OAUTH_SERVICE: &str = "neomail-oauth";

/// An OAuth account's keyring entry: the user's client secret and, once signed in, the refresh
/// token. Stored as JSON; `Debug` is redacted.
#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthEntry {
    pub client_secret: String,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for OAuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthEntry(<redacted>)")
    }
}

impl OAuthEntry {
    pub fn client_secret(&self) -> Secret {
        Secret::new(self.client_secret.clone())
    }

    pub fn refresh_token(&self) -> Option<Secret> {
        self.refresh_token.clone().map(Secret::new)
    }
}

/// The platform's credential store, chosen once when it is opened.
pub struct Keyring {
    store: Arc<CredentialStore>,
    /// Where the password goes, for messages: "the Secret Service", "the macOS keychain".
    name: &'static str,
    wsl: bool,
}

impl Keyring {
    /// Opens the platform's store. On Linux this connects to the Secret Service on the D-Bus
    /// session bus. zbus's blocking calls run on a tokio runtime of their own, so neither this
    /// nor any other method may be called from inside a tokio runtime: it would panic.
    pub fn open() -> Result<Self> {
        Self::open_for(SERVICE)
    }

    /// Like [`Keyring::open`], with the hint for OAuth accounts when it is unavailable.
    pub fn open_for_oauth() -> Result<Self> {
        Self::open_for(OAUTH_SERVICE)
    }

    fn open_for(service: &str) -> Result<Self> {
        let wsl = is_wsl();
        match platform_store() {
            Ok(store) => Ok(Self::with_store(store, PLATFORM_NAME, wsl)),
            Err(e) => Err(error(&e, service, "", wsl)),
        }
    }

    pub fn with_store(store: Arc<CredentialStore>, name: &'static str, wsl: bool) -> Self {
        Self { store, name, wsl }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn entry(&self, service: &str, account: &str) -> Result<Entry> {
        self.store
            .build(service, account, None)
            .map_err(|e| error(&e, service, account, self.wsl))
    }

    pub fn get(&self, account: &str) -> Result<Password> {
        match self.entry(SERVICE, account)?.get_password() {
            Ok(password) => Ok(Password::new(password)),
            Err(Error::NoEntry) => Err(anyhow!(
                "no password in {} (service {SERVICE:?}, user {account:?}); run \
                 `nmail account set-password {account}` or set password_command",
                self.name
            )),
            Err(e) => Err(error(&e, SERVICE, account, self.wsl)),
        }
    }

    pub fn set(&self, account: &str, password: &Password) -> Result<()> {
        self.entry(SERVICE, account)?
            .set_password(password.expose())
            .map_err(|e| error(&e, SERVICE, account, self.wsl))
    }

    /// Deletes the password entry. `false` when there was none.
    pub fn delete(&self, account: &str) -> Result<bool> {
        self.delete_entry(SERVICE, account)
    }

    /// The OAuth entry, or `None` when there is none.
    pub fn get_oauth(&self, account: &str) -> Result<Option<OAuthEntry>> {
        match self.entry(OAUTH_SERVICE, account)?.get_password() {
            // serde_json's errors can quote the input: never pass them on.
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|_| anyhow!("the keyring entry for {account:?} is malformed")),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(error(&e, OAUTH_SERVICE, account, self.wsl)),
        }
    }

    pub fn set_oauth(&self, account: &str, entry: &OAuthEntry) -> Result<()> {
        let json = serde_json::to_string(entry)
            .map_err(|_| anyhow!("cannot encode the keyring entry for {account:?}"))?;
        self.entry(OAUTH_SERVICE, account)?
            .set_password(&json)
            .map_err(|e| error(&e, OAUTH_SERVICE, account, self.wsl))
    }

    /// Deletes the OAuth entry. `false` when there was none.
    pub fn delete_oauth(&self, account: &str) -> Result<bool> {
        self.delete_entry(OAUTH_SERVICE, account)
    }

    fn delete_entry(&self, service: &str, account: &str) -> Result<bool> {
        match self.entry(service, account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(Error::NoEntry) => Ok(false),
            Err(e) => Err(error(&e, service, account, self.wsl)),
        }
    }
}

#[cfg(target_os = "linux")]
const PLATFORM_NAME: &str = "the Secret Service";
#[cfg(target_os = "macos")]
const PLATFORM_NAME: &str = "the macOS keychain";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const PLATFORM_NAME: &str = "the OS keyring";

#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    let store: Arc<CredentialStore> = zbus_secret_service_keyring_store::Store::new()?;
    Ok(store)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()?;
    Ok(store)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NotSupportedByStore(
        "neomail supports the keyrings of Linux and macOS only".to_owned(),
    ))
}

/// WSL sets `WSL_DISTRO_NAME` in its shells; `WSLInterop` exists even where it does not.
pub fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
}

/// One line for a keyring error, never holding the stored data. `account` is empty when no
/// entry was involved yet; `service` picks the hint.
fn error(e: &Error, service: &str, account: &str, wsl: bool) -> anyhow::Error {
    let detail = match e {
        Error::PlatformFailure(inner) | Error::NoStorageAccess(inner) => inner.to_string(),
        Error::NoDefaultStore | Error::NotSupportedByStore(_) => e.to_string(),
        Error::BadEncoding(_) => {
            return anyhow!("the keyring entry for {account:?} is not valid UTF-8");
        }
        Error::BadDataFormat(..) => {
            return anyhow!("the keyring entry for {account:?} is malformed");
        }
        Error::Ambiguous(items) => {
            return anyhow!(
                "{} keyring items match service {service:?} and user {account:?}; delete all but one",
                items.len()
            );
        }
        Error::Invalid(..) | Error::TooLong(..) | Error::BadStoreFormat(_) => {
            return anyhow!("the keyring refused the entry for {account:?}: {e}");
        }
        _ => return anyhow!("the keyring failed for {account:?}"),
    };
    // Keep it on one line: a D-Bus error can span several.
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if service == OAUTH_SERVICE {
        let wsl_hint = if wsl {
            "; WSL often has no Secret Service: run one such as gnome-keyring"
        } else {
            ""
        };
        return anyhow!(
            "the OS keyring is unavailable ({detail}); OAuth sign-ins are kept only there, so \
             start a Secret Service (GNOME Keyring, KeePassXC){wsl_hint}"
        );
    }
    let wsl_hint = if wsl {
        "; WSL often has no Secret Service: use password_command (for example with pass) or \
         run a Secret Service such as gnome-keyring"
    } else {
        ""
    };
    anyhow!(
        "the OS keyring is unavailable ({detail}); set password_command in the config instead{wsl_hint}"
    )
}

#[cfg(test)]
mod tests {
    use keyring_core::mock;

    use super::*;

    const SECRET: &str = "hunter2-not-a-real-secret";

    fn mock_keyring(wsl: bool) -> (Keyring, Arc<mock::Store>) {
        let store = mock::Store::new().unwrap();
        (Keyring::with_store(store.clone(), "the mock", wsl), store)
    }

    /// Makes the next call on `account`'s password entry fail with `e`.
    fn fail_next(store: &mock::Store, account: &str, e: Error) {
        fail_next_in(store, SERVICE, account, e);
    }

    fn fail_next_in(store: &mock::Store, service: &str, account: &str, e: Error) {
        use keyring_core::api::CredentialStoreApi;
        let entry = store.build(service, account, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(e);
    }

    #[test]
    fn round_trip() {
        let (k, _) = mock_keyring(false);
        k.set("work", &Password::new(SECRET.into())).unwrap();
        assert_eq!(k.get("work").unwrap().expose(), SECRET);
        assert!(k.get("home").is_err(), "entries are per account");
        assert!(k.delete("work").unwrap());
        assert!(!k.delete("work").unwrap());
    }

    #[test]
    fn missing_entry_says_how_to_add_one() {
        let (k, _) = mock_keyring(false);
        let message = format!("{:#}", k.get("work").unwrap_err());
        assert!(message.contains("no password in the mock"), "{message}");
        assert!(
            message.contains("run `nmail account set-password work` or set password_command"),
            "{message}"
        );
    }

    #[test]
    fn unavailable_keyring_suggests_password_command_and_on_wsl_says_why() {
        for wsl in [false, true] {
            let (k, store) = mock_keyring(wsl);
            fail_next(
                &store,
                "work",
                Error::NoStorageAccess("no Secret Service\non the bus".into()),
            );
            let message = format!("{:#}", k.get("work").unwrap_err());
            assert!(
                message.starts_with(
                    "the OS keyring is unavailable (no Secret Service on the bus); \
                     set password_command"
                ),
                "{message}"
            );
            assert_eq!(message.contains("WSL often has no Secret Service"), wsl);
            assert_eq!(message.contains("gnome-keyring"), wsl);
        }
        let (k, store) = mock_keyring(false);
        fail_next(&store, "work", Error::PlatformFailure("locked".into()));
        let e = k.set("work", &Password::new(SECRET.into())).unwrap_err();
        assert!(format!("{e:#}").contains("unavailable (locked)"), "{e:#}");
    }

    #[test]
    fn oauth_entry_round_trip_is_separate_from_the_password() {
        let (k, store) = mock_keyring(false);
        k.set("gmail", &Password::new(SECRET.into())).unwrap();
        assert!(k.get_oauth("gmail").unwrap().is_none());
        let entry = OAuthEntry {
            client_secret: SECRET.into(),
            refresh_token: Some("1//refresh-not-real".into()),
        };
        k.set_oauth("gmail", &entry).unwrap();
        let back = k.get_oauth("gmail").unwrap().unwrap();
        assert_eq!(back.client_secret().expose(), SECRET);
        assert_eq!(
            back.refresh_token().unwrap().expose(),
            "1//refresh-not-real"
        );
        assert_eq!(
            k.get("gmail").unwrap().expose(),
            SECRET,
            "the password is untouched"
        );
        assert!(!format!("{back:?}").contains(SECRET));
        assert!(k.delete_oauth("gmail").unwrap());
        assert!(!k.delete_oauth("gmail").unwrap());
        assert!(k.delete("gmail").unwrap());

        use keyring_core::api::CredentialStoreApi;
        let raw = store.build(OAUTH_SERVICE, "bad", None).unwrap();
        raw.set_password(&format!("{{\"client_secret\": {SECRET}"))
            .unwrap();
        let message = format!("{:#}", k.get_oauth("bad").unwrap_err());
        assert!(!message.contains(SECRET), "{message}");
        fail_next_in(
            &store,
            OAUTH_SERVICE,
            "bad",
            Error::NoStorageAccess("no bus".into()),
        );
        let message = format!("{:#}", k.get_oauth("bad").unwrap_err());
        assert!(
            message.contains("OAuth sign-ins are kept only there"),
            "{message}"
        );
    }

    #[test]
    fn errors_never_hold_the_stored_bytes() {
        let (k, store) = mock_keyring(false);
        for e in [
            Error::BadEncoding(SECRET.as_bytes().to_vec()),
            Error::BadDataFormat(SECRET.as_bytes().to_vec(), "bad".into()),
        ] {
            fail_next(&store, "work", e);
            let message = format!("{:#} {:?}", k.get("work").unwrap_err(), k.get("work"));
            assert!(!message.contains(SECRET), "{message}");
        }
    }
}
