use thiserror::Error;

const SERVICE_NAME: &str = "holt";

#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("Keychain error: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("Key not found for agent: {0}")]
    NotFound(String),
}

/// Manages API key storage via the OS keychain (PRD Section 12.1).
/// Windows: Windows Credential Manager
/// Linux: Secret Service (GNOME Keyring / KDE Wallet)
/// macOS: Keychain
pub struct KeychainManager;

impl Default for KeychainManager {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainManager {
    pub fn new() -> Self {
        Self
    }

    /// Store an API key for the given agent ID.
    pub fn store_api_key(&self, agent_id: &str, key: &str) -> Result<(), KeychainError> {
        let entry = keyring::Entry::new(SERVICE_NAME, agent_id)?;
        entry.set_password(key)?;
        Ok(())
    }

    /// Retrieve an API key for the given agent ID.
    pub fn get_api_key(&self, agent_id: &str) -> Result<String, KeychainError> {
        let entry = keyring::Entry::new(SERVICE_NAME, agent_id)?;
        match entry.get_password() {
            Ok(key) => Ok(key),
            Err(keyring::Error::NoEntry) => Err(KeychainError::NotFound(agent_id.to_string())),
            Err(e) => Err(KeychainError::Keyring(e)),
        }
    }

    /// Delete an API key for the given agent ID.
    pub fn delete_api_key(&self, agent_id: &str) -> Result<(), KeychainError> {
        let entry = keyring::Entry::new(SERVICE_NAME, agent_id)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already gone, that's fine
            Err(e) => Err(KeychainError::Keyring(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keychain tests require a running keychain daemon and may not work in CI.
    // Run with: cargo test -- --ignored
    #[test]
    #[ignore]
    fn test_keychain_store_get_delete() {
        let km = KeychainManager::new();
        let agent_id = "test-keychain-agent";
        let test_key = "sk-test-key-12345";

        // Store
        km.store_api_key(agent_id, test_key).unwrap();

        // Get
        let retrieved = km.get_api_key(agent_id).unwrap();
        assert_eq!(retrieved, test_key);

        // Delete
        km.delete_api_key(agent_id).unwrap();

        // Verify deleted
        match km.get_api_key(agent_id) {
            Err(KeychainError::NotFound(_)) => {} // Expected
            other => panic!("Expected NotFound, got {:?}", other),
        }
    }
}
