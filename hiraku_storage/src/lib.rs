//! Platform-independent byte storage for Hiraku runtime state.
//!
//! Serialization belongs to callers. This crate only maps validated logical
//! keys to durable bytes using the platform's appropriate backend.

use thiserror::Error;

mod platform;

pub use platform::AsyncPlatformStorage;
pub use platform::{BufferedStorage, initialize_runtime, runtime_status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeStorageStatus { Uninitialized, Loading, Ready, Writing, Failed(String) }

/// Acknowledges queued work, not durable completion. Poll runtime_status before
/// allowing operations dependent on this write to continue.
#[derive(Clone, Copy, Debug)]
pub struct WriteQueued;

/// Immutable generation records followed by a mutable publication/index record.
/// Native publishes the last record atomically; IndexedDB commits all records
/// in one transaction. Earlier records must use fresh, unique generation keys.
#[derive(Clone, Debug)]
pub struct GenerationRecord {
    pub key: String,
    pub extension: String,
    pub payload: Vec<u8>,
}

pub(crate) fn validate_generation(records: &[GenerationRecord]) -> Result<(), StorageError> {
    if records.is_empty() { return Err(StorageError::InvalidKey) }
    let mut keys = std::collections::BTreeSet::new();
    for record in records {
        validate_key(&record.key)?;
        if record.extension.is_empty() || !record.extension.bytes().all(|c| c.is_ascii_alphanumeric())
            || !keys.insert(&record.key) { return Err(StorageError::InvalidKey) }
    }
    Ok(())
}
/// Default durable storage selected for the current platform.
///
/// Native builds use files below the supplied root. WebAssembly builds use
/// browser storage under the supplied namespace; callers use the same API.
pub use platform::PlatformStorage;

/// Awaitable durable byte storage. A successful write/remove means the backend
/// transaction has completed, not merely that a request was enqueued.
/// Futures need not be Send: browser objects belong to their JavaScript thread.
#[allow(async_fn_in_trait)]
pub trait AsyncByteStorage {
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    async fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError>;
    async fn remove(&self, key: &str) -> Result<(), StorageError>;
    async fn contains(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.read(key).await?.is_some())
    }
}

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
    /// Test whether a key exists without decoding its payload.
    fn contains(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.read(key)?.is_some())
    }
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
