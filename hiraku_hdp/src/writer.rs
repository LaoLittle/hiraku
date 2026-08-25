use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::{
    ChunkDescriptor, CompressionMethod, CompressionOptions, EncryptionMethod, FileEntry, HdpError,
    PackageIndex,
    codec::{encode, encode_stream},
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

/// Metadata returned after a package has been streamed directly to disk.
#[derive(Clone, Debug)]
pub struct WrittenPackage {
    pub index: PackageIndex,
    pub volume_sizes: Vec<u64>,
}

impl WrittenPackage {
    pub fn volume_count(&self) -> usize {
        self.volume_sizes.len()
    }

    pub fn stored_size(&self) -> u64 {
        self.volume_sizes.iter().sum()
    }
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
            .into_par_iter()
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

/// Packs a directory directly into one or more HDP files.
///
/// Only the index and one encoded chunk are retained in memory. Volume headers
/// and the first-volume index are reserved up front and backfilled with `Seek`
/// after all stored sizes and volume assignments are known.
pub fn pack_directory_to(
    root: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: PackOptions,
) -> Result<WrittenPackage, HdpError> {
    pack_directory_to_with(root, output, options, |_| FileOptions::default())
}

pub fn pack_directory_to_with(
    root: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: PackOptions,
    mut file_options: impl FnMut(&str) -> FileOptions,
) -> Result<WrittenPackage, HdpError> {
    validate_pack_options(options)?;
    let root = root.as_ref();
    let output = output.as_ref();
    let mut source_paths = Vec::new();
    collect_files(root, &mut source_paths)?;
    source_paths.sort();

    let mut files = Vec::with_capacity(source_paths.len());
    for source_path in source_paths {
        let path = source_path
            .strip_prefix(root)
            .map_err(|_| HdpError::InvalidPath(source_path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_path(&path)?;
        let decoded_size = source_path.metadata()?.len();
        let chunk_count = chunk_count(decoded_size, options.chunk_size)?;
        files.push(DirectoryFile {
            source_path,
            options: file_options(&path),
            path,
            decoded_size,
            chunk_count,
        });
    }

    let package_id = package_id_from_paths(&files)?;
    files.sort_by(|left, right| {
        (!left.options.bootstrap, &left.path).cmp(&(!right.options.bootstrap, &right.path))
    });
    let index_size = encoded_index_size(
        &files
            .iter()
            .map(|file| (file.path.clone(), file.chunk_count))
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

    let mut volumes = vec![create_volume(output, 0, first_data_offset as u64)?];
    let mut volume_sizes = vec![first_data_offset as u64];
    let mut entries = Vec::with_capacity(files.len());
    let mut encoded_chunk = Vec::new();
    let mut copy_buffer = vec![0_u8; 64 * 1024];

    for file in files {
        let mut source = File::open(&file.source_path)?;
        if source.metadata()?.len() != file.decoded_size {
            return Err(source_changed(&file.path));
        }
        let compression = file.options.compression.unwrap_or(options.compression);
        let mut chunks = Vec::with_capacity(file.chunk_count);
        let mut remaining = file.decoded_size;

        while remaining > 0 {
            let decoded_size = remaining.min(options.chunk_size as u64);
            let source_offset = file.decoded_size - remaining;
            let mut checksum_reader = ChecksumReader::new((&mut source).take(decoded_size));
            let method = encode_stream(
                &mut checksum_reader,
                decoded_size as usize,
                compression,
                &mut encoded_chunk,
            )?;
            if checksum_reader.bytes_read != decoded_size {
                return Err(source_changed(&file.path));
            }
            let checksum = checksum_reader.checksum;
            let stored_size = if method == CompressionMethod::STORED {
                decoded_size
            } else {
                encoded_chunk.len() as u64
            };

            let mut volume_index = volumes.len() - 1;
            if options.max_volume_size.is_some_and(|limit| {
                volume_sizes[volume_index].saturating_add(stored_size) > limit as u64
            }) {
                if file.options.bootstrap {
                    return Err(HdpError::InvalidFormat(format!(
                        "bootstrap data does not fit in volume zero (`{}`)",
                        file.path
                    )));
                }
                volume_index += 1;
                volumes.push(create_volume(output, volume_index, HEADER_SIZE as u64)?);
                volume_sizes.push(HEADER_SIZE as u64);
            }

            let offset = volume_sizes[volume_index];
            let volume = &mut volumes[volume_index];
            volume.seek(SeekFrom::Start(offset))?;
            if method == CompressionMethod::STORED {
                source.seek(SeekFrom::Start(source_offset))?;
                copy_exact(
                    &mut source,
                    volume,
                    decoded_size,
                    &mut copy_buffer,
                    &file.path,
                )?;
            } else {
                volume.write_all(&encoded_chunk)?;
            }
            volume_sizes[volume_index] = offset
                .checked_add(stored_size)
                .ok_or_else(|| HdpError::InvalidFormat("volume size overflow".into()))?;
            chunks.push(ChunkDescriptor {
                volume: volume_index as u32,
                offset,
                stored_size,
                decoded_size,
                checksum,
                compression: method,
                encryption: EncryptionMethod::NONE,
            });
            remaining -= decoded_size;
        }

        entries.push(FileEntry {
            path: file.path,
            decoded_size: file.decoded_size,
            chunks,
        });
    }

    let volume_count = u32::try_from(volumes.len())
        .map_err(|_| HdpError::InvalidFormat("too many volumes".into()))?;
    let encoded_index = encode_index(&entries)?;
    if encoded_index.len() != index_size {
        return Err(HdpError::InvalidFormat(
            "calculated index size does not match encoded index".into(),
        ));
    }
    let index_checksum = checksum64(&encoded_index);
    for (volume_index, volume) in volumes.iter_mut().enumerate() {
        volume.seek(SeekFrom::Start(0))?;
        volume.write_all(&encode_header(VolumeHeader {
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
            index_checksum: if volume_index == 0 { index_checksum } else { 0 },
        }))?;
        if volume_index == 0 {
            volume.write_all(&encoded_index)?;
        }
        volume.set_len(volume_sizes[volume_index])?;
        volume.flush()?;
    }
    remove_stale_volumes(output, volumes.len())?;

    Ok(WrittenPackage {
        index: PackageIndex {
            package_id,
            volume_count,
            files: entries
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect(),
        },
        volume_sizes,
    })
}

struct DirectoryFile {
    source_path: PathBuf,
    path: String,
    decoded_size: u64,
    chunk_count: usize,
    options: FileOptions,
}

fn validate_pack_options(options: PackOptions) -> Result<(), HdpError> {
    if options.chunk_size == 0 {
        return Err(HdpError::InvalidFormat(
            "writer chunk size cannot be zero".into(),
        ));
    }
    Ok(())
}

fn chunk_count(size: u64, chunk_size: usize) -> Result<usize, HdpError> {
    if size == 0 {
        return Ok(0);
    }
    let chunk_size = chunk_size as u64;
    usize::try_from((size - 1) / chunk_size + 1)
        .map_err(|_| HdpError::InvalidFormat("file has too many chunks".into()))
}

fn package_id_from_paths(files: &[DirectoryFile]) -> Result<u64, HdpError> {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    for file in files {
        update_hash(&mut hash, file.path.as_bytes());
        update_hash(&mut hash, &[0xff]);
        let mut source = File::open(&file.source_path)?;
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            update_hash(&mut hash, &buffer[..read]);
        }
        update_hash(&mut hash, &[0xfe]);
    }
    Ok(hash)
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

struct ChecksumReader<R> {
    inner: R,
    checksum: u64,
    bytes_read: u64,
}

impl<R> ChecksumReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            checksum: 0xcbf29ce484222325,
            bytes_read: 0,
        }
    }
}

impl<R: Read> Read for ChecksumReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        update_hash(&mut self.checksum, &buffer[..read]);
        self.bytes_read += read as u64;
        Ok(read)
    }
}

