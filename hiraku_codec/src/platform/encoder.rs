//! Capability boundary for future platform encoder adapters. No backend is
//! advertised until configure/encode/flush are actually implemented.
use crate::CodecError;

pub(crate) fn unavailable() -> CodecError {
    CodecError::Unsupported("no encoder adapter is implemented on this platform".into())
}
