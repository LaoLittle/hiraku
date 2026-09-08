use super::native::PlatformStorage;
use crate::{ByteStorage, RuntimeStorageStatus, StorageError, WriteQueued};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct BufferedStorage(PlatformStorage);
impl BufferedStorage {
    pub fn enqueue_generation(
        &self,
        records: Vec<crate::GenerationRecord>,
    ) -> Result<WriteQueued, StorageError> {
        self.0.publish_generation(&records)?;
        Ok(WriteQueued)
    }
    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self(PlatformStorage::new(root, namespace, extension))
    }
    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.0.read(key)
    }
    pub fn contains(&self, key: &str) -> Result<bool, StorageError> {
        self.0.contains(key)
    }
    pub fn enqueue_write(&self, key: &str, payload: &[u8]) -> Result<WriteQueued, StorageError> {
        self.0.write(key, payload)?;
        Ok(WriteQueued)
    }
}
pub fn initialize_runtime(_project: &str, _stores: Vec<BufferedStorage>) {}
pub fn runtime_status() -> RuntimeStorageStatus {
    RuntimeStorageStatus::Ready
}
