use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::script::StoryRuntimeSnapshot;

pub const CURRENT_SAVE_VERSION: u32 = 11;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptCallFrameSnapshot {
    pub script: String,
    pub snapshot: StoryRuntimeSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoredValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<StoredValue>),
    Map(BTreeMap<String, StoredValue>),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScriptPosition {
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SaveCheckpoint {
    pub script: String,
    pub ordinal: u64,
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub position: ScriptPosition,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedInput {
    pub checkpoint: SaveCheckpoint,
    pub value: StoredValue,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RngState {
    pub state: u64,
    pub stream: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub text: String,
    pub value: StoredValue,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImageLayerSnapshot {
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpriteSnapshot {
    pub id: String,
    pub path: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub layer: f32,
    pub scale: f32,
    pub alpha: f32,
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioSnapshot {
    pub path: String,
    pub volume: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DialogueSnapshot {
    pub speaker: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TextEffectSnapshot {
    pub mode: String,
    pub cps: f32,
    pub fade_seconds: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraSnapshot {
    pub blur: f32,
    pub zoom: f32,
    pub offset: [f32; 3],
    pub rotation: [f32; 3],
    pub projection: String,
    pub scope: String,
}

impl Default for CameraSnapshot {
    fn default() -> Self {
        Self {
            blur: 0.0,
            zoom: 1.0,
            offset: [0.0; 3],
            rotation: [0.0; 3],
            projection: "orthographic".to_string(),
            scope: "world".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SceneSnapshot {
    pub background: Option<ImageLayerSnapshot>,
    #[serde(default)]
    pub sprites: Vec<SpriteSnapshot>,
    #[serde(default)]
    pub character_positions: BTreeMap<String, [f32; 2]>,
    pub overlay_alpha: f32,
    pub bgm: Option<AudioSnapshot>,
    pub dialogue: Option<DialogueSnapshot>,
    #[serde(default)]
    pub text_effect: TextEffectSnapshot,
    #[serde(default)]
    pub camera: CameraSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveGameData {
    #[serde(default = "default_save_version")]
    pub version: u32,
    pub resume_script: String,
    #[serde(default)]
    pub random_seed: u64,
    #[serde(default)]
    pub rng_state: Option<RngState>,
    #[serde(default)]
    pub time_seed: i64,
    #[serde(default)]
    pub checkpoint: Option<SaveCheckpoint>,
    #[serde(default)]
    pub script_stack: Vec<String>,
    #[serde(default)]
    pub globals: BTreeMap<String, StoredValue>,
    #[serde(default)]
    pub scope: BTreeMap<String, StoredValue>,
    #[serde(default)]
    pub input_log: Vec<SavedInput>,
    #[serde(default)]
    pub scene: SceneSnapshot,
    #[serde(default)]
    pub vm_snapshot: Option<StoryRuntimeSnapshot>,
    #[serde(default)]
    pub script_call_stack: Vec<ScriptCallFrameSnapshot>,
    #[serde(default)]
    pub pending_ui_screen: Option<String>,
    #[serde(default)]
    pub pending_ui_arguments: Vec<StoredValue>,
    /// Runtime UI roles selected by scripts (for example `dialogue` or `title`).
    /// This is state, not part of the bytecode/native ABI manifest.
    #[serde(default)]
    pub ui_registry: BTreeMap<String, String>,
    #[serde(default)]
    pub mounted_ui_overlays: BTreeMap<String, String>,
}

fn default_save_version() -> u32 {
    CURRENT_SAVE_VERSION
}

impl Default for SaveGameData {
    fn default() -> Self {
        Self {
            version: CURRENT_SAVE_VERSION,
            resume_script: String::new(),
            random_seed: 0,
            rng_state: None,
            time_seed: 0,
            checkpoint: None,
            script_stack: Vec::new(),
            globals: BTreeMap::new(),
            scope: BTreeMap::new(),
            input_log: Vec::new(),
            scene: SceneSnapshot::default(),
            vm_snapshot: None,
            script_call_stack: Vec::new(),
            pending_ui_screen: None,
            pending_ui_arguments: Vec::new(),
            ui_registry: BTreeMap::new(),
            mounted_ui_overlays: BTreeMap::new(),
        }
    }
}

#[derive(Resource, Clone, Default)]
pub struct SceneSharedState(pub SceneSnapshot);
