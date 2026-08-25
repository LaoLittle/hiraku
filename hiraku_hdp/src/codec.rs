use std::io::{self, Cursor, Read};

use crate::HdpError;

/// Stable on-disk identifier for a chunk compression algorithm.
///
/// This is intentionally a newtype instead of an enum so adding a codec does
/// not require changing the package/index representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressionMethod(u8);

impl CompressionMethod {
    pub const STORED: Self = Self(0);
    pub const ZSTD: Self = Self(1);

    pub const fn from_id(id: u8) -> Self {
        Self(id)
    }

    pub const fn id(self) -> u8 {
        self.0
    }

    pub const fn is_supported(self) -> bool {
        matches!(self, Self::STORED | Self::ZSTD)
    }
}

/// Stable on-disk identifier for chunk encryption.
///
/// Encryption is deliberately orthogonal to compression. Version 1 only
/// accepts `NONE`; authenticated encryption can be added without rewriting the
/// file table or codec API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncryptionMethod(u8);

impl EncryptionMethod {
    pub const NONE: Self = Self(0);
    pub const CHACHA20_POLY1305: Self = Self(1);

    pub const fn from_id(id: u8) -> Self {
        Self(id)
    }

    pub const fn id(self) -> u8 {
        self.0
    }

    pub const fn is_supported(self) -> bool {
        matches!(self, Self::NONE)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CompressionOptions {
    pub method: CompressionMethod,
    pub level: i32,
    /// Store a chunk verbatim unless compression saves at least this many bytes.
    pub min_savings: usize,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self {
            method: CompressionMethod::ZSTD,
            level: 3,
            min_savings: 16,
        }
    }
}

pub(crate) fn encode(
    input: &[u8],
    options: CompressionOptions,
) -> Result<(CompressionMethod, Vec<u8>), HdpError> {
    match options.method {
        CompressionMethod::STORED => Ok((CompressionMethod::STORED, input.to_vec())),
        CompressionMethod::ZSTD => {
            let encoded = zstd::stream::encode_all(Cursor::new(input), options.level)?;
            if encoded.len().saturating_add(options.min_savings) >= input.len() {
                Ok((CompressionMethod::STORED, input.to_vec()))
            } else {
                Ok((CompressionMethod::ZSTD, encoded))
            }
        }
        other => Err(HdpError::UnsupportedCompression(other.id())),
    }
}

/// Compresses one bounded package chunk from a reader into a reusable buffer.
///
/// The caller owns chunk placement and may reread the source when this returns
/// `STORED`; this keeps codec selection independent from filesystem/volume
/// policy while avoiding a package-sized allocation.
pub(crate) fn encode_stream(
    input: &mut impl Read,
    input_size: usize,
    options: CompressionOptions,
    output: &mut Vec<u8>,
) -> Result<CompressionMethod, HdpError> {
    output.clear();
    match options.method {
        CompressionMethod::STORED => {
            io::copy(input, &mut io::sink())?;
            Ok(CompressionMethod::STORED)
        }
        CompressionMethod::ZSTD => {
            let mut encoder = zstd::stream::write::Encoder::new(&mut *output, options.level)?;
            io::copy(input, &mut encoder)?;
            encoder.finish()?;
            if output.len().saturating_add(options.min_savings) >= input_size {
                Ok(CompressionMethod::STORED)
            } else {
                Ok(CompressionMethod::ZSTD)
            }
        }
        other => Err(HdpError::UnsupportedCompression(other.id())),
    }
}

pub(crate) fn decode(method: CompressionMethod, input: &[u8]) -> Result<Vec<u8>, HdpError> {
    match method {
        CompressionMethod::STORED => Ok(input.to_vec()),
        CompressionMethod::ZSTD => Ok(zstd::stream::decode_all(Cursor::new(input))?),
        other => Err(HdpError::UnsupportedCompression(other.id())),
    }
}
