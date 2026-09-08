use super::native::PlatformStorage;
use crate::{AsyncByteStorage, ByteStorage, StorageError};
use std::path::PathBuf;

/// Filesystem work runs on a blocking pool, not on the caller's executor.
#[derive(Clone, Debug)]
pub struct AsyncPlatformStorage(PlatformStorage);

impl AsyncPlatformStorage {
    pub fn new(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        extension: impl Into<String>,
    ) -> Self {
        Self(PlatformStorage::new(root, namespace, extension))
    }
}

impl AsyncByteStorage for AsyncPlatformStorage {
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let store = self.0.clone();
        let key = crate::validate_key(key)?.to_owned();
        blocking::unblock(move || store.read(&key)).await
    }
    async fn write(&self, key: &str, payload: &[u8]) -> Result<(), StorageError> {
        let store = self.0.clone();
        let key = crate::validate_key(key)?.to_owned();
        let payload = payload.to_vec();
        blocking::unblock(move || store.write(&key, &payload)).await
    }
    async fn remove(&self, key: &str) -> Result<(), StorageError> {
        let store = self.0.clone();
        let key = crate::validate_key(key)?.to_owned();
        blocking::unblock(move || store.remove(&key)).await
    }
    async fn contains(&self, key: &str) -> Result<bool, StorageError> {
        let store = self.0.clone();
        let key = crate::validate_key(key)?.to_owned();
        blocking::unblock(move || store.contains(&key)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(PathBuf);
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if self.0.exists() {
                std::fs::remove_dir_all(&self.0).expect("remove owned test directory");
            }
        }
    }

    #[test]
    fn async_storage_roundtrip_reopen_remove_and_validation() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock")
            .as_nanos();
        let root = TestDirectory(
            std::env::temp_dir().join(format!("hiraku-storage-{}-{unique}", std::process::id())),
        );
        futures_lite::future::block_on(async {
            let store = AsyncPlatformStorage::new(&root.0, "alice", "bin");
            assert!(store.read("../bob").await.is_err());
            assert!(store.write("", b"invalid").await.is_err());
            assert!(!root.0.exists(), "invalid keys must not create files");
            assert_eq!(store.read("alice").await.expect("missing read"), None);
            let bytes = vec![0, 1, 128, 255];
            store.write("alice", &bytes).await.expect("write completes");
            let reopened = AsyncPlatformStorage::new(&root.0, "alice", "bin");
            assert_eq!(
                reopened.read("alice").await.expect("persisted bytes"),
                Some(bytes)
            );
            store.write("alice", &[]).await.expect("empty payload");
            assert!(store.contains("alice").await.expect("empty payload exists"));
            assert_eq!(store.read("alice").await.expect("empty read"), Some(vec![]));
            store.remove("alice").await.expect("remove completes");
            assert!(!reopened.contains("alice").await.expect("removed"));
            store
                .remove("alice")
                .await
                .expect("removing missing key is harmless");
        });
    }
}
