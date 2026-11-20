use lna_types::{ErrorCode, LnaError, LnaResult};
use secrecy::SecretString;
use std::collections::HashMap;
use std::sync::RwLock;

const SERVICE_NAME: &str = "LocalNotebookAgent";

pub trait SecretStore: Send + Sync {
    fn set(&self, key_ref: &str, secret: SecretString) -> LnaResult<()>;
    fn get(&self, key_ref: &str) -> LnaResult<Option<SecretString>>;
    fn clear(&self, key_ref: &str) -> LnaResult<()>;
    fn exists(&self, key_ref: &str) -> LnaResult<bool>;
}

/// Uses OS native credential manager (Windows Credential Locker / macOS Keychain / Linux Secret Service).
/// Falls back to an in-memory volatile store only if `allow_memory_fallback` is true (dev/headless).
pub struct KeyringSecretStore {
    allow_memory_fallback: bool,
    volatile: RwLock<HashMap<String, SecretString>>,
}

impl KeyringSecretStore {
    pub fn new(allow_memory_fallback: bool) -> Self {
        Self {
            allow_memory_fallback,
            volatile: RwLock::new(HashMap::new()),
        }
    }

    fn entry(key_ref: &str) -> LnaResult<keyring::Entry> {
        keyring::Entry::new(SERVICE_NAME, key_ref).map_err(|e| {
            LnaError::new(
                ErrorCode::KeychainUnavailable,
                format!("Failed to access OS credential store: {e}"),
            )
        })
    }
}

impl SecretStore for KeyringSecretStore {
    fn set(&self, key_ref: &str, secret: SecretString) -> LnaResult<()> {
        use secrecy::ExposeSecret;
        match Self::entry(key_ref).and_then(|e| {
            e.set_password(secret.expose_secret()).map_err(|err| {
                LnaError::new(
                    ErrorCode::KeychainUnavailable,
                    format!("Failed to write secret to keychain: {err}"),
                )
            })
        }) {
            Ok(()) => {
                if let Ok(mut g) = self.volatile.write() {
                    g.insert(key_ref.to_string(), secret);
                }
                Ok(())
            }
            Err(err) if self.allow_memory_fallback => {
                tracing::warn!(
                    key_ref,
                    error = %err.message,
                    "keychain unavailable, using volatile in-memory store"
                );
                let mut g = self
                    .volatile
                    .write()
                    .map_err(|_| LnaError::internal("Secret lock poisoned"))?;
                g.insert(key_ref.to_string(), secret);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn get(&self, key_ref: &str) -> LnaResult<Option<SecretString>> {
        if let Ok(g) = self.volatile.read() {
            if let Some(v) = g.get(key_ref) {
                return Ok(Some(v.clone()));
            }
        }
        match Self::entry(key_ref) {
            Ok(entry) => match entry.get_password() {
                Ok(pw) => Ok(Some(SecretString::from(pw))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) if self.allow_memory_fallback => {
                    tracing::debug!(key_ref, %err, "keychain read failed with fallback enabled");
                    Ok(None)
                }
                Err(err) => Err(LnaError::new(
                    ErrorCode::KeychainUnavailable,
                    format!("Failed to read secret from keychain: {err}"),
                )),
            },
            Err(_err) if self.allow_memory_fallback => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn clear(&self, key_ref: &str) -> LnaResult<()> {
        if let Ok(mut g) = self.volatile.write() {
            g.remove(key_ref);
        }
        match Self::entry(key_ref) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_err) if self.allow_memory_fallback => Ok(()),
                Err(err) => Err(LnaError::new(
                    ErrorCode::KeychainUnavailable,
                    format!("Failed to delete secret from keychain: {err}"),
                )),
            },
            Err(_err) if self.allow_memory_fallback => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn exists(&self, key_ref: &str) -> LnaResult<bool> {
        Ok(self.get(key_ref)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    /// B-24: secrets must persist in the OS credential store, so a fresh store
    /// instance (as after an app restart) can read them back.
    #[cfg(windows)]
    #[test]
    fn secret_persists_across_store_instances() {
        let key_ref = format!("lna_test_{}", lna_types::new_ulid());
        KeyringSecretStore::new(false)
            .set(&key_ref, SecretString::from("persist-check".to_string()))
            .unwrap();

        let fresh = KeyringSecretStore::new(false);
        let got = fresh.get(&key_ref).unwrap();
        fresh.clear(&key_ref).unwrap();
        assert_eq!(
            got.map(|s| s.expose_secret().to_string()).as_deref(),
            Some("persist-check")
        );
        assert!(!KeyringSecretStore::new(false).exists(&key_ref).unwrap());
    }
}
