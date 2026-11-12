//! Slot indexes are independent of the versioned execution snapshot.
use super::*;
use hiraku_storage::GenerationRecord;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveSlotMetadata {
    pub version: u32,
    pub resume_script: String,
    pub has_thumbnail: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlotIndex {
    format: u32,
    generation: String,
    metadata: SaveSlotMetadata,
}

fn store(root: &Path, extension: &str) -> PlatformStorage {
    PlatformStorage::new(root, SAVE_NAMESPACE, extension)
}

fn index_key(slot: &str) -> Result<String, StorageError> {
    sanitize_slot_name(slot)?;
    Ok(format!("slot-{}", blake3::hash(slot.as_bytes()).to_hex()))
}

fn read_index(root: &Path, slot: &str) -> Result<Option<SlotIndex>, StorageError> {
    let Some(bytes) = store(root, "hson").read(&index_key(slot)?)? else {
        return Ok(None);
    };
    let index: SlotIndex =
        hson::from_slice(&bytes).map_err(|error| StorageError::HsonData(error.to_string()))?;
    if index.format != 1
        || uuid::Uuid::parse_str(&index.generation).is_err()
        || index.generation.len() != 32
    {
        return Err(StorageError::InvalidSave(
            "invalid or unsupported slot index".into(),
        ));
    }
    Ok(Some(index))
}

fn read_generation(
    root: &Path,
    index: &SlotIndex,
    kind: &str,
    extension: &str,
) -> Result<Vec<u8>, StorageError> {
    let key = format!("{kind}-{}", index.generation);
    store(root, extension)
        .read(&key)?
        .ok_or_else(|| StorageError::InvalidSave(format!("missing generation record `{key}`")))
}

pub fn save_slot_exists(slot: &str) -> Result<bool, StorageError> {
    let root = save_root_path();
    slot_exists_at(&root, slot)
}

pub fn delete_save_slot(slot: &str) -> Result<(), StorageError> {
    delete_slot_at(&save_root_path(), slot)
}

fn delete_slot_at(root: &Path, slot: &str) -> Result<(), StorageError> {
    let key = index_key(slot)?;
    let index = match read_index(root, slot) {
        Ok(index) => index,
        Err(StorageError::InvalidSave(_) | StorageError::HsonData(_)) => None,
        Err(error) => return Err(error),
    };
    let mut records = vec![hiraku_storage::RemovalRecord {
        key,
        extension: "hson".into(),
    }];
    if let Some(index) = index {
        for (kind, extension) in [("snapshot", "sav"), ("thumbnail", "png")] {
            records.push(hiraku_storage::RemovalRecord {
                key: format!("{kind}-{}", index.generation),
                extension: extension.into(),
            });
        }
    }
    save_storage(root).enqueue_remove(records)?;
    Ok(())
}

fn slot_exists_at(root: &Path, slot: &str) -> Result<bool, StorageError> {
    Ok(store(root, "hson").contains(&index_key(slot)?)?)
}

fn require_index(root: &Path, slot: &str) -> Result<SlotIndex, StorageError> {
    read_index(root, slot)?.ok_or_else(|| StorageError::MissingSlot(slot.to_owned()))
}

pub fn load_save_data(slot: &str) -> Result<SaveGameData, StorageError> {
    load_save_data_from_root(&save_root_path(), slot)
}

pub fn load_save_data_from_root(root: &Path, slot: &str) -> Result<SaveGameData, StorageError> {
    let index = require_index(root, slot)?;
    let payload = read_generation(root, &index, "snapshot", "sav")?;
    decode_save_data(&payload)
}

pub fn load_save_metadata(slot: &str) -> Result<SaveSlotMetadata, StorageError> {
    metadata_from_root(&save_root_path(), slot)
}

fn metadata_from_root(root: &Path, slot: &str) -> Result<SaveSlotMetadata, StorageError> {
    Ok(require_index(root, slot)?.metadata)
}

pub fn load_save_thumbnail(slot: &str) -> Result<Vec<u8>, StorageError> {
    thumbnail_from_root(&save_root_path(), slot)
}

fn thumbnail_from_root(root: &Path, slot: &str) -> Result<Vec<u8>, StorageError> {
    let index = require_index(root, slot)?;
    if index.metadata.has_thumbnail {
        read_generation(root, &index, "thumbnail", "png")
    } else {
        Ok(Vec::new())
    }
}

pub fn write_save_data_to_root(
    root: &Path,
    slot: &str,
    data: &SaveGameData,
) -> Result<(), StorageError> {
    let key = index_key(slot)?;
    let previous = match read_index(root, slot) {
        Ok(index) => index,
        Err(StorageError::InvalidSave(_) | StorageError::HsonData(_)) => None,
        Err(error) => return Err(error),
    };
    let generation = uuid::Uuid::new_v4().simple().to_string();
    let index = SlotIndex {
        format: 1,
        generation: generation.clone(),
        metadata: SaveSlotMetadata {
            version: data.version,
            resume_script: data.resume_script.clone(),
            has_thumbnail: !data.thumbnail_png.is_empty(),
        },
    };
    let snapshot = proto::SaveGameData::try_from(data)?;
    let mut records = vec![GenerationRecord {
        key: format!("snapshot-{generation}"),
        extension: "sav".into(),
        payload: snapshot.encode_to_vec(),
    }];
    if index.metadata.has_thumbnail {
        records.push(GenerationRecord {
            key: format!("thumbnail-{generation}"),
            extension: "png".into(),
            payload: data.thumbnail_png.clone(),
        });
    }
    records.push(GenerationRecord {
        key,
        extension: "hson".into(),
        payload: hson::to_vec(&index).map_err(|error| StorageError::HsonData(error.to_string()))?,
    });
    save_storage(root).enqueue_generation(records)?;
    if let Some(previous) = previous {
        let retired = [("snapshot", "sav"), ("thumbnail", "png")]
            .into_iter()
            .map(|(kind, extension)| hiraku_storage::RemovalRecord {
                key: format!("{kind}-{}", previous.generation),
                extension: extension.into(),
            })
            .collect();
        // The publication precedes reclamation in the storage queue. Never
        // remove the old generation before a replacement has been published.
        if let Err(error) = save_storage(root).enqueue_remove(retired) {
            bevy::log::warn!(
                "slot `{slot}` was saved, but its retired generation could not be reclaimed: {error}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("hiraku-slots-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn repeated_overwrite_reclaims_only_the_replaced_generation_and_its_preview() {
        let root = temp_root();
        let mut data = SaveGameData {
            thumbnail_png: vec![137, 80, 78, 71],
            ..Default::default()
        };
        write_save_data_to_root(&root, "bob", &data).expect("independent slot");
        let bob = require_index(&root, "bob").expect("independent generation");
        write_save_data_to_root(&root, "alice", &data).expect("initial generation");
        for _ in 0..8 {
            let previous = require_index(&root, "alice").expect("previous index");
            write_save_data_to_root(&root, "alice", &data).expect("publish before reclaiming");
            let current = require_index(&root, "alice").expect("current index");
            assert_ne!(current.generation, previous.generation);
            assert!(load_save_data_from_root(&root, "alice").is_ok());
            assert_eq!(
                thumbnail_from_root(&root, "alice").expect("current preview"),
                data.thumbnail_png
            );
            assert!(
                !root
                    .join(format!("snapshot-{}.sav", previous.generation))
                    .exists()
            );
            assert!(
                !root
                    .join(format!("thumbnail-{}.png", previous.generation))
                    .exists()
            );
            assert!(
                root.join(format!("snapshot-{}.sav", bob.generation))
                    .exists()
            );
            assert!(
                root.join(format!("thumbnail-{}.png", bob.generation))
                    .exists()
            );
        }
        let previous = require_index(&root, "alice").expect("last illustrated generation");
        data.thumbnail_png.clear();
        write_save_data_to_root(&root, "alice", &data).expect("overwrite without a preview");
        assert!(
            thumbnail_from_root(&root, "alice")
                .expect("empty preview")
                .is_empty()
        );
        assert!(
            !root
                .join(format!("thumbnail-{}.png", previous.generation))
                .exists()
        );
        assert_eq!(std::fs::read_dir(&root).expect("test directory").count(), 5);
        std::fs::remove_dir_all(root).expect("remove synthetic generations");
    }

    #[test]
    fn a_cleanup_error_does_not_report_a_published_replacement_as_a_failed_save() {
        let root = temp_root();
        let data = SaveGameData::default();
        write_save_data_to_root(&root, "alice", &data).expect("initial slot");
        let previous = require_index(&root, "alice").expect("initial index");
        let old_snapshot = root.join(format!("snapshot-{}.sav", previous.generation));
        std::fs::remove_file(&old_snapshot).expect("remove synthetic payload");
        std::fs::create_dir(&old_snapshot)
            .expect("simulate an obsolete payload that cannot be unlinked");
        write_save_data_to_root(&root, "alice", &data)
            .expect("replacement was published successfully");
        let current = require_index(&root, "alice").expect("replacement index");
        assert_ne!(current.generation, previous.generation);
        assert!(load_save_data_from_root(&root, "alice").is_ok());
        std::fs::remove_dir_all(root).expect("remove synthetic generations");
    }

    #[test]
    fn deleting_a_slot_removes_its_generation_without_decoding_or_touching_other_slots() {
        let root = temp_root();
        let data = SaveGameData {
            thumbnail_png: vec![137, 80, 78, 71],
            ..Default::default()
        };
        for slot in ["alice", "bob"] {
            write_save_data_to_root(&root, slot, &data).expect("write synthetic slot");
        }
        let index = require_index(&root, "alice").expect("slot index");
        store(&root, "sav")
            .enqueue_write(&format!("snapshot-{}", index.generation), b"corrupt")
            .expect("corrupt synthetic snapshot");
        delete_slot_at(&root, "alice").expect("delete without decoding snapshot");
        assert!(!slot_exists_at(&root, "alice").expect("deleted index"));
        assert!(
            !store(&root, "sav")
                .contains(&format!("snapshot-{}", index.generation))
                .expect("deleted snapshot")
        );
        assert!(slot_exists_at(&root, "bob").expect("other slot retained"));
        assert!(
            !store(&root, "png")
                .contains(&format!("thumbnail-{}", index.generation))
                .expect("deleted preview")
        );
        delete_slot_at(&root, "alice").expect("repeated deletion is idempotent");
        assert!(delete_slot_at(&root, "../bob").is_err());
        store(&root, "hson")
            .enqueue_write(&index_key("alice").expect("key"), b"corrupt")
            .expect("corrupt synthetic index");
        delete_slot_at(&root, "alice").expect("delete malformed index");
        assert!(!slot_exists_at(&root, "alice").expect("malformed slot removed"));
        std::fs::remove_dir_all(&root).expect("remove synthetic test directory");
    }

    #[test]
    fn preview_survives_missing_and_corrupt_snapshot_and_overwrite() {
        let root = temp_root();
        let mut data = SaveGameData {
            thumbnail_png: vec![137, 80, 78, 71],
            ..Default::default()
        };
        write_save_data_to_root(&root, "alice", &data).expect("publish slot");
        assert!(
            load_save_data_from_root(&root, "alice")
                .expect("snapshot")
                .thumbnail_png
                .is_empty()
        );
        let index = require_index(&root, "alice").expect("index");
        let snapshot = root.join(format!("snapshot-{}.sav", index.generation));
        std::fs::write(&snapshot, [0xff]).expect("corrupt synthetic snapshot");
        assert!(load_save_data_from_root(&root, "alice").is_err());
        assert!(
            metadata_from_root(&root, "alice")
                .expect("metadata")
                .has_thumbnail
        );
        assert_eq!(
            thumbnail_from_root(&root, "alice").expect("preview"),
            data.thumbnail_png
        );
        std::fs::remove_file(snapshot).expect("remove synthetic snapshot");
        assert!(load_save_data_from_root(&root, "alice").is_err());
        assert_eq!(
            thumbnail_from_root(&root, "alice").expect("preview"),
            data.thumbnail_png
        );
        data.thumbnail_png.clear();
        write_save_data_to_root(&root, "alice", &data).expect("overwrite");
        assert!(
            !metadata_from_root(&root, "alice")
                .expect("metadata")
                .has_thumbnail
        );
        assert!(
            thumbnail_from_root(&root, "alice")
                .expect("no preview")
                .is_empty()
        );
        std::fs::remove_dir_all(root).expect("remove test generation files");
    }

    #[test]
    fn metadata_required_and_incompatible_snapshot_does_not_hide_preview() {
        let root = temp_root();
        save_storage(&root)
            .enqueue_write("bob", b"old snapshot")
            .expect("fixture");
        assert!(!slot_exists_at(&root, "bob").expect("slot lookup"));
        assert!(load_save_data_from_root(&root, "bob").is_err());
        assert!(thumbnail_from_root(&root, "bob").is_err());
        let data = SaveGameData {
            version: CURRENT_SAVE_VERSION + 1,
            thumbnail_png: vec![1, 2],
            ..Default::default()
        };
        write_save_data_to_root(&root, "bob", &data).expect("publish");
        assert!(slot_exists_at(&root, "bob").expect("slot lookup"));
        assert!(load_save_data_from_root(&root, "bob").is_err());
        assert_eq!(
            thumbnail_from_root(&root, "bob").expect("preview"),
            data.thumbnail_png
        );
        assert_eq!(
            metadata_from_root(&root, "bob").expect("metadata").version,
            data.version
        );
        assert!(write_save_data_to_root(&root, "../alice", &data).is_err());
        std::fs::remove_dir_all(root).expect("remove test generation files");
    }
}
