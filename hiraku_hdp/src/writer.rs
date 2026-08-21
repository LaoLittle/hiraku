use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ChunkDescriptor, CompressionMethod, CompressionOptions, EncryptionMethod, FileEntry, HdpError,
    PackageIndex,
    codec::encode,
    format::{
        HEADER_SIZE, VolumeHeader, checksum64, encode_header, encode_index, encoded_index_size,
        validate_path,
    },
};

#[derive(Clone, Copy, Debug, Default)]
pub struct FileOptions {
    /// Place this file before non-bootstrap data and require it to remain in volume zero.
    pub bootstrap: bool,
    /// Override the package compression policy for this file.
    pub compression: Option<CompressionOptions>,
}

#[derive(Clone, Copy, Debug)]
pub struct PackOptions {
    pub chunk_size: usize,
    /// Target physical volume size. `None` emits one desktop package file.
    pub max_volume_size: Option<usize>,
    pub compression: CompressionOptions,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            chunk_size: 1024 * 1024,
            max_volume_size: None,
            compression: CompressionOptions::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackageOutput {
    pub index: PackageIndex,
    pub volumes: Vec<Vec<u8>>,
}

#[derive(Default)]
pub struct PackageBuilder {
    files: BTreeMap<String, SourceFile>,
}

struct SourceFile {
    bytes: Vec<u8>,
    options: FileOptions,
}

struct PreparedFile {
    path: String,
    decoded_size: u64,
    bootstrap: bool,
    chunks: Vec<PreparedChunk>,
}

struct PreparedChunk {
    bytes: Vec<u8>,
    decoded_size: u64,
    checksum: u64,
    compression: CompressionMethod,
    descriptor: Option<ChunkDescriptor>,
}

impl PackageBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(
        &mut self,
        path: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), HdpError> {
        self.add_file_with_options(path, bytes, FileOptions::default())
    }

    pub fn add_file_with_options(
        &mut self,
        path: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        options: FileOptions,
    ) -> Result<(), HdpError> {
        let path = path.into();
        validate_path(&path)?;
        if self
            .files
            .insert(
                path.clone(),
                SourceFile {
                    bytes: bytes.into(),
                    options,
                },
            )
            .is_some()
        {
            return Err(HdpError::DuplicatePath(path));
        }
        Ok(())
    }

