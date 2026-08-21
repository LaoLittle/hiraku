use std::collections::BTreeMap;

use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned,
    byteorder::little_endian::{U16, U32, U64},
};

use crate::{CompressionMethod, EncryptionMethod, HdpError};

pub const MAGIC: [u8; 4] = *b"HDP\0";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_SIZE: usize = size_of::<WireVolumeHeader>();
pub(crate) const CHUNK_DESCRIPTOR_SIZE: usize = 40;

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct WireVolumeHeader {
    magic: [u8; 4],
    version: U16,
    header_size: U16,
    flags: U32,
    package_id: U64,
    volume_index: U32,
    volume_count: U32,
    index_size: U64,
    data_offset: U64,
    index_checksum: U64,
}

const _: () = assert!(HEADER_SIZE == 52);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkDescriptor {
    pub volume: u32,
    pub offset: u64,
    pub stored_size: u64,
    pub decoded_size: u64,
    pub checksum: u64,
    pub compression: CompressionMethod,
    pub encryption: EncryptionMethod,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub decoded_size: u64,
    pub chunks: Vec<ChunkDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageIndex {
    pub package_id: u64,
    pub volume_count: u32,
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct VolumeHeader {
    pub package_id: u64,
    pub volume_index: u32,
    pub volume_count: u32,
    pub index_size: u64,
    pub data_offset: u64,
    pub index_checksum: u64,
}

pub(crate) fn encode_header(header: VolumeHeader) -> [u8; HEADER_SIZE] {
    let wire = WireVolumeHeader {
        magic: MAGIC,
        version: U16::new(FORMAT_VERSION),
        header_size: U16::new(HEADER_SIZE as u16),
        flags: U32::new(0),
        package_id: U64::new(header.package_id),
        volume_index: U32::new(header.volume_index),
        volume_count: U32::new(header.volume_count),
        index_size: U64::new(header.index_size),
        data_offset: U64::new(header.data_offset),
        index_checksum: U64::new(header.index_checksum),
    };
    wire.as_bytes()
        .try_into()
        .expect("wire HDP header size must equal HEADER_SIZE")
}

pub(crate) fn decode_header(bytes: &[u8]) -> Result<VolumeHeader, HdpError> {
    let (wire, _) = WireVolumeHeader::read_from_prefix(bytes)
        .map_err(|_| HdpError::InvalidFormat("truncated volume header".into()))?;
    if wire.magic != MAGIC {
        return Err(HdpError::InvalidFormat("incorrect magic".into()));
    }
    let version = wire.version.get();
    if version != FORMAT_VERSION {
        return Err(HdpError::UnsupportedVersion(version));
    }
    let header_size = wire.header_size.get() as usize;
    if header_size != HEADER_SIZE {
        return Err(HdpError::InvalidFormat(format!(
            "unexpected header size {header_size}"
        )));
    }
    Ok(VolumeHeader {
        package_id: wire.package_id.get(),
        volume_index: wire.volume_index.get(),
        volume_count: wire.volume_count.get(),
        index_size: wire.index_size.get(),
        data_offset: wire.data_offset.get(),
        index_checksum: wire.index_checksum.get(),
    })
}

pub(crate) fn encoded_index_size(files: &[(String, usize)]) -> Result<usize, HdpError> {
    let mut size = 4_usize;
    for (path, chunks) in files {
        let path_len =
            u32::try_from(path.len()).map_err(|_| HdpError::InvalidPath(path.clone()))? as usize;
        size = size
            .checked_add(4 + path_len + 8 + 4)
            .and_then(|size| size.checked_add(chunks.checked_mul(CHUNK_DESCRIPTOR_SIZE)?))
            .ok_or_else(|| HdpError::InvalidFormat("index is too large".into()))?;
    }
    Ok(size)
}

pub(crate) fn encode_index(files: &[FileEntry]) -> Result<Vec<u8>, HdpError> {
    let mut output = Vec::new();
    push_u32(
        &mut output,
        u32::try_from(files.len()).map_err(|_| HdpError::InvalidFormat("too many files".into()))?,
    );
    for file in files {
        push_u32(
            &mut output,
            u32::try_from(file.path.len()).map_err(|_| HdpError::InvalidPath(file.path.clone()))?,
        );
        output.extend_from_slice(file.path.as_bytes());
        push_u64(&mut output, file.decoded_size);
        push_u32(
            &mut output,
            u32::try_from(file.chunks.len())
                .map_err(|_| HdpError::InvalidFormat("too many chunks".into()))?,
        );
        for chunk in &file.chunks {
            push_u32(&mut output, chunk.volume);
            output.push(chunk.compression.id());
            output.push(chunk.encryption.id());
            output.extend_from_slice(&[0, 0]);
            push_u64(&mut output, chunk.offset);
            push_u64(&mut output, chunk.stored_size);
            push_u64(&mut output, chunk.decoded_size);
            push_u64(&mut output, chunk.checksum);
        }
    }
    Ok(output)
}

pub(crate) fn decode_index(
    bytes: &[u8],
    package_id: u64,
    volume_count: u32,
) -> Result<PackageIndex, HdpError> {
    let mut cursor = SliceCursor::new(bytes);
    let file_count = cursor.u32()?;
    let mut files = BTreeMap::new();
    for _ in 0..file_count {
        let path_len = cursor.u32()? as usize;
        let path = std::str::from_utf8(cursor.take(path_len)?)
            .map_err(|_| HdpError::InvalidFormat("file path is not UTF-8".into()))?
            .to_string();
        validate_path(&path)?;
        let decoded_size = cursor.u64()?;
        let chunk_count = cursor.u32()?;
        let mut chunks = Vec::with_capacity(chunk_count as usize);
        for _ in 0..chunk_count {
            let volume = cursor.u32()?;
            let compression = CompressionMethod::from_id(cursor.u8()?);
            let encryption = EncryptionMethod::from_id(cursor.u8()?);
            cursor.take(2)?;
            chunks.push(ChunkDescriptor {
                volume,
                compression,
                encryption,
                offset: cursor.u64()?,
                stored_size: cursor.u64()?,
                decoded_size: cursor.u64()?,
                checksum: cursor.u64()?,
            });
        }
        let entry = FileEntry {
            path: path.clone(),
            decoded_size,
            chunks,
        };
        if files.insert(path.clone(), entry).is_some() {
            return Err(HdpError::DuplicatePath(path));
        }
    }
    if !cursor.is_empty() {
        return Err(HdpError::InvalidFormat("trailing bytes in index".into()));
    }
    Ok(PackageIndex {
        package_id,
        volume_count,
        files,
    })
}

pub(crate) fn validate_path(path: &str) -> Result<(), HdpError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(HdpError::InvalidPath(path.to_string()));
    }
    Ok(())
}

pub(crate) fn checksum64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

struct SliceCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SliceCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], HdpError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| HdpError::InvalidFormat("index offset overflow".into()))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| HdpError::InvalidFormat("truncated index".into()))?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, HdpError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, HdpError> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .expect("four-byte slice must convert to an array");
        Ok(u32::from_le_bytes(raw))
    }

    fn u64(&mut self) -> Result<u64, HdpError> {
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .expect("eight-byte slice must convert to an array");
        Ok(u64::from_le_bytes(raw))
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}
