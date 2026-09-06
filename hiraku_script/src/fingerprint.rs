//! Content identities for executable layouts. These are compatibility checks,
//! not signatures authenticating an untrusted save file.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramFingerprint(pub String);

impl crate::Bytecode {
    /// Includes the bytecode ABI, source identity, native manifest identity,
    /// symbols, constants, signatures and register/instruction layout.
    pub fn fingerprint(&self) -> Result<ProgramFingerprint, crate::hson::HsonError> {
        fingerprint(self)
    }
}

pub(crate) fn fingerprint(
    value: &impl Serialize,
) -> Result<ProgramFingerprint, crate::hson::HsonError> {
    let encoded = crate::hson::to_string(value)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"hiraku/executable-layout/v1\0");
    hash.update(encoded.as_bytes());
    Ok(ProgramFingerprint(hash.finalize().to_hex().to_string()))
}
