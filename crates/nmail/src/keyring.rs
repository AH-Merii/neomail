//! The OS keyring: the Secret Service over D-Bus on Linux (zbus, pure Rust), the login keychain
//! on macOS. An account's entry has the service `neomail` and the account name as the user.
//!
//! Errors are written here rather than passed through: keyring-core's `BadEncoding` and
//! `BadDataFormat` carry the stored bytes, so they are never formatted with `{:?}`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use keyring_core::{CredentialStore, Entry, Error};

use crate::secret::Password;

/// The keyring service every neomail entry is stored under.
pub const SERVICE: &str = "neomail";

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
        let wsl = is_wsl();
        match platform_store() {
            Ok(store) => Ok(Self::with_store(store, PLATFORM_NAME, wsl)),
            Err(e) => Err(error(&e, "", wsl)),
        }
    }

    pub fn with_store(store: Arc<CredentialStore>, name: &'static str, wsl: bool) -> Self {
        Self { store, name, wsl }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn entry(&self, account: &str) -> Result<Entry> {
        self.store
            .build(SERVICE, account, None)
            .map_err(|e| error(&e, account, self.wsl))
    }

    pub fn get(&self, account: &str) -> Result<Password> {
        match self.entry(account)?.get_password() {
            Ok(password) => Ok(Password::new(password)),
            Err(Error::NoEntry) => Err(anyhow!(
                "no password in {} (service {SERVICE:?}, user {account:?}); run \
                 `nmail account set-password {account}` or set password_command",
                self.name
            )),
            Err(e) => Err(error(&e, account, self.wsl)),
        }
    }

    pub fn set(&self, account: &str, password: &Password) -> Result<()> {
        self.entry(account)?
            .set_password(password.expose())
            .map_err(|e| error(&e, account, self.wsl))
    }

    /// Deletes the entry. `false` when there was none.
    pub fn delete(&self, account: &str) -> Result<bool> {
        match self.entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(Error::NoEntry) => Ok(false),
            Err(e) => Err(error(&e, account, self.wsl)),
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
fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists()
}

/// One line for a keyring error, never holding the stored data. `account` is empty when no
/// entry was involved yet.
fn error(e: &Error, account: &str, wsl: bool) -> anyhow::Error {
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
                "{} keyring items match service {SERVICE:?} and user {account:?}; delete all but one",
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

    /// Makes the next call on `account`'s entry fail with `e`.
    fn fail_next(store: &mock::Store, account: &str, e: Error) {
        use keyring_core::api::CredentialStoreApi;
        let entry = store.build(SERVICE, account, None).unwrap();
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
