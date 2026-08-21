use std::{fmt, io};

#[derive(Debug)]
pub enum HdpError {
    Io(io::Error),
    InvalidFormat(String),
    UnsupportedVersion(u16),
    UnsupportedCompression(u8),
    UnsupportedEncryption(u8),
    DuplicatePath(String),
    InvalidPath(String),
    MissingFile(String),
    MissingVolume(u32),
    CorruptChunk { path: String, chunk: usize },
}

impl fmt::Display for HdpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "HDP I/O error: {error}"),
            Self::InvalidFormat(message) => write!(formatter, "invalid HDP package: {message}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported HDP format version {version}")
            }
            Self::UnsupportedCompression(method) => {
                write!(formatter, "unsupported HDP compression method {method}")
            }
            Self::UnsupportedEncryption(method) => {
                write!(formatter, "unsupported HDP encryption method {method}")
            }
            Self::DuplicatePath(path) => write!(formatter, "duplicate HDP path `{path}`"),
            Self::InvalidPath(path) => write!(formatter, "invalid HDP path `{path}`"),
            Self::MissingFile(path) => write!(formatter, "HDP file not found: `{path}`"),
            Self::MissingVolume(volume) => write!(formatter, "HDP volume {volume} is missing"),
            Self::CorruptChunk { path, chunk } => {
                write!(formatter, "HDP chunk {chunk} for `{path}` is corrupt")
            }
        }
    }
}

impl std::error::Error for HdpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for HdpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
