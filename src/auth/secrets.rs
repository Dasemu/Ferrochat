//! The ONLY module allowed to call the `keyring` crate.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use secrecy::{ExposeSecret, SecretString};

use crate::config::APP_ID;
use crate::error::{Error, Result};

/// Keyring account holding the random 256-bit key that encrypts the
/// protocol library's Signal/session database (SQLCipher).
pub const SIGNAL_STORE_KEY: &str = "signal-store-key";

/// A secret value. Zeroised on drop, `Debug` prints `[REDACTED]`.
#[derive(Clone)]
pub struct Secret(SecretString);

impl Secret {
    /// Wraps a string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    /// Exposes the inner value. Keep the borrow short.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Storage for secrets. Real implementation: OS keyring. Tests and `--mock`
/// use [`MemoryStore`].
pub trait SecretStore: Send + Sync {
    /// Reads a secret, `None` if absent.
    fn get(&self, key: &str) -> Result<Option<Secret>>;
    /// Writes (or replaces) a secret.
    fn set(&self, key: &str, value: &Secret) -> Result<()>;
    /// Deletes a secret; deleting a missing one is not an error.
    fn delete(&self, key: &str) -> Result<()>;
}

/// OS keyring: Secret Service (KWallet / GNOME Keyring) on Linux, Keychain on
/// macOS, Credential Manager on Windows. Calls are blocking.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

fn keyring_err(e: keyring::Error) -> Error {
    // keyring errors never contain the secret value itself.
    Error::Keyring(e.to_string())
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        let entry = keyring::Entry::new(APP_ID, key).map_err(keyring_err)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(Secret::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_err(e)),
        }
    }

    fn set(&self, key: &str, value: &Secret) -> Result<()> {
        let entry = keyring::Entry::new(APP_ID, key).map_err(keyring_err)?;
        entry.set_password(value.expose()).map_err(keyring_err)
    }

    fn delete(&self, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(APP_ID, key).map_err(keyring_err)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_err(e)),
        }
    }
}

/// In-memory secret store for tests and `--mock` mode.
#[derive(Default, Clone)]
pub struct MemoryStore(Arc<Mutex<HashMap<String, Secret>>>);

impl MemoryStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Secret>>> {
        self.0
            .lock()
            .map_err(|_| Error::Keyring("memory store poisoned".into()))
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        Ok(self.lock()?.get(key).cloned())
    }

    fn set(&self, key: &str, value: &Secret) -> Result<()> {
        self.lock()?.insert(key.to_string(), value.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.lock()?.remove(key);
        Ok(())
    }
}

fn well_formed(k: &Secret) -> bool {
    k.expose().len() == 64 && k.expose().bytes().all(|b| b.is_ascii_hexdigit())
}

/// Reads the Signal store key without ever creating one. `None` when it is
/// missing or malformed: with an existing session file the caller must ask
/// the user instead of minting a key (which would orphan the session).
pub fn get_signal_store_key(store: &dyn SecretStore) -> Result<Option<Secret>> {
    Ok(store.get(SIGNAL_STORE_KEY)?.filter(well_formed))
}

/// Returns the Signal store key, creating a fresh random 256-bit one if the
/// keyring has none yet. `created` tells the caller that any existing
/// encrypted database is unreadable and must be discarded.
pub fn signal_store_key(store: &dyn SecretStore) -> Result<(Secret, bool)> {
    if let Some(k) = store.get(SIGNAL_STORE_KEY)? {
        if well_formed(&k) {
            return Ok((k, false));
        }
        tracing::warn!("signal store key in keyring is malformed; generating a new one");
    }
    let mut raw = zeroize::Zeroizing::new([0u8; 32]);
    getrandom::fill(raw.as_mut()).map_err(|e| Error::Keyring(format!("rng: {e}")))?;
    let key = Secret::new(hex::encode(raw.as_ref()));
    store.set(SIGNAL_STORE_KEY, &key)?;
    Ok((key, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::new();
        assert!(s.get("a").unwrap().is_none());
        s.set("a", &Secret::new("v")).unwrap();
        assert_eq!(s.get("a").unwrap().unwrap().expose(), "v");
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert!(s.get("a").unwrap().is_none());
    }

    #[test]
    fn secret_debug_is_redacted() {
        let s = Secret::new("super-secret-token");
        assert_eq!(format!("{s:?}"), "[REDACTED]");
    }

    #[test]
    fn signal_key_is_created_once() {
        let s = MemoryStore::new();
        let (k1, created1) = signal_store_key(&s).unwrap();
        let (k2, created2) = signal_store_key(&s).unwrap();
        assert!(created1);
        assert!(!created2);
        assert_eq!(k1.expose(), k2.expose());
        assert_eq!(k1.expose().len(), 64);
    }

    #[test]
    fn get_never_creates() {
        let s = MemoryStore::new();
        assert!(get_signal_store_key(&s).unwrap().is_none());
        assert!(s.get(SIGNAL_STORE_KEY).unwrap().is_none());
        s.set(SIGNAL_STORE_KEY, &Secret::new("short")).unwrap();
        assert!(get_signal_store_key(&s).unwrap().is_none());
        let (k, _) = signal_store_key(&MemoryStore::new()).unwrap();
        s.set(SIGNAL_STORE_KEY, &k).unwrap();
        assert!(get_signal_store_key(&s).unwrap().is_some());
    }

    #[test]
    fn malformed_key_is_replaced() {
        let s = MemoryStore::new();
        s.set(SIGNAL_STORE_KEY, &Secret::new("short")).unwrap();
        let (k, created) = signal_store_key(&s).unwrap();
        assert!(created);
        assert_eq!(k.expose().len(), 64);
    }
}
