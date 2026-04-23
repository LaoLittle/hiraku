use std::{collections::BTreeMap, sync::{Arc, Mutex}};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoredValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
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
pub struct SceneSnapshot {
    pub background: Option<ImageLayerSnapshot>,
    #[serde(default)]
    pub sprites: Vec<SpriteSnapshot>,
    pub overlay_alpha: f32,
    pub bgm: Option<AudioSnapshot>,
    pub dialogue: Option<DialogueSnapshot>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SaveGameData {
    pub resume_script: String,
    #[serde(default)]
    pub script_stack: Vec<String>,
    #[serde(default)]
    pub globals: BTreeMap<String, StoredValue>,
    #[serde(default)]
    pub scene: SceneSnapshot,
}

#[derive(Clone, Debug, Default)]
pub struct UiStylePatch {
    pub dialogue_bg: Option<[f32; 4]>,
    pub dialogue_border: Option<[f32; 4]>,
    pub speaker_color: Option<[f32; 4]>,
    pub line_color: Option<[f32; 4]>,
    pub hint_color: Option<[f32; 4]>,
    pub choice_panel_bg: Option<[f32; 4]>,
    pub choice_prompt_color: Option<[f32; 4]>,
    pub choice_button_bg: Option<[f32; 4]>,
    pub choice_button_hovered: Option<[f32; 4]>,
    pub choice_button_pressed: Option<[f32; 4]>,
    pub choice_button_border: Option<[f32; 4]>,
    pub choice_text_color: Option<[f32; 4]>,
}

#[derive(Resource, Clone, Default)]
pub struct SceneSharedState(pub Arc<Mutex<SceneSnapshot>>);
