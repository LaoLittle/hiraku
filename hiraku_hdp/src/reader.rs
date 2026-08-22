use std::{
    fs,
    path::Path,
    sync::{Arc, OnceLock},
};

use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};

use crate::{
    ChunkDescriptor, HdpError, PackageIndex,
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
    volumes: Vec<OnceLock<Arc<[u8]>>>,
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

    /// Opens the package index and volume zero without requiring later volumes.
    pub fn from_first_volume(bytes: impl Into<Arc<[u8]>>) -> Result<Self, HdpError> {
        let first = bytes.into();
        let index = Self::read_index(&first)?;
        let volumes = (0..index.volume_count)
            .map(|_| OnceLock::new())
            .collect::<Vec<_>>();
        volumes[0]
            .set(first)
            .expect("volume zero slot must be empty during archive construction");
        let archive = Self { index, volumes };
        archive.validate_index()?;
        archive.validate_volume(0)?;
        Ok(archive)
    }

    pub fn from_volumes(volumes: impl IntoIterator<Item = Arc<[u8]>>) -> Result<Self, HdpError> {
        let mut volumes = volumes.into_iter();
        let first = volumes.next().ok_or(HdpError::MissingVolume(0))?;
        let archive = Self::from_first_volume(first)?;
        for (position, volume) in volumes.enumerate() {
            archive.provide_volume((position + 1) as u32, volume)?;
        }
        if !archive.is_complete() {
            return Err(HdpError::MissingVolume(
                archive.first_missing_volume().unwrap_or(0),
            ));
        }
        Ok(archive)
    }

    /// Validates and publishes one physical volume. Each slot can be filled once.
    pub fn provide_volume(&self, position: u32, volume: Arc<[u8]>) -> Result<(), HdpError> {
        let slot = self
            .volumes
            .get(position as usize)
            .ok_or(HdpError::MissingVolume(position))?;
        if slot.get().is_some() {
            return Err(HdpError::InvalidFormat(format!(
                "volume {position} was provided more than once"
            )));
        }
        self.validate_volume_bytes(position as usize, &volume)?;
        slot.set(volume).map_err(|_| {
            HdpError::InvalidFormat(format!("volume {position} was provided more than once"))
        })?;
        Ok(())
    }

    pub fn is_volume_available(&self, volume: u32) -> bool {
        self.volumes
            .get(volume as usize)
            .is_some_and(|slot| slot.get().is_some())
    }

    pub fn is_complete(&self) -> bool {
        self.volumes.iter().all(|volume| volume.get().is_some())
    }

    fn first_missing_volume(&self) -> Option<u32> {
        self.volumes
            .iter()
            .position(|volume| volume.get().is_none())
            .map(|position| position as u32)
    }

    fn validate_volume(&self, position: usize) -> Result<(), HdpError> {
        let volume = self.volumes[position]
            .get()
            .ok_or(HdpError::MissingVolume(position as u32))?;
        self.validate_volume_bytes(position, volume)
    }

    fn validate_volume_bytes(&self, position: usize, volume: &[u8]) -> Result<(), HdpError> {
        let header = decode_header(volume)?;
        if header.package_id != self.index.package_id {
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
        if header.volume_count != self.index.volume_count {
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
        for file in self.index.files.values() {
            for chunk in file
                .chunks
                .iter()
                .filter(|chunk| chunk.volume == position as u32)
            {
                let end = chunk.offset.checked_add(chunk.stored_size).ok_or_else(|| {
                    HdpError::InvalidFormat(format!("chunk offset overflows for `{}`", file.path))
                })?;
                if end > volume.len() as u64 {
                    return Err(HdpError::InvalidFormat(format!(
                        "chunk range is outside volume {position} for `{}`",
                        file.path
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_index(&self) -> Result<(), HdpError> {
        for file in self.index.files.values() {
            let mut decoded_size = 0_u64;
            for chunk in &file.chunks {
                if chunk.volume >= self.index.volume_count {
                    return Err(HdpError::InvalidFormat(format!(
                        "chunk references missing volume {} for `{}`",
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
        Ok(())
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

        let decoded_chunks = file
            .chunks
            .par_iter()
            .enumerate()
            .map(|(idx, chunk)| self.decode_chunk(idx, chunk, path))
            .collect::<Result<Vec<_>, _>>()?;

        for decoded in decoded_chunks {
            output.extend_from_slice(&decoded);
        }

        if output.len() != capacity {
            return Err(HdpError::InvalidFormat(format!(
                "decoded size mismatch for `{path}`"
            )));
        }
        Ok(output)
    }

    fn decode_chunk(
        &self,
        chunk_index: usize,
        chunk: &ChunkDescriptor,
        path: &str,
    ) -> Result<Vec<u8>, HdpError> {
        if chunk.encryption.id() != 0 {
            return Err(HdpError::UnsupportedEncryption(chunk.encryption.id()));
        }
        let volume = self
            .volumes
            .get(chunk.volume as usize)
            .and_then(OnceLock::get)
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
        let decoded = decode(chunk.compression, stored).map_err(|_| HdpError::CorruptChunk {
            path: path.to_string(),
            chunk: chunk_index,
        })?;
        if decoded.len() as u64 != chunk.decoded_size || checksum64(&decoded) != chunk.checksum {
            return Err(HdpError::CorruptChunk {
                path: path.to_string(),
                chunk: chunk_index,
            });
        }

        Ok(decoded)
    }
}
