use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
}

fn default_save_version() -> u32 {
    1
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiStylePatch {
    pub dialogue_bg: Option<[f32; 4]>,
    pub dialogue_border: Option<[f32; 4]>,
    pub dialogue_left: Option<f32>,
    pub dialogue_right: Option<f32>,
    pub dialogue_bottom: Option<f32>,
    pub dialogue_min_height: Option<f32>,
    pub dialogue_padding_x: Option<f32>,
    pub dialogue_padding_y: Option<f32>,
    pub dialogue_radius: Option<f32>,
    pub speaker_size: Option<f32>,
    pub line_size: Option<f32>,
    pub hint_size: Option<f32>,
    pub hint_visible: Option<bool>,
    pub speaker_color: Option<[f32; 4]>,
    pub line_color: Option<[f32; 4]>,
    pub hint_color: Option<[f32; 4]>,
    pub choice_panel_bg: Option<[f32; 4]>,
    pub choice_bottom: Option<f32>,
    pub choice_panel_width: Option<f32>,
    pub choice_padding: Option<f32>,
    pub choice_gap: Option<f32>,
    pub choice_prompt_size: Option<f32>,
    pub choice_button_size: Option<f32>,
    pub choice_center_text: Option<bool>,
    pub choice_show_indices: Option<bool>,
    pub choice_prompt_color: Option<[f32; 4]>,
    pub choice_button_bg: Option<[f32; 4]>,
    pub choice_button_hovered: Option<[f32; 4]>,
    pub choice_button_pressed: Option<[f32; 4]>,
    pub choice_button_border: Option<[f32; 4]>,
    pub choice_text_color: Option<[f32; 4]>,
    pub quick_menu_bottom: Option<f32>,
    pub quick_menu_gap: Option<f32>,
    pub quick_button_size: Option<f32>,
    pub quick_menu_bg: Option<[f32; 4]>,
    pub quick_button_bg: Option<[f32; 4]>,
    pub quick_button_hovered: Option<[f32; 4]>,
    pub quick_button_pressed: Option<[f32; 4]>,
    pub quick_button_border: Option<[f32; 4]>,
    pub quick_text_color: Option<[f32; 4]>,
}

#[derive(Resource, Clone, Default)]
pub struct SceneSharedState(pub Arc<Mutex<SceneSnapshot>>);
