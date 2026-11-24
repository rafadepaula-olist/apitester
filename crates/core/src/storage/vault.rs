use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::{CoreError, CoreResult};

/// Abstraction for storing sensitive secrets (API keys, OAuth tokens, passwords).
pub trait VaultStore: Send + Sync {
    fn set_secret(&self, key: &str, value: &str) -> CoreResult<()>;
    fn get_secret(&self, key: &str) -> CoreResult<Option<String>>;
    fn delete_secret(&self, key: &str) -> CoreResult<()>;
}

/// OS-native keyring vault backed by the `keyring` crate.
#[derive(Debug, Clone)]
pub struct KeyringVault {
    service: String,
}

impl KeyringVault {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl Default for KeyringVault {
    fn default() -> Self {
        Self::new("apitester")
    }
}

impl VaultStore for KeyringVault {
    fn set_secret(&self, key: &str, value: &str) -> CoreResult<()> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CoreError::Vault(e.to_string()))?;
        entry
            .set_password(value)
            .map_err(|e| CoreError::Vault(e.to_string()))
    }

    fn get_secret(&self, key: &str) -> CoreResult<Option<String>> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CoreError::Vault(e.to_string()))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CoreError::Vault(e.to_string())),
        }
    }

    fn delete_secret(&self, key: &str) -> CoreResult<()> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| CoreError::Vault(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CoreError::Vault(e.to_string())),
        }
    }
}

/// Thread-safe in-memory vault for headless environments and automated tests.
#[derive(Debug, Default)]
pub struct MemoryVault {
    entries: RwLock<HashMap<String, String>>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }
}

impl VaultStore for MemoryVault {
    fn set_secret(&self, key: &str, value: &str) -> CoreResult<()> {
        let mut guard = self
            .entries
            .write()
            .map_err(|e| CoreError::Vault(format!("lock poisoned: {e}")))?;
        guard.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn get_secret(&self, key: &str) -> CoreResult<Option<String>> {
        let guard = self
            .entries
            .read()
            .map_err(|e| CoreError::Vault(format!("lock poisoned: {e}")))?;
        Ok(guard.get(key).cloned())
    }

    fn delete_secret(&self, key: &str) -> CoreResult<()> {
        let mut guard = self
            .entries
            .write()
            .map_err(|e| CoreError::Vault(format!("lock poisoned: {e}")))?;
        guard.remove(key);
        Ok(())
    }
}

/// Vault that attempts the OS keyring first and transparently falls back to `MemoryVault`
/// when no keyring daemon is available (e.g., headless CI or minimal Linux sessions).
#[derive(Debug, Default)]
pub struct HybridVault {
    keyring: KeyringVault,
    fallback: MemoryVault,
}

impl HybridVault {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            keyring: KeyringVault::new(service),
            fallback: MemoryVault::new(),
        }
    }
}

impl VaultStore for HybridVault {
    fn set_secret(&self, key: &str, value: &str) -> CoreResult<()> {
        match self.keyring.set_secret(key, value) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.set_secret(key, value),
        }
    }

    fn get_secret(&self, key: &str) -> CoreResult<Option<String>> {
        match self.keyring.get_secret(key) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) | Err(_) => self.fallback.get_secret(key),
        }
    }

    fn delete_secret(&self, key: &str) -> CoreResult<()> {
        let _ = self.keyring.delete_secret(key);
        self.fallback.delete_secret(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_crud() {
        let vault = MemoryVault::new();
        assert_eq!(vault.get_secret("api_key").unwrap(), None);

        vault.set_secret("api_key", "s3cr3t").unwrap();
        assert_eq!(
            vault.get_secret("api_key").unwrap(),
            Some("s3cr3t".to_string())
        );

        vault.delete_secret("api_key").unwrap();
        assert_eq!(vault.get_secret("api_key").unwrap(), None);
    }

    #[test]
    fn hybrid_vault_works_without_os_keyring() {
        let vault = HybridVault::new("apitester-test-service");
        vault.set_secret("test_token", "tok_123").unwrap();
        assert_eq!(
            vault.get_secret("test_token").unwrap(),
            Some("tok_123".to_string())
        );
        vault.delete_secret("test_token").unwrap();
        assert_eq!(vault.get_secret("test_token").unwrap(), None);
    }
}
