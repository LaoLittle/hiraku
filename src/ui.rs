use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state::StoredValue;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenSpec {
    pub title: Option<String>,
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    #[serde(default)]
    pub children: Vec<ScreenNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScreenNode {
    Text(TextNode),
    Button(ButtonNode),
    Row(ContainerNode),
    Column(ContainerNode),
    Spacer(SpacerNode),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNode {
    pub text: String,
    #[serde(default = "default_text_size")]
    pub size: f32,
    #[serde(default)]
    pub color: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ButtonNode {
    pub text: String,
    pub value: StoredValue,
    #[serde(default = "default_button_size")]
    pub size: f32,
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    #[serde(default)]
    pub width: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerNode {
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub children: Vec<ScreenNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpacerNode {
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
}

#[derive(Resource, Default)]
pub struct ScreenUiState {
    pub active_root: Option<Entity>,
    pub waiting: Option<std::sync::mpsc::Sender<crate::script::ScriptResponse>>,
}

#[derive(Component)]
pub struct ScreenUiRoot;

#[derive(Component)]
pub struct ScreenUiNode;

#[derive(Component, Clone)]
pub struct ScreenUiButton {
    pub value: StoredValue,
}

fn default_text_size() -> f32 {
    26.0
}

fn default_button_size() -> f32 {
    28.0
}

pub fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}
