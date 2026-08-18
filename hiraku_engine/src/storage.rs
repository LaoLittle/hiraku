use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::Resource;
use prost::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    data::evaluate_hks_map,
    proto,
    state::{
        AudioSnapshot, DialogueSnapshot, ImageLayerSnapshot, SaveCheckpoint, SaveGameData,
        SavedInput, SceneSnapshot, ScriptPosition, SpriteSnapshot, StoredValue, TextEffectSnapshot,
    },
    vfs::workspace_base_path,
};

const SAVE_ROOT: &str = "saves";
const SAVE_EXTENSION: &str = "sav";
const USER_SETTINGS_PATH: &str = "config/hiraku.data.hks";

#[derive(Clone, Debug, Resource, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default = "default_volume")]
    pub bgm_volume: f32,
    #[serde(default = "default_volume")]
    pub voice_volume: f32,
    #[serde(default = "default_volume")]
    pub sfx_volume: f32,
}

#[derive(Debug, Deserialize)]
struct UserSettingsFile {
    #[serde(rename = "bgmVolume")]
    #[serde(default = "default_volume_f64")]
    bgm_volume: f64,
    #[serde(rename = "voiceVolume")]
    #[serde(default = "default_volume_f64")]
    voice_volume: f64,
    #[serde(rename = "sfxVolume")]
    #[serde(default = "default_volume_f64")]
    sfx_volume: f64,
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
    #[error("failed to load HKS data: {0}")]
    HksData(String),
    #[error("failed to decode save protobuf: {0}")]
    ProstDecode(#[from] prost::DecodeError),
    #[error("invalid save data: {0}")]
    InvalidSave(String),
    #[error("slot name can only contain letters, digits, '-' or '_'")]
    InvalidSlot,
}

pub fn save_root_path() -> PathBuf {
    workspace_base_path().join(SAVE_ROOT)
}

pub fn read_user_settings() -> Result<UserSettings, StorageError> {
    #[cfg(target_arch = "wasm32")]
    return Ok(UserSettings::default());

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = workspace_base_path().join(USER_SETTINGS_PATH);
        match fs::read_to_string(path) {
            Ok(payload) => {
                let data = evaluate_hks_map(USER_SETTINGS_PATH, &payload)
                    .map_err(|error| StorageError::HksData(error.to_string()))?;
                let settings =
                    serde_json::from_value::<UserSettingsFile>(serde_json::Value::Object(data))
                        .map_err(|error| StorageError::HksData(error.to_string()))?;
                Ok(UserSettings {
                    bgm_volume: settings.bgm_volume as f32,
                    voice_volume: settings.voice_volume as f32,
                    sfx_volume: settings.sfx_volume as f32,
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(UserSettings::default()),
            Err(err) => Err(StorageError::Io(err)),
        }
    }
}

pub fn write_user_settings(settings: &UserSettings) -> Result<(), StorageError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = settings;
        return Ok(());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = workspace_base_path().join(USER_SETTINGS_PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let payload = format!(
            ".{{\n    bgmVolume: {:?},\n    voiceVolume: {:?},\n    sfxVolume: {:?},\n}}\n",
            settings.bgm_volume, settings.voice_volume, settings.sfx_volume
        );
        fs::write(path, payload)?;
        Ok(())
    }
}

pub fn load_save_data(slot: &str) -> Result<SaveGameData, StorageError> {
    load_save_data_from_root(&save_root_path(), slot)
}

pub fn load_save_data_from_root(root: &Path, slot: &str) -> Result<SaveGameData, StorageError> {
    let path = slot_path_in(root, slot)?;
    let payload = fs::read(path)?;
    decode_save_data(&payload)
}

pub fn write_save_data_to_root(
    root: &Path,
    slot: &str,
    data: &SaveGameData,
) -> Result<(), StorageError> {
    fs::create_dir_all(root)?;
    let path = slot_path_in(root, slot)?;
    fs::write(path, encode_save_data(data))?;
    Ok(())
}

fn encode_save_data(data: &SaveGameData) -> Vec<u8> {
    proto::SaveGameData::from(data).encode_to_vec()
}

fn decode_save_data(payload: &[u8]) -> Result<SaveGameData, StorageError> {
    proto::SaveGameData::decode(payload)?.try_into()
}

pub fn list_save_slots() -> Result<Vec<SaveSlotSummary>, StorageError> {
    #[cfg(target_arch = "wasm32")]
    return Ok(Vec::new());

    #[cfg(not(target_arch = "wasm32"))]
    {
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
            if path.extension().and_then(|value| value.to_str()) != Some(SAVE_EXTENSION) {
                continue;
            }

            let Some(slot) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };

            let Ok(payload) = fs::read(&path) else {
                continue;
            };
            let Ok(data) = decode_save_data(&payload) else {
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
}

pub fn slot_path_in(root: &Path, slot: &str) -> Result<PathBuf, StorageError> {
    let slot = sanitize_slot_name(slot)?;
    Ok(root.join(format!("{slot}.{SAVE_EXTENSION}")))
}

impl From<&SaveGameData> for proto::SaveGameData {
    fn from(data: &SaveGameData) -> Self {
        Self {
            version: data.version,
            resume_script: data.resume_script.clone(),
            random_seed: data.random_seed,
            time_seed: data.time_seed,
            rng_state: data.rng_state.as_ref().map(Into::into),
            checkpoint: data.checkpoint.as_ref().map(Into::into),
            script_stack: data.script_stack.clone(),
            globals: stored_entries_from_map(&data.globals),
            scope: stored_entries_from_map(&data.scope),
            input_log: data.input_log.iter().map(Into::into).collect(),
            scene: Some((&data.scene).into()),
        }
    }
}

impl TryFrom<proto::SaveGameData> for SaveGameData {
    type Error = StorageError;

    fn try_from(data: proto::SaveGameData) -> Result<Self, Self::Error> {
        Ok(Self {
            version: data.version,
            resume_script: data.resume_script,
            random_seed: data.random_seed,
            rng_state: data.rng_state.map(Into::into),
            time_seed: data.time_seed,
            checkpoint: data.checkpoint.map(TryInto::try_into).transpose()?,
            script_stack: data.script_stack,
            globals: stored_map_from_entries(data.globals)?,
            scope: stored_map_from_entries(data.scope)?,
            input_log: data
                .input_log
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            scene: data
                .scene
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
        })
    }
}

impl From<&crate::state::RngState> for proto::RngState {
    fn from(state: &crate::state::RngState) -> Self {
        Self {
            state: state.state,
            stream: state.stream,
        }
    }
}

impl From<proto::RngState> for crate::state::RngState {
    fn from(state: proto::RngState) -> Self {
        Self {
            state: state.state,
            stream: state.stream,
        }
    }
}

impl From<&SaveCheckpoint> for proto::SaveCheckpoint {
    fn from(checkpoint: &SaveCheckpoint) -> Self {
        Self {
            script: checkpoint.script.clone(),
            ordinal: checkpoint.ordinal,
            kind: checkpoint.kind.clone(),
            label: checkpoint.label.clone(),
            position: Some((&checkpoint.position).into()),
        }
    }
}

impl TryFrom<proto::SaveCheckpoint> for SaveCheckpoint {
    type Error = StorageError;

    fn try_from(checkpoint: proto::SaveCheckpoint) -> Result<Self, Self::Error> {
        Ok(Self {
            script: checkpoint.script,
            ordinal: checkpoint.ordinal,
            kind: checkpoint.kind,
            label: checkpoint.label,
            position: checkpoint.position.map(Into::into).unwrap_or_default(),
        })
    }
}

impl From<&ScriptPosition> for proto::ScriptPosition {
    fn from(position: &ScriptPosition) -> Self {
        Self {
            line: position.line.map(|value| value as u64),
            column: position.column.map(|value| value as u64),
        }
    }
}

impl From<proto::ScriptPosition> for ScriptPosition {
    fn from(position: proto::ScriptPosition) -> Self {
        Self {
            line: position.line.map(|value| value as usize),
            column: position.column.map(|value| value as usize),
        }
    }
}

impl From<&SavedInput> for proto::SavedInput {
    fn from(input: &SavedInput) -> Self {
        Self {
            checkpoint: Some((&input.checkpoint).into()),
            value: Some((&input.value).into()),
        }
    }
}

impl TryFrom<proto::SavedInput> for SavedInput {
    type Error = StorageError;

    fn try_from(input: proto::SavedInput) -> Result<Self, Self::Error> {
        Ok(Self {
            checkpoint: input
                .checkpoint
                .ok_or_else(|| {
                    StorageError::InvalidSave("saved input missing checkpoint".to_string())
                })?
                .try_into()?,
            value: input
                .value
                .ok_or_else(|| StorageError::InvalidSave("saved input missing value".to_string()))?
                .try_into()?,
        })
    }
}

impl From<&StoredValue> for proto::StoredValue {
    fn from(value: &StoredValue) -> Self {
        use proto::stored_value::Kind;

        let kind = match value {
            StoredValue::Bool(value) => Kind::Bool(*value),
            StoredValue::Int(value) => Kind::Int(*value),
            StoredValue::Float(value) => Kind::Float(*value),
            StoredValue::String(value) => Kind::String(value.clone()),
            StoredValue::Array(values) => Kind::Array(proto::StoredArray {
                values: values.iter().map(Into::into).collect(),
            }),
            StoredValue::Map(values) => Kind::Map(proto::StoredMap {
                entries: stored_entries_from_map(values),
            }),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<proto::StoredValue> for StoredValue {
    type Error = StorageError;

    fn try_from(value: proto::StoredValue) -> Result<Self, Self::Error> {
        use proto::stored_value::Kind;

        match value
            .kind
            .ok_or_else(|| StorageError::InvalidSave("stored value missing kind".to_string()))?
        {
            Kind::Bool(value) => Ok(StoredValue::Bool(value)),
            Kind::Int(value) => Ok(StoredValue::Int(value)),
            Kind::Float(value) => Ok(StoredValue::Float(value)),
            Kind::String(value) => Ok(StoredValue::String(value)),
            Kind::Array(values) => values
                .values
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()
                .map(StoredValue::Array),
            Kind::Map(values) => stored_map_from_entries(values.entries).map(StoredValue::Map),
        }
    }
}

impl From<&SceneSnapshot> for proto::SceneSnapshot {
    fn from(scene: &SceneSnapshot) -> Self {
        Self {
            background: scene.background.as_ref().map(Into::into),
            sprites: scene.sprites.iter().map(Into::into).collect(),
            character_positions: scene
                .character_positions
                .iter()
                .map(|(actor_id, position)| proto::CharacterPosition {
                    actor_id: actor_id.clone(),
                    x: position[0],
                    y: position[1],
                })
                .collect(),
            overlay_alpha: scene.overlay_alpha,
            bgm: scene.bgm.as_ref().map(Into::into),
            dialogue: scene.dialogue.as_ref().map(Into::into),
            text_effect: Some((&scene.text_effect).into()),
        }
    }
}

impl TryFrom<proto::SceneSnapshot> for SceneSnapshot {
    type Error = StorageError;

    fn try_from(scene: proto::SceneSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            background: scene.background.map(Into::into),
            sprites: scene.sprites.into_iter().map(Into::into).collect(),
            character_positions: scene
                .character_positions
                .into_iter()
                .map(|position| (position.actor_id, [position.x, position.y]))
                .collect(),
            overlay_alpha: scene.overlay_alpha,
            bgm: scene.bgm.map(Into::into),
            dialogue: scene.dialogue.map(Into::into),
            text_effect: scene.text_effect.map(Into::into).unwrap_or_default(),
        })
    }
}

impl From<&ImageLayerSnapshot> for proto::ImageLayerSnapshot {
    fn from(snapshot: &ImageLayerSnapshot) -> Self {
        Self {
            path: snapshot.path.clone(),
        }
    }
}

impl From<proto::ImageLayerSnapshot> for ImageLayerSnapshot {
    fn from(snapshot: proto::ImageLayerSnapshot) -> Self {
        Self {
            path: snapshot.path,
        }
    }
}

impl From<&SpriteSnapshot> for proto::SpriteSnapshot {
    fn from(snapshot: &SpriteSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            path: snapshot.path.clone(),
            x: snapshot.x,
            y: snapshot.y,
            layer: snapshot.layer,
            scale: snapshot.scale,
            alpha: snapshot.alpha,
            rect: snapshot.rect.map(Vec::from).unwrap_or_default(),
        }
    }
}

impl From<proto::SpriteSnapshot> for SpriteSnapshot {
    fn from(snapshot: proto::SpriteSnapshot) -> Self {
        Self {
            id: snapshot.id,
            path: snapshot.path,
            x: snapshot.x,
            y: snapshot.y,
            layer: snapshot.layer,
            scale: snapshot.scale,
            alpha: snapshot.alpha,
            rect: (snapshot.rect.len() == 4).then(|| {
                [
                    snapshot.rect[0],
                    snapshot.rect[1],
                    snapshot.rect[2],
                    snapshot.rect[3],
                ]
            }),
        }
    }
}

impl From<&AudioSnapshot> for proto::AudioSnapshot {
    fn from(snapshot: &AudioSnapshot) -> Self {
        Self {
            path: snapshot.path.clone(),
            volume: snapshot.volume,
        }
    }
}

impl From<proto::AudioSnapshot> for AudioSnapshot {
    fn from(snapshot: proto::AudioSnapshot) -> Self {
        Self {
            path: snapshot.path,
            volume: snapshot.volume,
        }
    }
}

impl From<&DialogueSnapshot> for proto::DialogueSnapshot {
    fn from(snapshot: &DialogueSnapshot) -> Self {
        Self {
            speaker: snapshot.speaker.clone(),
            text: snapshot.text.clone(),
        }
    }
}

impl From<proto::DialogueSnapshot> for DialogueSnapshot {
    fn from(snapshot: proto::DialogueSnapshot) -> Self {
        Self {
            speaker: snapshot.speaker,
            text: snapshot.text,
        }
    }
}

impl From<&TextEffectSnapshot> for proto::TextEffectSnapshot {
    fn from(snapshot: &TextEffectSnapshot) -> Self {
        Self {
            mode: snapshot.mode.clone(),
            cps: snapshot.cps,
            fade_seconds: snapshot.fade_seconds,
        }
    }
}

impl From<proto::TextEffectSnapshot> for TextEffectSnapshot {
    fn from(snapshot: proto::TextEffectSnapshot) -> Self {
        Self {
            mode: snapshot.mode,
            cps: snapshot.cps,
            fade_seconds: snapshot.fade_seconds,
        }
    }
}

fn stored_entries_from_map(values: &BTreeMap<String, StoredValue>) -> Vec<proto::StoredEntry> {
    values
        .iter()
        .map(|(key, value)| proto::StoredEntry {
            key: key.clone(),
            value: Some(value.into()),
        })
        .collect()
}

fn stored_map_from_entries(
    entries: Vec<proto::StoredEntry>,
) -> Result<BTreeMap<String, StoredValue>, StorageError> {
    let mut values = BTreeMap::new();
    for entry in entries {
        let value = entry
            .value
            .ok_or_else(|| {
                StorageError::InvalidSave(format!("stored entry `{}` missing value", entry.key))
            })?
            .try_into()?;
        values.insert(entry.key, value);
    }
    Ok(values)
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

fn default_volume_f64() -> f64 {
    1.0
}
