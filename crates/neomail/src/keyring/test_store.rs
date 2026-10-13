//! A keyring for the CLI tests only: a JSON file named by `NEOMAIL_TEST_KEYRING`, so a value
//! stored by one `neomail` process is read by the next without touching the real Secret Service
//! or keychain. Compiled into debug builds alone (`cfg(debug_assertions)`): a release binary
//! never reads secrets from a plain file.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use keyring_core::api::{CredentialApi, CredentialStoreApi};
use keyring_core::{Credential, CredentialStore, Entry, Error, Result};

/// The variable naming the file.
pub const ENV: &str = "NEOMAIL_TEST_KEYRING";

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn shared(path: PathBuf) -> Arc<CredentialStore> {
        Arc::new(Store { path })
    }
}

type Entries = BTreeMap<String, String>;

fn load(path: &PathBuf) -> Result<Entries> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| Error::PlatformFailure("the test keyring is not JSON".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Entries::new()),
        Err(e) => Err(Error::PlatformFailure(Box::new(e))),
    }
}

fn save(path: &PathBuf, entries: &Entries) -> Result<()> {
    let bytes = serde_json::to_vec(entries)
        .map_err(|_| Error::PlatformFailure("cannot write the test keyring".into()))?;
    std::fs::write(path, bytes).map_err(|e| Error::PlatformFailure(Box::new(e)))
}

impl CredentialStoreApi for Store {
    fn vendor(&self) -> String {
        "neomail test keyring".to_owned()
    }

    fn id(&self) -> String {
        self.path.display().to_string()
    }

    fn build(
        &self,
        service: &str,
        user: &str,
        _modifiers: Option<&HashMap<&str, &str>>,
    ) -> Result<Entry> {
        let credential: Arc<Credential> = Arc::new(Item {
            path: self.path.clone(),
            service: service.to_owned(),
            user: user.to_owned(),
        });
        Ok(Entry::new_with_credential(credential))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
struct Item {
    path: PathBuf,
    service: String,
    user: String,
}

impl Item {
    fn key(&self) -> String {
        format!("{}\n{}", self.service, self.user)
    }
}

impl CredentialApi for Item {
    fn set_secret(&self, secret: &[u8]) -> Result<()> {
        let mut entries = load(&self.path)?;
        let value = String::from_utf8(secret.to_vec())
            .map_err(|_| Error::Invalid("secret".into(), "not UTF-8".into()))?;
        entries.insert(self.key(), value);
        save(&self.path, &entries)
    }

    fn get_secret(&self) -> Result<Vec<u8>> {
        load(&self.path)?
            .remove(&self.key())
            .map(String::into_bytes)
            .ok_or(Error::NoEntry)
    }

    fn delete_credential(&self) -> Result<()> {
        let mut entries = load(&self.path)?;
        entries.remove(&self.key()).ok_or(Error::NoEntry)?;
        save(&self.path, &entries)
    }

    fn get_credential(&self) -> Result<Option<Arc<Credential>>> {
        self.get_secret().map(|_| None)
    }

    fn get_specifiers(&self) -> Option<(String, String)> {
        Some((self.service.clone(), self.user.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