    pub fn build(self, options: PackOptions) -> Result<PackageOutput, HdpError> {
        if options.chunk_size == 0 {
            return Err(HdpError::InvalidFormat(
                "writer chunk size cannot be zero".into(),
            ));
        }

        let package_id = package_id(&self.files);
        let mut prepared = self
            .files
            .into_iter()
            .map(|(path, source)| prepare_file(path, source, options))
            .collect::<Result<Vec<_>, HdpError>>()?;
        prepared.sort_by(|left, right| {
            (!left.bootstrap, &left.path).cmp(&(!right.bootstrap, &right.path))
        });

        let index_size = encoded_index_size(
            &prepared
                .iter()
                .map(|file| (file.path.clone(), file.chunks.len()))
                .collect::<Vec<_>>(),
        )?;
        let first_data_offset = HEADER_SIZE
            .checked_add(index_size)
            .ok_or_else(|| HdpError::InvalidFormat("index offset overflow".into()))?;
        if options
            .max_volume_size
            .is_some_and(|limit| first_data_offset > limit)
        {
            return Err(HdpError::InvalidFormat(
                "volume zero is too small for the HDP index".into(),
            ));
        }

        let mut volumes = vec![vec![0; first_data_offset]];
        let mut current_volume = 0_usize;
        for file in &mut prepared {
            for chunk in &mut file.chunks {
                let limit = options.max_volume_size;
                let would_exceed = limit.is_some_and(|limit| {
                    volumes[current_volume]
                        .len()
                        .saturating_add(chunk.bytes.len())
                        > limit
                });
                if would_exceed {
                    if file.bootstrap {
                        return Err(HdpError::InvalidFormat(format!(
                            "bootstrap data does not fit in volume zero (`{}`)",
                            file.path
                        )));
                    }
                    current_volume += 1;
                    volumes.push(vec![0; HEADER_SIZE]);
                }

                let offset = volumes[current_volume].len() as u64;
                volumes[current_volume].extend_from_slice(&chunk.bytes);
                chunk.descriptor = Some(ChunkDescriptor {
                    volume: current_volume as u32,
                    offset,
                    stored_size: chunk.bytes.len() as u64,
                    decoded_size: chunk.decoded_size,
                    checksum: chunk.checksum,
                    compression: chunk.compression,
                    encryption: EncryptionMethod::NONE,
                });
            }
        }

        let files = prepared
            .into_iter()
            .map(|file| FileEntry {
                path: file.path,
                decoded_size: file.decoded_size,
                chunks: file
                    .chunks
                    .into_iter()
                    .map(|chunk| {
                        chunk
                            .descriptor
                            .expect("every prepared chunk must be assigned to a volume")
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let encoded_index = encode_index(&files)?;
        if encoded_index.len() != index_size {
            return Err(HdpError::InvalidFormat(
                "calculated index size does not match encoded index".into(),
            ));
        }
        volumes[0][HEADER_SIZE..first_data_offset].copy_from_slice(&encoded_index);

        let volume_count = u32::try_from(volumes.len())
            .map_err(|_| HdpError::InvalidFormat("too many volumes".into()))?;
        for (volume_index, volume) in volumes.iter_mut().enumerate() {
            let header = encode_header(VolumeHeader {
                package_id,
                volume_index: volume_index as u32,
                volume_count,
                index_size: if volume_index == 0 {
                    index_size as u64
                } else {
                    0
                },
                data_offset: if volume_index == 0 {
                    first_data_offset as u64
                } else {
                    HEADER_SIZE as u64
                },
                index_checksum: if volume_index == 0 {
                    checksum64(&encoded_index)
                } else {
                    0
                },
            });
            volume[..HEADER_SIZE].copy_from_slice(&header);
        }

        let index = PackageIndex {
            package_id,
            volume_count,
            files: files
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect(),
        };
        Ok(PackageOutput { index, volumes })
    }
}

fn prepare_file(
    path: String,
    source: SourceFile,
    options: PackOptions,
) -> Result<PreparedFile, HdpError> {
    let compression = source.options.compression.unwrap_or(options.compression);
    let mut chunks = Vec::new();
    for raw in source.bytes.chunks(options.chunk_size) {
        let (method, bytes) = encode(raw, compression)?;
        chunks.push(PreparedChunk {
            bytes,
            decoded_size: raw.len() as u64,
            checksum: checksum64(raw),
            compression: method,
            descriptor: None,
        });
    }
    Ok(PreparedFile {
        path,
        decoded_size: source.bytes.len() as u64,
        bootstrap: source.options.bootstrap,
        chunks,
    })
}

fn package_id(files: &BTreeMap<String, SourceFile>) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for (path, file) in files {
        for byte in path
            .as_bytes()
            .iter()
            .copied()
            .chain([0xff])
            .chain(file.bytes.iter().copied())
            .chain([0xfe])
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

pub fn pack_directory(
    root: impl AsRef<Path>,
    options: PackOptions,
) -> Result<PackageOutput, HdpError> {
    pack_directory_with(root, options, |_| FileOptions::default())
}

pub fn pack_directory_with(
    root: impl AsRef<Path>,
    options: PackOptions,
    mut file_options: impl FnMut(&str) -> FileOptions,
) -> Result<PackageOutput, HdpError> {
    let root = root.as_ref();
    let mut paths = Vec::new();
    collect_files(root, &mut paths)?;
    paths.sort();
    let mut builder = PackageBuilder::new();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| HdpError::InvalidPath(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let options = file_options(&relative);
        builder.add_file_with_options(relative, fs::read(path)?, options)?;
    }
    builder.build(options)
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), HdpError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name() == ".DS_Store" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_files(&path, output)?;
        } else if entry.file_type()?.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

pub fn write_package(path: impl AsRef<Path>, package: &PackageOutput) -> Result<(), HdpError> {
    let path = path.as_ref();
    for (index, volume) in package.volumes.iter().enumerate() {
        let volume_path = if index == 0 {
            path.to_path_buf()
        } else {
            PathBuf::from(format!("{}.{index:03}", path.display()))
        };
        fs::write(volume_path, volume)?;
    }
    remove_stale_volumes(path, package.volumes.len())?;
    Ok(())
}

fn remove_stale_volumes(path: &Path, volume_count: usize) -> Result<(), HdpError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(index) = suffix.parse::<usize>() else {
            continue;
        };
        if index == 0 || index >= volume_count {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::Archive;

    use super::*;

    #[test]
    fn roundtrips_stored_and_zstd_chunks() {
        let mut builder = PackageBuilder::new();
        builder.add_file("empty.hson", []).unwrap();
        builder
            .add_file("scripts/start.hks", vec![b'a'; 4000])
            .unwrap();
        builder
            .add_file_with_options(
                "textures/noise.bin",
                (0_u8..=255).cycle().take(2048).collect::<Vec<_>>(),
                FileOptions {
                    compression: Some(CompressionOptions {
                        method: CompressionMethod::STORED,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let package = builder
            .build(PackOptions {
                chunk_size: 512,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(&package.volumes[0][..crate::MAGIC.len()], &crate::MAGIC);
        assert_eq!(crate::HEADER_SIZE, 52);
        let archive = Archive::from_bytes(Arc::<[u8]>::from(package.volumes[0].clone())).unwrap();
        assert_eq!(archive.read_file("empty.hson").unwrap(), b"");
        assert_eq!(
            archive.read_file("scripts/start.hks").unwrap(),
            vec![b'a'; 4000]
        );
        assert_eq!(archive.index().files["scripts/start.hks"].chunks.len(), 8);
        assert!(
            archive.index().files["scripts/start.hks"]
                .chunks
                .iter()
                .all(|chunk| chunk.compression == CompressionMethod::ZSTD)
        );
    }

    #[test]
    fn split_volumes_keep_bootstrap_data_in_the_first_volume() {
        let mut builder = PackageBuilder::new();
        builder
            .add_file_with_options(
                "startup.story.hks",
                vec![b's'; 256],
                FileOptions {
                    bootstrap: true,
                    compression: Some(CompressionOptions {
                        method: CompressionMethod::STORED,
                        ..Default::default()
                    }),
                },
            )
            .unwrap();
        builder
            .add_file_with_options(
                "voice/chapter.ogg",
                vec![b'v'; 1500],
                FileOptions {
                    compression: Some(CompressionOptions {
                        method: CompressionMethod::STORED,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let package = builder
            .build(PackOptions {
                chunk_size: 300,
                max_volume_size: Some(800),
                ..Default::default()
            })
            .unwrap();
        assert!(package.volumes.len() > 1);
        assert!(
            package.index.files["startup.story.hks"]
                .chunks
                .iter()
                .all(|chunk| chunk.volume == 0)
        );
        let archive =
            Archive::from_volumes(package.volumes.iter().cloned().map(Arc::<[u8]>::from)).unwrap();
        assert_eq!(
            archive.read_file("voice/chapter.ogg").unwrap(),
            vec![b'v'; 1500]
        );
    }

    #[test]
    fn rejects_parent_path_components() {
        let mut builder = PackageBuilder::new();
        assert!(builder.add_file("../secret", b"no").is_err());
    }
}
