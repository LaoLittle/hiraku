use std::path::PathBuf;

use crate::{ByteStorage, StorageError, validate_key};

/// Browser `localStorage` backend selected on WebAssembly targets.
#[derive(Clone, Debug)]
pub struct PlatformStorage {
    namespace: String,
}

impl PlatformStorage {
    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        let _ = root.into();
        let _ = extension.into();
        Self { namespace: namespace.into() }
    }

    fn browser_key(&self, key: &str) -> Result<String, StorageError> {
        validate_key(key)?;
        Ok(format!("{}.{}", self.namespace, key))
    }
}

impl ByteStorage for PlatformStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let key = self.browser_key(key)?;
        let Some(payload) = browser_storage()?.get_item(&key).map_err(|error| {
            StorageError::Browser(format!("cannot read `{key}`: {error:?}"))
        })? else {
            return Ok(None);
        };
        decode_hex(&payload).map(Some)
    }

    fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError> {
        let key = self.browser_key(key)?;
        browser_storage()?.set_item(&key, &encode_hex(payload)).map_err(|error| {
            StorageError::Browser(format!("cannot write `{key}` (localStorage may be full): {error:?}"))
        })
    }

    fn remove(&self, key: &str) -> Result<(), StorageError> {
        let key = self.browser_key(key)?;
        browser_storage()?.remove_item(&key).map_err(|error| {
            StorageError::Browser(format!("cannot remove `{key}`: {error:?}"))
        })
    }
}

fn browser_storage() -> Result<web_sys::Storage, StorageError> {
    let window = web_sys::window()
        .ok_or_else(|| StorageError::Browser("window is unavailable".into()))?;
    window.local_storage()
        .map_err(|error| StorageError::Browser(format!("cannot access localStorage: {error:?}")))?
        .ok_or_else(|| StorageError::Browser("localStorage is disabled".into()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, StorageError> {
    if !value.len().is_multiple_of(2) {
        return Err(StorageError::Corrupt("hexadecimal payload has an odd length".into()));
    }
    value.as_bytes().chunks_exact(2)
        .map(|digits| Ok((hex_digit(digits[0])? << 4) | hex_digit(digits[1])?))
        .collect()
}

fn hex_digit(digit: u8) -> Result<u8, StorageError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(StorageError::Corrupt("payload contains a non-hexadecimal character".into())),
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
        assert_eq!(decode_hex(&encoded).expect("hex decodes"), payload);
    }
}
