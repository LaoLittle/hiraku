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
    let snapshot = proto::SaveGameData::from(data);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("hiraku-slots-{}", uuid::Uuid::new_v4()))
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
