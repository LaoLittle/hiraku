//! Platform-independent byte storage for Hiraku runtime state.
//!
//! Serialization belongs to callers. This crate only maps validated logical
//! keys to durable bytes using the platform's appropriate backend.

use thiserror::Error;

mod platform;

/// Default durable storage selected for the current platform.
///
/// Native builds use files below the supplied root. WebAssembly builds use
/// browser storage under the supplied namespace; callers use the same API.
pub use platform::PlatformStorage;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage key can only contain letters, digits, '-' or '_'")]
    InvalidKey,
    #[error("failed to access file storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("browser storage failed: {0}")]
    Browser(String),
    #[error("stored payload is corrupt: {0}")]
    Corrupt(String),
}

/// Durable storage of opaque byte payloads.
pub trait ByteStorage: Send + Sync {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError>;
    fn remove(&self, key: &str) -> Result<(), StorageError>;
}

pub fn validate_key(key: &str) -> Result<&str, StorageError> {
    if !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        Ok(key)
    } else {
        Err(StorageError::InvalidKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_keys_that_could_escape_native_storage() {
        assert!(validate_key("quick-save_1").is_ok());
        assert!(validate_key("../save").is_err());
        assert!(validate_key("").is_err());
    }
}
