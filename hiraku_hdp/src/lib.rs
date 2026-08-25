//! Hiraku Data Package container support.
//!
//! HDP keeps its format model independent from any particular compression or
//! encryption implementation. Version 1 currently ships stored and zstd
//! compression and reserves the encryption field for future authenticated
//! chunk encryption.

mod codec;
mod error;
mod format;
mod reader;
mod writer;

pub use codec::{CompressionMethod, CompressionOptions, EncryptionMethod};
pub use error::HdpError;
pub use format::{ChunkDescriptor, FORMAT_VERSION, FileEntry, HEADER_SIZE, MAGIC, PackageIndex};
pub use reader::Archive;
pub use writer::{
    FileOptions, PackOptions, PackageBuilder, PackageOutput, WrittenPackage, pack_directory,
    pack_directory_to, pack_directory_to_with, pack_directory_with, write_package,
};
