use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use bevy::{asset::io::file::FileAssetReader, prelude::Resource};
use prost::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::state::{
    AudioSnapshot, DialogueSnapshot, ImageLayerSnapshot, SaveCheckpoint, SaveGameData, SavedInput,
    SceneSnapshot, ScriptPosition, SpriteSnapshot, StoredValue, TextEffectSnapshot,
};

const SAVE_ROOT: &str = "saves";
const SAVE_EXTENSION: &str = "sav";
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
    #[error("failed to decode save protobuf: {0}")]
    ProstDecode(#[from] prost::DecodeError),
    #[error("invalid save data: {0}")]
    InvalidSave(String),
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
    proto::ProtoSaveGameData::from(data).encode_to_vec()
}

fn decode_save_data(payload: &[u8]) -> Result<SaveGameData, StorageError> {
    proto::ProtoSaveGameData::decode(payload)?.try_into()
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

pub fn slot_path_in(root: &Path, slot: &str) -> Result<PathBuf, StorageError> {
    let slot = sanitize_slot_name(slot)?;
    Ok(root.join(format!("{slot}.{SAVE_EXTENSION}")))
}

mod proto {
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoSaveGameData {
        #[prost(uint32, tag = "1")]
        pub version: u32,
        #[prost(string, tag = "2")]
        pub resume_script: String,
        #[prost(uint64, tag = "3")]
        pub random_seed: u64,
        #[prost(int64, tag = "4")]
        pub time_seed: i64,
        #[prost(message, optional, tag = "5")]
        pub checkpoint: Option<ProtoSaveCheckpoint>,
        #[prost(string, repeated, tag = "6")]
        pub script_stack: Vec<String>,
        #[prost(message, repeated, tag = "7")]
        pub globals: Vec<ProtoStoredEntry>,
        #[prost(message, repeated, tag = "8")]
        pub scope: Vec<ProtoStoredEntry>,
        #[prost(message, repeated, tag = "9")]
        pub input_log: Vec<ProtoSavedInput>,
        #[prost(message, optional, tag = "10")]
        pub scene: Option<ProtoSceneSnapshot>,
        #[prost(message, optional, tag = "11")]
        pub rng_state: Option<ProtoRngState>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoRngState {
        #[prost(uint64, tag = "1")]
        pub state: u64,
        #[prost(uint64, tag = "2")]
        pub stream: u64,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoSaveCheckpoint {
        #[prost(string, tag = "1")]
        pub script: String,
        #[prost(uint64, tag = "2")]
        pub ordinal: u64,
        #[prost(string, tag = "3")]
        pub kind: String,
        #[prost(string, optional, tag = "4")]
        pub label: Option<String>,
        #[prost(message, optional, tag = "5")]
        pub position: Option<ProtoScriptPosition>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoScriptPosition {
        #[prost(uint64, optional, tag = "1")]
        pub line: Option<u64>,
        #[prost(uint64, optional, tag = "2")]
        pub column: Option<u64>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoSavedInput {
        #[prost(message, optional, tag = "1")]
        pub checkpoint: Option<ProtoSaveCheckpoint>,
        #[prost(message, optional, tag = "2")]
        pub value: Option<ProtoStoredValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoStoredEntry {
        #[prost(string, tag = "1")]
        pub key: String,
        #[prost(message, optional, tag = "2")]
        pub value: Option<ProtoStoredValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoStoredArray {
        #[prost(message, repeated, tag = "1")]
        pub values: Vec<ProtoStoredValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoStoredMap {
        #[prost(message, repeated, tag = "1")]
        pub entries: Vec<ProtoStoredEntry>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoStoredValue {
        #[prost(oneof = "proto_stored_value::Kind", tags = "1, 2, 3, 4, 5, 6")]
        pub kind: Option<proto_stored_value::Kind>,
    }

    pub mod proto_stored_value {
        use prost::Oneof;

        use super::{ProtoStoredArray, ProtoStoredMap};

        #[derive(Clone, PartialEq, Oneof)]
        pub enum Kind {
            #[prost(bool, tag = "1")]
            Bool(bool),
            #[prost(int64, tag = "2")]
            Int(i64),
            #[prost(double, tag = "3")]
            Float(f64),
            #[prost(string, tag = "4")]
            String(String),
            #[prost(message, tag = "5")]
            Array(ProtoStoredArray),
            #[prost(message, tag = "6")]
            Map(ProtoStoredMap),
        }
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoSceneSnapshot {
        #[prost(message, optional, tag = "1")]
        pub background: Option<ProtoImageLayerSnapshot>,
        #[prost(message, repeated, tag = "2")]
        pub sprites: Vec<ProtoSpriteSnapshot>,
        #[prost(message, repeated, tag = "3")]
        pub character_positions: Vec<ProtoCharacterPosition>,
        #[prost(float, tag = "4")]
        pub overlay_alpha: f32,
        #[prost(message, optional, tag = "5")]
        pub bgm: Option<ProtoAudioSnapshot>,
        #[prost(message, optional, tag = "6")]
        pub dialogue: Option<ProtoDialogueSnapshot>,
        #[prost(message, optional, tag = "7")]
        pub text_effect: Option<ProtoTextEffectSnapshot>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoImageLayerSnapshot {
        #[prost(string, tag = "1")]
        pub path: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoSpriteSnapshot {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub path: String,
        #[prost(float, tag = "3")]
        pub x: f32,
        #[prost(float, tag = "4")]
        pub y: f32,
        #[prost(float, tag = "5")]
        pub layer: f32,
        #[prost(float, tag = "6")]
        pub scale: f32,
        #[prost(float, tag = "7")]
        pub alpha: f32,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoCharacterPosition {
        #[prost(string, tag = "1")]
        pub actor_id: String,
        #[prost(float, tag = "2")]
        pub x: f32,
        #[prost(float, tag = "3")]
        pub y: f32,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoAudioSnapshot {
        #[prost(string, tag = "1")]
        pub path: String,
        #[prost(float, tag = "2")]
        pub volume: f32,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoDialogueSnapshot {
        #[prost(string, tag = "1")]
        pub speaker: String,
        #[prost(string, tag = "2")]
        pub text: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ProtoTextEffectSnapshot {
        #[prost(string, tag = "1")]
        pub mode: String,
        #[prost(float, tag = "2")]
        pub cps: f32,
        #[prost(float, tag = "3")]
        pub fade_seconds: f32,
    }
}

impl From<&SaveGameData> for proto::ProtoSaveGameData {
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

impl TryFrom<proto::ProtoSaveGameData> for SaveGameData {
    type Error = StorageError;

    fn try_from(data: proto::ProtoSaveGameData) -> Result<Self, Self::Error> {
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

impl From<&crate::state::RngState> for proto::ProtoRngState {
    fn from(state: &crate::state::RngState) -> Self {
        Self {
            state: state.state,
            stream: state.stream,
        }
    }
}

impl From<proto::ProtoRngState> for crate::state::RngState {
    fn from(state: proto::ProtoRngState) -> Self {
        Self {
            state: state.state,
            stream: state.stream,
        }
    }
}

impl From<&SaveCheckpoint> for proto::ProtoSaveCheckpoint {
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

impl TryFrom<proto::ProtoSaveCheckpoint> for SaveCheckpoint {
    type Error = StorageError;

    fn try_from(checkpoint: proto::ProtoSaveCheckpoint) -> Result<Self, Self::Error> {
        Ok(Self {
            script: checkpoint.script,
            ordinal: checkpoint.ordinal,
            kind: checkpoint.kind,
            label: checkpoint.label,
            position: checkpoint.position.map(Into::into).unwrap_or_default(),
        })
    }
}

impl From<&ScriptPosition> for proto::ProtoScriptPosition {
    fn from(position: &ScriptPosition) -> Self {
        Self {
            line: position.line.map(|value| value as u64),
            column: position.column.map(|value| value as u64),
        }
    }
}

impl From<proto::ProtoScriptPosition> for ScriptPosition {
    fn from(position: proto::ProtoScriptPosition) -> Self {
        Self {
            line: position.line.map(|value| value as usize),
            column: position.column.map(|value| value as usize),
        }
    }
}

impl From<&SavedInput> for proto::ProtoSavedInput {
    fn from(input: &SavedInput) -> Self {
        Self {
            checkpoint: Some((&input.checkpoint).into()),
            value: Some((&input.value).into()),
        }
    }
}

impl TryFrom<proto::ProtoSavedInput> for SavedInput {
    type Error = StorageError;

    fn try_from(input: proto::ProtoSavedInput) -> Result<Self, Self::Error> {
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

impl From<&StoredValue> for proto::ProtoStoredValue {
    fn from(value: &StoredValue) -> Self {
        use proto::proto_stored_value::Kind;

        let kind = match value {
            StoredValue::Bool(value) => Kind::Bool(*value),
            StoredValue::Int(value) => Kind::Int(*value),
            StoredValue::Float(value) => Kind::Float(*value),
            StoredValue::String(value) => Kind::String(value.clone()),
            StoredValue::Array(values) => Kind::Array(proto::ProtoStoredArray {
                values: values.iter().map(Into::into).collect(),
            }),
            StoredValue::Map(values) => Kind::Map(proto::ProtoStoredMap {
                entries: stored_entries_from_map(values),
            }),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<proto::ProtoStoredValue> for StoredValue {
    type Error = StorageError;

    fn try_from(value: proto::ProtoStoredValue) -> Result<Self, Self::Error> {
        use proto::proto_stored_value::Kind;

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

impl From<&SceneSnapshot> for proto::ProtoSceneSnapshot {
    fn from(scene: &SceneSnapshot) -> Self {
        Self {
            background: scene.background.as_ref().map(Into::into),
            sprites: scene.sprites.iter().map(Into::into).collect(),
            character_positions: scene
                .character_positions
                .iter()
                .map(|(actor_id, position)| proto::ProtoCharacterPosition {
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

impl TryFrom<proto::ProtoSceneSnapshot> for SceneSnapshot {
    type Error = StorageError;

    fn try_from(scene: proto::ProtoSceneSnapshot) -> Result<Self, Self::Error> {
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

impl From<&ImageLayerSnapshot> for proto::ProtoImageLayerSnapshot {
    fn from(snapshot: &ImageLayerSnapshot) -> Self {
        Self {
            path: snapshot.path.clone(),
        }
    }
}

impl From<proto::ProtoImageLayerSnapshot> for ImageLayerSnapshot {
    fn from(snapshot: proto::ProtoImageLayerSnapshot) -> Self {
        Self {
            path: snapshot.path,
        }
    }
}

impl From<&SpriteSnapshot> for proto::ProtoSpriteSnapshot {
    fn from(snapshot: &SpriteSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            path: snapshot.path.clone(),
            x: snapshot.x,
            y: snapshot.y,
            layer: snapshot.layer,
            scale: snapshot.scale,
            alpha: snapshot.alpha,
        }
    }
}

impl From<proto::ProtoSpriteSnapshot> for SpriteSnapshot {
    fn from(snapshot: proto::ProtoSpriteSnapshot) -> Self {
        Self {
            id: snapshot.id,
            path: snapshot.path,
            x: snapshot.x,
            y: snapshot.y,
            layer: snapshot.layer,
            scale: snapshot.scale,
            alpha: snapshot.alpha,
        }
    }
}

impl From<&AudioSnapshot> for proto::ProtoAudioSnapshot {
    fn from(snapshot: &AudioSnapshot) -> Self {
        Self {
            path: snapshot.path.clone(),
            volume: snapshot.volume,
        }
    }
}

impl From<proto::ProtoAudioSnapshot> for AudioSnapshot {
    fn from(snapshot: proto::ProtoAudioSnapshot) -> Self {
        Self {
            path: snapshot.path,
            volume: snapshot.volume,
        }
    }
}

impl From<&DialogueSnapshot> for proto::ProtoDialogueSnapshot {
    fn from(snapshot: &DialogueSnapshot) -> Self {
        Self {
            speaker: snapshot.speaker.clone(),
            text: snapshot.text.clone(),
        }
    }
}

impl From<proto::ProtoDialogueSnapshot> for DialogueSnapshot {
    fn from(snapshot: proto::ProtoDialogueSnapshot) -> Self {
        Self {
            speaker: snapshot.speaker,
            text: snapshot.text,
        }
    }
}

impl From<&TextEffectSnapshot> for proto::ProtoTextEffectSnapshot {
    fn from(snapshot: &TextEffectSnapshot) -> Self {
        Self {
            mode: snapshot.mode.clone(),
            cps: snapshot.cps,
            fade_seconds: snapshot.fade_seconds,
        }
    }
}

impl From<proto::ProtoTextEffectSnapshot> for TextEffectSnapshot {
    fn from(snapshot: proto::ProtoTextEffectSnapshot) -> Self {
        Self {
            mode: snapshot.mode,
            cps: snapshot.cps,
            fade_seconds: snapshot.fade_seconds,
        }
    }
}

fn stored_entries_from_map(values: &BTreeMap<String, StoredValue>) -> Vec<proto::ProtoStoredEntry> {
    values
        .iter()
        .map(|(key, value)| proto::ProtoStoredEntry {
            key: key.clone(),
            value: Some(value.into()),
        })
        .collect()
}

fn stored_map_from_entries(
    entries: Vec<proto::ProtoStoredEntry>,
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
