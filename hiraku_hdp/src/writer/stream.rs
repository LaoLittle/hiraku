//! Unknown-length generated assets: spool compressed chunks, then publish headers.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct StreamPackageBuilder {
    spool: File,
    _scratch: Scratch,
    files: BTreeMap<String, (bool, FileEntry)>,
    options: PackOptions,
}

impl StreamPackageBuilder {
    pub fn new(options: PackOptions) -> Result<Self, HdpError> {
        validate_pack_options(options)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (scratch, spool) = loop {
            let path = std::env::temp_dir().join(format!(
                "hiraku-hdp-{}-{}.spool",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break (path, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        Ok(Self {
            spool,
            _scratch: Scratch(scratch),
            files: BTreeMap::new(),
            options,
        })
    }

    /// Consume one asset without retaining other source assets in memory.
    pub fn add_reader(
        &mut self,
        path: &str,
        mut reader: impl Read,
        options: FileOptions,
    ) -> Result<(), HdpError> {
        validate_path(path)?;
        if self.files.contains_key(path) {
            return Err(HdpError::DuplicatePath(path.into()));
        }
        let mut buffer = vec![0; self.options.chunk_size];
        let mut chunks = Vec::new();
        let mut decoded_size = 0;
        loop {
            let mut count = 0;
            while count < buffer.len() {
                match reader.read(&mut buffer[count..]) {
                    Ok(0) => break,
                    Ok(size) => count += size,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            if count == 0 {
                break;
            }
            let bytes = &buffer[..count];
            let (compression, encoded) = encode(
                bytes,
                options.compression.unwrap_or(self.options.compression),
            )?;
            let offset = self.spool.seek(SeekFrom::End(0))?;
            self.spool.write_all(&encoded)?;
            chunks.push(ChunkDescriptor {
                volume: 0,
                offset,
                stored_size: encoded.len() as u64,
                decoded_size: count as u64,
                checksum: checksum64(bytes),
                compression,
                encryption: EncryptionMethod::NONE,
            });
            decoded_size += count as u64;
        }
        self.files.insert(
            path.into(),
            (
                options.bootstrap,
                FileEntry {
                    path: path.into(),
                    decoded_size,
                    chunks,
                },
            ),
        );
        Ok(())
    }

    pub fn write_to(mut self, output: impl AsRef<Path>) -> Result<WrittenPackage, HdpError> {
        let output = output.as_ref();
        let mut files = std::mem::take(&mut self.files)
            .into_values()
            .collect::<Vec<_>>();
        files.sort_by(|a, b| (!a.0, &a.1.path).cmp(&(!b.0, &b.1.path)));
        let index_size = encoded_index_size(
            &files
                .iter()
                .map(|(_, entry)| (entry.path.clone(), entry.chunks.len()))
                .collect::<Vec<_>>(),
        )?;
        let first = HEADER_SIZE
            .checked_add(index_size)
            .ok_or_else(|| HdpError::InvalidFormat("index size overflow".into()))?;
        if self
            .options
            .max_volume_size
            .is_some_and(|limit| first > limit)
        {
            return Err(HdpError::InvalidFormat(
                "volume zero is too small for the index".into(),
            ));
        }
        if let Some(limit) = self.options.max_volume_size {
            let bootstrap_size = files
                .iter()
                .filter(|(bootstrap, _)| *bootstrap)
                .flat_map(|(_, file)| &file.chunks)
                .map(|chunk| chunk.stored_size)
                .sum::<u64>();
            if first as u64 + bootstrap_size > limit as u64 {
                return Err(HdpError::InvalidFormat(
                    "bootstrap data does not fit in volume zero".into(),
                ));
            }
            if files
                .iter()
                .flat_map(|(_, file)| &file.chunks)
                .any(|chunk| chunk.stored_size + HEADER_SIZE as u64 > limit as u64)
            {
                return Err(HdpError::InvalidFormat(
                    "encoded chunk exceeds volume capacity; reduce chunk_size".into(),
                ));
            }
        }
        // Stable content identity, independent of temporary offsets/insertion order.
        let mut identity = Vec::new();
        for (_, file) in &files {
            identity.extend_from_slice(file.path.as_bytes());
            identity.push(0);
            identity.extend_from_slice(&file.decoded_size.to_le_bytes());
            for chunk in &file.chunks {
                identity.extend_from_slice(&chunk.checksum.to_le_bytes());
            }
        }
        let package_id = checksum64(&identity);
        let mut volumes = vec![create_volume(output, 0, first as u64)?];
        let mut sizes = vec![first as u64];
        let mut buffer = vec![0; 64 * 1024];
        for (bootstrap, file) in &mut files {
            for chunk in &mut file.chunks {
                let mut volume = volumes.len() - 1;
                if self
                    .options
                    .max_volume_size
                    .is_some_and(|limit| sizes[volume] + chunk.stored_size > limit as u64)
                {
                    if *bootstrap {
                        return Err(HdpError::InvalidFormat(format!(
                            "bootstrap data does not fit in volume zero (`{}`)",
                            file.path
                        )));
                    }
                    volume += 1;
                    volumes.push(create_volume(output, volume, HEADER_SIZE as u64)?);
                    sizes.push(HEADER_SIZE as u64);
                }
                self.spool.seek(SeekFrom::Start(chunk.offset))?;
                volumes[volume].seek(SeekFrom::Start(sizes[volume]))?;
                copy_exact(
                    &mut self.spool,
                    &mut volumes[volume],
                    chunk.stored_size,
                    &mut buffer,
                    &file.path,
                )?;
                chunk.volume = u32::try_from(volume)
                    .map_err(|_| HdpError::InvalidFormat("too many volumes".into()))?;
                chunk.offset = sizes[volume];
                sizes[volume] += chunk.stored_size;
            }
        }
        let entries = files
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        let index = encode_index(&entries)?;
        let volume_count = u32::try_from(volumes.len())
            .map_err(|_| HdpError::InvalidFormat("too many volumes".into()))?;
        for (number, volume) in volumes.iter_mut().enumerate() {
            volume.seek(SeekFrom::Start(0))?;
            volume.write_all(&encode_header(VolumeHeader {
                package_id,
                volume_index: number as u32,
                volume_count,
                index_size: if number == 0 { index.len() as u64 } else { 0 },
                data_offset: if number == 0 {
                    first as u64
                } else {
                    HEADER_SIZE as u64
                },
                index_checksum: if number == 0 { checksum64(&index) } else { 0 },
            }))?;
            if number == 0 {
                volume.write_all(&index)?;
            }
            volume.set_len(sizes[number])?;
            volume.flush()?;
        }
        remove_stale_volumes(output, volumes.len())?;
        Ok(WrittenPackage {
            index: PackageIndex {
                package_id,
                volume_count,
                files: entries
                    .into_iter()
                    .map(|entry| (entry.path.clone(), entry))
                    .collect(),
            },
            volume_sizes: sizes,
        })
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        // Declared after the file so its handle is closed before cleanup.
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_readers_split_and_roundtrip_without_source_files() {
        let temp = super::super::tests::TestDirectory::new();
        let options = PackOptions {
            chunk_size: 256,
            max_volume_size: Some(4096),
            compression: CompressionOptions {
                method: CompressionMethod::STORED,
                ..Default::default()
            },
        };
        let mut writer = StreamPackageBuilder::new(options).expect("spool");
        let scratch = writer._scratch.0.clone();
        writer
            .add_reader(
                "startup.hks",
                &b"alice"[..],
                FileOptions {
                    bootstrap: true,
                    ..Default::default()
                },
            )
            .expect("bootstrap");
        writer
            .add_reader("empty", io::empty(), FileOptions::default())
            .expect("empty file");
        let bytes = (0..8192).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        writer
            .add_reader(
                "texture.uastc.ktx2",
                bytes.as_slice(),
                FileOptions::default(),
            )
            .expect("generated reader");
        assert!(
            writer
                .add_reader("empty", io::empty(), FileOptions::default())
                .is_err()
        );
        assert!(
            writer
                .add_reader("../escape", io::empty(), FileOptions::default())
                .is_err()
        );
        let output = temp.path.join("generated.hdp");
        let package = writer.write_to(&output).expect("publish volumes");
        assert!(package.volume_count() > 1);
        assert!(package.volume_sizes.iter().all(|size| *size <= 4096));
        assert!(
            !scratch.exists(),
            "spool cleaned up after successful publication"
        );
        let archive = crate::Archive::open(&output).expect("archive");
        assert_eq!(
            archive
                .read_file("texture.uastc.ktx2")
                .expect("generated texture"),
            bytes
        );
        assert_eq!(
            archive.read_file("startup.hks").expect("bootstrap"),
            b"alice"
        );
        assert!(archive.read_file("empty").expect("empty").is_empty());
    }
}
