use std::path::{Path, PathBuf};

use crate::{ByteStorage, StorageError, validate_key};

/// Filesystem-backed storage selected on native targets.
#[derive(Clone, Debug)]
pub struct PlatformStorage {
    root: PathBuf,
    extension: String,
}

impl PlatformStorage {
    pub(super) fn publish_generation(
        &self,
        records: &[crate::GenerationRecord],
    ) -> Result<(), StorageError> {
        use std::io::Write;
        crate::validate_generation(records)?;
        std::fs::create_dir_all(&self.root)?;
        for record in &records[..records.len() - 1] {
            let path = native_path(&self.root, &record.key, &record.extension)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(&record.payload)?;
            file.sync_all()?;
        }
        let index = records.last().expect("validated nonempty generation");
        let path = native_path(&self.root, &index.key, &index.extension)?;
        static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = self.root.join(format!(
            ".{}-{}-{sequence}.tmp",
            index.key,
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            file.write_all(&index.payload)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        Ok(())
    }

    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        let _ = namespace.into();
        Self {
            root: root.into(),
            extension: extension.into(),
        }
    }

    fn file_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        native_path(&self.root, key, &self.extension)
    }
}

impl ByteStorage for PlatformStorage {
    fn contains(&self, key: &str) -> Result<bool, StorageError> {
        match std::fs::metadata(self.file_path(key)?) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match std::fs::read(self.file_path(key)?) {
            Ok(payload) => Ok(Some(payload)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError> {
        let path = self.file_path(key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, payload).map_err(Into::into)
    }

    fn remove(&self, key: &str) -> Result<(), StorageError> {
        match std::fs::remove_file(self.file_path(key)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn native_path(root: &Path, key: &str, extension: &str) -> Result<PathBuf, StorageError> {
    validate_key(key)?;
    Ok(root.join(format!("{key}.{extension}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_collision_preserves_published_index() {
        let root =
            std::env::temp_dir().join(format!("hiraku-generation-test-{}", std::process::id()));
        let storage = PlatformStorage::new(&root, "test", "hson");
        let record = |key: &str, payload: &[u8]| crate::GenerationRecord {
            key: key.into(),
            extension: "hson".into(),
            payload: payload.into(),
        };
        storage
            .publish_generation(&[record("alice", b"snapshot"), record("slot", b"first")])
            .expect("publish");
        assert!(
            storage
                .publish_generation(&[record("alice", b"replacement"), record("slot", b"second")])
                .is_err()
        );
        assert_eq!(
            storage.read("slot").expect("index"),
            Some(b"first".to_vec())
        );
        storage
            .publish_generation(&[record("bob", b"snapshot"), record("slot", b"second")])
            .expect("replace index");
        assert_eq!(
            storage.read("slot").expect("index"),
            Some(b"second".to_vec())
        );
        for key in ["alice", "bob", "slot"] {
            storage.remove(key).expect("remove fixture");
        }
        std::fs::remove_dir(root).expect("remove fixture directory");
    }

    #[test]
    fn native_backend_roundtrips_and_removes_bytes() {
        let root = std::env::temp_dir().join(format!("hiraku-storage-test-{}", std::process::id()));
        let storage = PlatformStorage::new(&root, "test", "bin");
        storage
            .write("quick", b"save payload")
            .expect("write succeeds");
        assert!(storage.contains("quick").expect("existence query succeeds"));
        assert!(storage.contains("../invalid").is_err());
        assert_eq!(
            storage.read("quick").expect("read succeeds"),
            Some(b"save payload".to_vec())
        );
        storage.remove("quick").expect("remove succeeds");
        assert!(
            !storage
                .contains("quick")
                .expect("missing key query succeeds")
        );
        assert_eq!(storage.read("quick").expect("missing read succeeds"), None);
        std::fs::remove_dir(root).expect("temporary directory is empty");
    }
}
