use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
};

use bevy::{asset::io::file::FileAssetReader, prelude::Resource};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::state::{SaveGameData, StoredValue};

const SAVE_ROOT: &str = "saves";
const USER_SETTINGS_PATH: &str = "config/hiraku.toml";

#[derive(Clone, Debug, Resource, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default = "default_volume")]
    pub bgm_volume: f32,
    #[serde(default = "default_volume")]
    pub voice_volume: f32,
    #[serde(default = "default_volume")]
    pub sfx_volume: f32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            bgm_volume: 1.0,
            voice_volume: 1.0,
            sfx_volume: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SaveSlotSummary {
    pub slot: String,
    pub resume_script: String,
    pub route: Option<String>,
    pub background: Option<String>,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to access storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse toml: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("failed to serialize toml: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("slot name can only contain letters, digits, '-' or '_'")]
    InvalidSlot,
}

pub fn save_root_path() -> PathBuf {
    FileAssetReader::get_base_path().join(SAVE_ROOT)
}

pub fn read_user_settings() -> Result<UserSettings, StorageError> {
    let path = FileAssetReader::get_base_path().join(USER_SETTINGS_PATH);
    match fs::read_to_string(path) {
        Ok(payload) => Ok(toml::from_str(&payload)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(UserSettings::default()),
        Err(err) => Err(StorageError::Io(err)),
    }
}

pub fn write_user_settings(settings: &UserSettings) -> Result<(), StorageError> {
    let path = FileAssetReader::get_base_path().join(USER_SETTINGS_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = toml::to_string_pretty(settings)?;
    fs::write(path, payload)?;
    Ok(())
}

pub fn load_save_data(slot: &str) -> Result<SaveGameData, StorageError> {
    let path = slot_path(slot)?;
    let payload = fs::read_to_string(path)?;
    Ok(toml::from_str(&payload)?)
}

pub fn list_save_slots() -> Result<Vec<SaveSlotSummary>, StorageError> {
    let root = save_root_path();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(StorageError::Io(err)),
    };

    let mut slots = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }

        let Some(slot) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };

        let Ok(payload) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = toml::from_str::<SaveGameData>(&payload) else {
            continue;
        };

        slots.push(SaveSlotSummary {
            slot: slot.to_string(),
            resume_script: data.resume_script,
            route: global_string(&data.globals, "route"),
            background: data.scene.background.map(|background| background.path),
        });
    }

    slots.sort_by(|left, right| right.slot.cmp(&left.slot));
    Ok(slots)
}

fn slot_path(slot: &str) -> Result<PathBuf, StorageError> {
    let slot = sanitize_slot_name(slot)?;
    Ok(save_root_path().join(format!("{slot}.toml")))
}

fn sanitize_slot_name(slot: &str) -> Result<&str, StorageError> {
    if slot.is_empty() {
        return Err(StorageError::InvalidSlot);
    }

    if slot
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(slot)
    } else {
        Err(StorageError::InvalidSlot)
    }
}

fn global_string(globals: &BTreeMap<String, StoredValue>, key: &str) -> Option<String> {
    match globals.get(key) {
        Some(StoredValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn default_volume() -> f32 {
    1.0
}