fn copy_exact(
    source: &mut File,
    destination: &mut File,
    mut remaining: u64,
    buffer: &mut [u8],
    path: &str,
) -> Result<(), HdpError> {
    while remaining > 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        let read = source.read(&mut buffer[..requested])?;
        if read == 0 {
            return Err(source_changed(path));
        }
        destination.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    Ok(())
}

fn create_volume(path: &Path, index: usize, data_offset: u64) -> Result<File, HdpError> {
    let path = volume_path(path, index);
    let mut file = File::create(path)?;
    file.set_len(data_offset)?;
    file.seek(SeekFrom::Start(data_offset))?;
    Ok(file)
}

fn volume_path(path: &Path, index: usize) -> PathBuf {
    if index == 0 {
        path.to_path_buf()
    } else {
        PathBuf::from(format!("{}.{index:03}", path.display()))
    }
}

fn source_changed(path: &str) -> HdpError {
    HdpError::InvalidFormat(format!("source file changed while packing (`{path}`)"))
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
        let volume_path = volume_path(path, index);
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
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

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
        let streaming = Archive::from_first_volume(Arc::<[u8]>::from(package.volumes[0].clone()))
            .expect("volume zero must open independently");
        assert_eq!(
            streaming.read_file("startup.story.hks").unwrap(),
            vec![b's'; 256]
        );
        assert!(matches!(
            streaming.read_file("voice/chapter.ogg"),
            Err(HdpError::MissingVolume(_))
        ));
        for (volume, bytes) in package.volumes.iter().enumerate().skip(1) {
            streaming
                .provide_volume(volume as u32, Arc::<[u8]>::from(bytes.clone()))
                .expect("streamed volume must validate");
        }
        assert_eq!(
            streaming.read_file("voice/chapter.ogg").unwrap(),
            vec![b'v'; 1500]
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

    #[test]
    fn streams_a_split_directory_package_directly_to_disk() {
        let temporary = TestDirectory::new();
        let source = temporary.path.join("source");
        fs::create_dir_all(source.join("scripts")).expect("test source directory must be created");
        fs::create_dir_all(source.join("audio")).expect("test source directory must be created");
        fs::write(source.join("scripts/start.hks"), vec![b's'; 700])
            .expect("bootstrap fixture must be written");
        let audio = (0_u8..=255).cycle().take(2400).collect::<Vec<_>>();
        fs::write(source.join("audio/track.bin"), &audio).expect("stored fixture must be written");
        let output = temporary.path.join("content.hdp");

        let written = pack_directory_to_with(
            &source,
            &output,
            PackOptions {
                chunk_size: 256,
                max_volume_size: Some(900),
                ..Default::default()
            },
            |path| FileOptions {
                bootstrap: path.ends_with(".hks"),
                compression: path.ends_with(".bin").then_some(CompressionOptions {
                    method: CompressionMethod::STORED,
                    ..Default::default()
                }),
            },
        )
        .expect("streaming package must be written");

        assert!(written.volume_count() > 1);
        assert!(
            written.index.files["scripts/start.hks"]
                .chunks
                .iter()
                .all(|chunk| chunk.volume == 0)
        );
        assert_eq!(
            written.stored_size(),
            written
                .volume_sizes
                .iter()
                .enumerate()
                .map(|(index, _)| fs::metadata(volume_path(&output, index)).unwrap().len())
                .sum::<u64>()
        );

        let archive = Archive::open(&output).expect("streamed package must reopen");
        assert_eq!(
            archive
                .read_file("scripts/start.hks")
                .expect("bootstrap file must decode"),
            vec![b's'; 700]
        );
        assert_eq!(
            archive
                .read_file("audio/track.bin")
                .expect("stored file must decode"),
            audio
        );
    }

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock must follow the Unix epoch")
                .as_nanos();
            let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hiraku-hdp-streaming-{}-{timestamp}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test directory must be created");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
