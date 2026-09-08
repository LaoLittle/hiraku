//! Project-owned durable flags, deliberately outside SaveGameData.
use hiraku_storage::{BufferedStorage as PlatformStorage, StorageError};
#[cfg(test)]
use hiraku_storage::ByteStorage;

pub(super) fn backend() -> PlatformStorage {
    PlatformStorage::new(
        super::workspace_base_path().join("profile"),
        "hiraku.profile",
        "flag",
    )
}

fn key(id: &str) -> Result<String, StorageError> {
    if id.trim().is_empty() {
        return Err(StorageError::InvalidKey);
    }
    Ok(blake3::hash(id.as_bytes()).to_hex().to_string())
}

pub fn read_bool(id: &str) -> Result<bool, StorageError> {
    decode(backend().read(&key(id)?)?.as_deref())
}

pub fn write_bool(id: &str, value: bool) -> Result<(), StorageError> {
    if read_bool(id)? != value {
        backend().enqueue_write(&key(id)?, if value { b"true" } else { b"false" })?;
    }
    Ok(())
}

#[cfg(test)]
fn read_from(store: &impl ByteStorage, id: &str) -> Result<bool, StorageError> {
    decode(store.read(&key(id)?)?.as_deref())
}

fn decode(payload: Option<&[u8]>) -> Result<bool, StorageError> {
    match payload {
        None | Some(b"false") => Ok(false),
        Some(b"true") => Ok(true),
        Some(_) => Err(StorageError::Corrupt("invalid profile boolean".into())),
    }
}

#[cfg(test)]
fn write_to(store: &impl ByteStorage, id: &str, value: bool) -> Result<(), StorageError> {
    if read_from(store, id)? == value {
        return Ok(());
    }
    store.write(&key(id)?, if value { b"true" } else { b"false" })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct Memory(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>);
    impl ByteStorage for Memory {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.0.lock().expect("test store").get(key).cloned())
        }
        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
            self.0
                .lock()
                .expect("test store")
                .insert(key.into(), bytes.to_vec());
            Ok(())
        }
        fn remove(&self, key: &str) -> Result<(), StorageError> {
            self.0.lock().expect("test store").remove(key);
            Ok(())
        }
    }
    #[test]
    fn flags_survive_reopening_and_are_not_save_slots() {
        let profile = Memory::default();
        let saves = Memory::default();
        assert!(!read_from(&profile, "alice/cg/1").expect("missing flag"));
        write_to(&profile, "alice/cg/1", true).expect("unlock");
        saves.write("bob", b"old story state").expect("save");
        saves.remove("bob").expect("remove save");
        let reopened = profile.clone();
        assert!(read_from(&reopened, "alice/cg/1").expect("persistent flag"));
        reopened
            .write(&key("corrupt").expect("key"), b"bad")
            .expect("corrupt fixture");
        assert!(read_from(&reopened, "corrupt").is_err());
    }
}
