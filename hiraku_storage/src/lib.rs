//! Platform-independent byte storage for Hiraku runtime state.
//!
//! Serialization belongs to callers. This crate only maps validated logical
//! keys to durable bytes using the platform's appropriate backend.

use std::path::{Path, PathBuf};

use thiserror::Error;

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

/// The default backend selected for the current target.
///
/// Native targets store files below `root`. Web targets ignore the filesystem
/// path and use `localStorage` keys prefixed by `namespace`.
#[derive(Clone, Debug)]
pub struct PlatformStorage {
    #[cfg(not(target_arch = "wasm32"))]
    root: PathBuf,
    #[cfg(target_arch = "wasm32")]
    namespace: String,
    #[cfg(not(target_arch = "wasm32"))]
    extension: String,
}

impl PlatformStorage {
    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = namespace.into();
            Self {
                root: root.into(),
                extension: extension.into(),
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = root.into();
            let _ = extension.into();
            Self {
                namespace: namespace.into(),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn file_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        validate_key(key)?;
        Ok(self.root.join(format!("{key}.{}", self.extension)))
    }

    #[cfg(target_arch = "wasm32")]
    fn browser_key(&self, key: &str) -> Result<String, StorageError> {
        validate_key(key)?;
        Ok(format!("{}.{}", self.namespace, key))
    }
}

impl ByteStorage for PlatformStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.file_path(key)?;
            return match std::fs::read(path) {
                Ok(payload) => Ok(Some(payload)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            };
        }

        #[cfg(target_arch = "wasm32")]
        {
            let key = self.browser_key(key)?;
            let Some(payload) = browser_storage()?.get_item(&key).map_err(|error| {
                StorageError::Browser(format!("cannot read `{key}`: {error:?}"))
            })?
            else {
                return Ok(None);
            };
            decode_hex(&payload).map(Some)
        }
    }

    fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.file_path(key)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            return std::fs::write(path, payload).map_err(Into::into);
        }

        #[cfg(target_arch = "wasm32")]
        {
            let key = self.browser_key(key)?;
            browser_storage()?
                .set_item(&key, &encode_hex(payload))
                .map_err(|error| {
                    StorageError::Browser(format!(
                        "cannot write `{key}` (localStorage may be full): {error:?}"
                    ))
                })
        }
    }

    fn remove(&self, key: &str) -> Result<(), StorageError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.file_path(key)?;
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }

        #[cfg(target_arch = "wasm32")]
        {
            let key = self.browser_key(key)?;
            browser_storage()?
                .remove_item(&key)
                .map_err(|error| StorageError::Browser(format!("cannot remove `{key}`: {error:?}")))
        }
    }
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

pub fn native_path(root: &Path, key: &str, extension: &str) -> Result<PathBuf, StorageError> {
    validate_key(key)?;
    Ok(root.join(format!("{key}.{extension}")))
}

#[cfg(target_arch = "wasm32")]
fn browser_storage() -> Result<web_sys::Storage, StorageError> {
    let window =
        web_sys::window().ok_or_else(|| StorageError::Browser("window is unavailable".into()))?;
    window
        .local_storage()
        .map_err(|error| StorageError::Browser(format!("cannot access localStorage: {error:?}")))?
        .ok_or_else(|| StorageError::Browser("localStorage is disabled".into()))
}

#[cfg(any(test, target_arch = "wasm32"))]
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(any(test, target_arch = "wasm32"))]
fn decode_hex(value: &str) -> Result<Vec<u8>, StorageError> {
    if !value.len().is_multiple_of(2) {
        return Err(StorageError::Corrupt(
            "hexadecimal payload has an odd length".into(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|digits| Ok((hex_digit(digits[0])? << 4) | hex_digit(digits[1])?))
        .collect()
}

#[cfg(any(test, target_arch = "wasm32"))]
fn hex_digit(digit: u8) -> Result<u8, StorageError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(StorageError::Corrupt(
            "payload contains a non-hexadecimal character".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_encoding_preserves_binary_payloads() {
        let payload = [0, 1, 15, 16, 127, 128, 254, 255];
        let encoded = encode_hex(&payload);
        assert_eq!(encoded, "00010f107f80feff");
        assert_eq!(decode_hex(&encoded).unwrap(), payload);
    }

    #[test]
    fn rejects_keys_that_could_escape_native_storage() {
        assert!(validate_key("quick-save_1").is_ok());
        assert!(validate_key("../save").is_err());
        assert!(validate_key("").is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_backend_roundtrips_and_removes_bytes() {
        let root = std::env::temp_dir().join(format!("hiraku-storage-test-{}", std::process::id()));
        let storage = PlatformStorage::new(&root, "test", "bin");
        storage
            .write("quick", b"save payload")
            .expect("temporary save must be writable");
        assert_eq!(
            storage.read("quick").expect("save must be readable"),
            Some(b"save payload".to_vec())
        );
        storage
            .remove("quick")
            .expect("temporary save must be removable");
        assert_eq!(storage.read("quick").unwrap(), None);
        std::fs::remove_dir(root).expect("empty temporary storage root must be removable");
    }
}
