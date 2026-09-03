use std::path::{Path, PathBuf};

use crate::{ByteStorage, StorageError, validate_key};

/// Filesystem-backed storage selected on native targets.
#[derive(Clone, Debug)]
pub struct PlatformStorage {
    root: PathBuf,
    extension: String,
}

impl PlatformStorage {
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

pub fn native_path(root: &Path, key: &str, extension: &str) -> Result<PathBuf, StorageError> {
    validate_key(key)?;
    Ok(root.join(format!("{key}.{extension}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_backend_roundtrips_and_removes_bytes() {
        let root = std::env::temp_dir().join(format!("hiraku-storage-test-{}", std::process::id()));
        let storage = PlatformStorage::new(&root, "test", "bin");
        storage.write("quick", b"save payload").expect("write succeeds");
        assert_eq!(storage.read("quick").expect("read succeeds"), Some(b"save payload".to_vec()));
        storage.remove("quick").expect("remove succeeds");
        assert_eq!(storage.read("quick").expect("missing read succeeds"), None);
        std::fs::remove_dir(root).expect("temporary directory is empty");
    }
}
