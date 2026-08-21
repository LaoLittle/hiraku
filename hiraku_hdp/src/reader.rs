use std::{fs, path::Path, sync::Arc};

use crate::{
    HdpError, PackageIndex,
    codec::decode,
    format::{HEADER_SIZE, checksum64, decode_header, decode_index},
};

/// An opened HDP package backed by one or more complete physical volumes.
///
/// The owning representation is suitable for the current Bevy asset loader.
/// A future range-backed reader can reuse [`PackageIndex`] and the same chunk
/// decoder without requiring the whole package to be resident.
#[derive(Clone, Debug)]
pub struct Archive {
    index: PackageIndex,
    volumes: Vec<Arc<[u8]>>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HdpError> {
        let path = path.as_ref();
        let first = Arc::<[u8]>::from(fs::read(path)?);
        let index = Self::read_index(&first)?;
        let mut volumes = Vec::with_capacity(index.volume_count as usize);
        volumes.push(first);
        for volume in 1..index.volume_count {
            let volume_path = format!("{}.{volume:03}", path.display());
            volumes.push(Arc::<[u8]>::from(fs::read(volume_path)?));
        }
        Self::from_volumes(volumes)
    }

    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self, HdpError> {
        Self::from_volumes([bytes.into()])
    }

    pub fn from_volumes(volumes: impl IntoIterator<Item = Arc<[u8]>>) -> Result<Self, HdpError> {
        let volumes = volumes.into_iter().collect::<Vec<_>>();
        let first = volumes.first().ok_or(HdpError::MissingVolume(0))?;
        let index = Self::read_index(first)?;
        if volumes.len() != index.volume_count as usize {
            return Err(HdpError::MissingVolume(volumes.len() as u32));
        }

        for (position, volume) in volumes.iter().enumerate() {
            let header = decode_header(volume)?;
            if header.package_id != index.package_id {
                return Err(HdpError::InvalidFormat(format!(
                    "volume {position} belongs to another package"
                )));
            }
            if header.volume_index != position as u32 {
                return Err(HdpError::InvalidFormat(format!(
                    "expected volume {position}, found {}",
                    header.volume_index
                )));
            }
            if header.volume_count != index.volume_count {
                return Err(HdpError::InvalidFormat(format!(
                    "volume {position} has an inconsistent volume count"
                )));
            }
            if position != 0 && (header.index_size != 0 || header.index_checksum != 0) {
                return Err(HdpError::InvalidFormat(format!(
                    "volume {position} unexpectedly contains an index"
                )));
            }
            if header.data_offset < HEADER_SIZE as u64 || header.data_offset > volume.len() as u64 {
                return Err(HdpError::InvalidFormat(format!(
                    "volume {position} has an invalid data offset"
                )));
            }
        }

        for file in index.files.values() {
            let mut decoded_size = 0_u64;
            for chunk in &file.chunks {
                let volume = volumes
                    .get(chunk.volume as usize)
                    .ok_or(HdpError::MissingVolume(chunk.volume))?;
                let end = chunk.offset.checked_add(chunk.stored_size).ok_or_else(|| {
                    HdpError::InvalidFormat(format!("chunk offset overflows for `{}`", file.path))
                })?;
                if end > volume.len() as u64 {
                    return Err(HdpError::InvalidFormat(format!(
                        "chunk range is outside volume {} for `{}`",
                        chunk.volume, file.path
                    )));
                }
                if !chunk.compression.is_supported() {
                    return Err(HdpError::UnsupportedCompression(chunk.compression.id()));
                }
                if !chunk.encryption.is_supported() {
                    return Err(HdpError::UnsupportedEncryption(chunk.encryption.id()));
                }
                decoded_size = decoded_size
                    .checked_add(chunk.decoded_size)
                    .ok_or_else(|| {
                        HdpError::InvalidFormat(format!(
                            "decoded size overflows for `{}`",
                            file.path
                        ))
                    })?;
            }
            if decoded_size != file.decoded_size {
                return Err(HdpError::InvalidFormat(format!(
                    "file size does not match its chunks for `{}`",
                    file.path
                )));
            }
        }

        Ok(Self { index, volumes })
    }

    pub fn read_index(first_volume: &[u8]) -> Result<PackageIndex, HdpError> {
        let header = decode_header(first_volume)?;
        if header.volume_index != 0 {
            return Err(HdpError::InvalidFormat(
                "the first input is not volume zero".into(),
            ));
        }
        if header.volume_count == 0 {
            return Err(HdpError::InvalidFormat("volume count is zero".into()));
        }
        let index_start = HEADER_SIZE;
        let index_end = index_start
            .checked_add(
                usize::try_from(header.index_size)
                    .map_err(|_| HdpError::InvalidFormat("index is too large".into()))?,
            )
            .ok_or_else(|| HdpError::InvalidFormat("index offset overflow".into()))?;
        if header.data_offset != index_end as u64 {
            return Err(HdpError::InvalidFormat(
                "volume zero data offset does not follow its index".into(),
            ));
        }
        let bytes = first_volume
            .get(index_start..index_end)
            .ok_or_else(|| HdpError::InvalidFormat("truncated package index".into()))?;
        if checksum64(bytes) != header.index_checksum {
            return Err(HdpError::InvalidFormat(
                "package index checksum does not match".into(),
            ));
        }
        decode_index(bytes, header.package_id, header.volume_count)
    }

    pub fn index(&self) -> &PackageIndex {
        &self.index
    }

    pub fn contains(&self, path: &str) -> bool {
        self.index.files.contains_key(path)
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.index.files.keys().map(String::as_str)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, HdpError> {
        let file = self
            .index
            .files
            .get(path)
            .ok_or_else(|| HdpError::MissingFile(path.to_string()))?;
        let capacity = usize::try_from(file.decoded_size)
            .map_err(|_| HdpError::InvalidFormat(format!("`{path}` is too large")))?;
        let mut output = Vec::with_capacity(capacity);

        for (chunk_index, chunk) in file.chunks.iter().enumerate() {
            if chunk.encryption.id() != 0 {
                return Err(HdpError::UnsupportedEncryption(chunk.encryption.id()));
            }
            let volume = self
                .volumes
                .get(chunk.volume as usize)
                .ok_or(HdpError::MissingVolume(chunk.volume))?;
            let start = usize::try_from(chunk.offset).map_err(|_| HdpError::CorruptChunk {
                path: path.to_string(),
                chunk: chunk_index,
            })?;
            let end = usize::try_from(chunk.offset + chunk.stored_size).map_err(|_| {
                HdpError::CorruptChunk {
                    path: path.to_string(),
                    chunk: chunk_index,
                }
            })?;
            let stored = volume
                .get(start..end)
                .ok_or_else(|| HdpError::CorruptChunk {
                    path: path.to_string(),
                    chunk: chunk_index,
                })?;
            let decoded =
                decode(chunk.compression, stored).map_err(|_| HdpError::CorruptChunk {
                    path: path.to_string(),
                    chunk: chunk_index,
                })?;
            if decoded.len() as u64 != chunk.decoded_size || checksum64(&decoded) != chunk.checksum
            {
                return Err(HdpError::CorruptChunk {
                    path: path.to_string(),
                    chunk: chunk_index,
                });
            }
            output.extend_from_slice(&decoded);
        }

        if output.len() != capacity {
            return Err(HdpError::InvalidFormat(format!(
                "decoded size mismatch for `{path}`"
            )));
        }
        Ok(output)
    }
}
