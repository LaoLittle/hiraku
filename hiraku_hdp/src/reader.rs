use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::{
    ChunkDescriptor, HdpError, PackageIndex,
    codec::decode,
    format::{HEADER_SIZE, checksum64, decode_header, decode_index},
};

/// An indexed HDP package. `open` range-reads local files; `from_*` retains
/// supplied volume bytes for hosts without filesystem access.
#[derive(Clone, Debug)]
pub struct Archive {
    index: PackageIndex,
    volumes: Vec<OnceLock<Arc<[u8]>>>,
    /// File-backed packages retain paths and the index, not compressed payloads.
    files: Option<Vec<PathBuf>>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HdpError> {
        let path = path.as_ref();
        let mut first = fs::File::open(path)?;
        let mut header_bytes = vec![0; HEADER_SIZE];
        first.read_exact(&mut header_bytes)?;
        let header = decode_header(&header_bytes)?;
        let index_size = usize::try_from(header.index_size)
            .map_err(|_| HdpError::InvalidFormat("index is too large".into()))?;
        let prefix_size = HEADER_SIZE
            .checked_add(index_size)
            .ok_or_else(|| HdpError::InvalidFormat("index offset overflow".into()))?;
        if prefix_size as u64 > first.metadata()?.len() {
            return Err(HdpError::InvalidFormat("truncated package index".into()));
        }
        header_bytes.resize(prefix_size, 0);
        first.read_exact(&mut header_bytes[HEADER_SIZE..])?;
        let index = Self::read_index(&header_bytes)?;
        let mut files = vec![path.to_path_buf()];
        for volume in 1..index.volume_count {
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".{volume:03}"));
            files.push(PathBuf::from(name));
        }
        let archive = Self {
            volumes: (0..index.volume_count).map(|_| OnceLock::new()).collect(),
            index,
            files: Some(files),
        };
        archive.validate_index()?;
        for (position, path) in archive
            .files
            .as_ref()
            .expect("file-backed archive")
            .iter()
            .enumerate()
        {
            let mut file = fs::File::open(path)?;
            let mut header = [0; HEADER_SIZE];
            file.read_exact(&mut header)?;
            archive.validate_volume_extent(position, &header, file.metadata()?.len())?;
        }
        Ok(archive)
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
        let archive = Self {
            index,
            volumes,
            files: None,
        };
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
        if self.files.is_some() {
            return Err(HdpError::InvalidFormat(
                "cannot publish bytes to a file-backed archive".into(),
            ));
        }
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
        if let Some(files) = &self.files {
            return (volume as usize) < files.len();
        }
        self.volumes
            .get(volume as usize)
            .is_some_and(|slot| slot.get().is_some())
    }

    pub fn is_complete(&self) -> bool {
        self.files.is_some() || self.volumes.iter().all(|volume| volume.get().is_some())
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
        self.validate_volume_extent(position, volume, volume.len() as u64)
    }

    fn validate_volume_extent(
        &self,
        position: usize,
        volume: &[u8],
        length: u64,
    ) -> Result<(), HdpError> {
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
        if header.data_offset < HEADER_SIZE as u64 || header.data_offset > length {
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
                if end > length {
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

    /// Compressed bytes retained by this reader, excluding decoded consumers.
    pub fn resident_bytes(&self) -> usize {
        self.volumes
            .iter()
            .filter_map(OnceLock::get)
            .map(|v| v.len())
            .sum()
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

        // Bound scratch memory to one chunk, not a second complete decoded file.
        for (index, chunk) in file.chunks.iter().enumerate() {
            self.decode_chunk(index, chunk, path, &mut output)?;
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
        buffer: &mut Vec<u8>,
    ) -> Result<(), HdpError> {
        if chunk.encryption.id() != 0 {
            return Err(HdpError::UnsupportedEncryption(chunk.encryption.id()));
        }
        let disk_bytes;
        let stored = if let Some(files) = &self.files {
            let file_path = files
                .get(chunk.volume as usize)
                .ok_or(HdpError::MissingVolume(chunk.volume))?;
            let mut file = fs::File::open(file_path)?;
            file.seek(SeekFrom::Start(chunk.offset))?;
            let length =
                usize::try_from(chunk.stored_size).map_err(|_| HdpError::CorruptChunk {
                    path: path.into(),
                    chunk: chunk_index,
                })?;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes)?;
            disk_bytes = bytes;
            disk_bytes.as_slice()
        } else {
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
            volume
                .get(start..end)
                .ok_or_else(|| HdpError::CorruptChunk {
                    path: path.to_string(),
                    chunk: chunk_index,
                })?
        };
        let last = buffer.len();
        buffer.reserve(
            chunk
                .decoded_size
                .try_into()
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::FileTooLarge))?,
        );
        decode(chunk.compression, stored, buffer).map_err(|_| HdpError::CorruptChunk {
            path: path.to_string(),
            chunk: chunk_index,
        })?;
        let decoded = &buffer[last..];
        if decoded.len() as u64 != chunk.decoded_size || checksum64(decoded) != chunk.checksum {
            return Err(HdpError::CorruptChunk {
                path: path.to_string(),
                chunk: chunk_index,
            });
        }

        Ok(())
    }
}
