use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state::StoredValue;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenSpec {
    pub title: Option<String>,
    #[serde(default = "default_screen_panel")]
    pub panel: bool,
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub background_image: Option<String>,
    #[serde(default)]
    pub xalign: f32,
    #[serde(default)]
    pub yalign: f32,
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub overlay: Option<[f32; 4]>,
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
    Image(ScreenImageNode),
    Bar(BarNode),
    Row(ContainerNode),
    Column(ContainerNode),
    Spacer(SpacerNode),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScreenLayout {
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub height: Option<f32>,
    #[serde(default)]
    pub width_percent: Option<f32>,
    #[serde(default)]
    pub height_percent: Option<f32>,
    #[serde(default)]
    pub min_width: Option<f32>,
    #[serde(default)]
    pub left: Option<f32>,
    #[serde(default)]
    pub right: Option<f32>,
    #[serde(default)]
    pub top: Option<f32>,
    #[serde(default)]
    pub bottom: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNode {
    pub text: String,
    #[serde(default = "default_text_size")]
    pub size: f32,
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    #[serde(default)]
    pub align: Option<f32>,
    #[serde(default)]
    pub layout: ScreenLayout,
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
    pub hovered_background: Option<[f32; 4]>,
    #[serde(default)]
    pub pressed_background: Option<[f32; 4]>,
    #[serde(default)]
    pub align: Option<f32>,
    #[serde(default)]
    pub padding_x: Option<f32>,
    #[serde(default)]
    pub padding_y: Option<f32>,
    #[serde(default)]
    pub border_width: Option<f32>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub layout: ScreenLayout,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenImageNode {
    pub path: String,
    #[serde(default)]
    pub layout: ScreenLayout,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BarNode {
    pub value: f32,
    #[serde(default)]
    pub min: f32,
    #[serde(default = "default_bar_max")]
    pub max: f32,
    #[serde(default = "default_bar_width")]
    pub width: f32,
    #[serde(default = "default_bar_height")]
    pub height: f32,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    #[serde(default)]
    pub fill: Option<[f32; 4]>,
    #[serde(default)]
    pub border: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerNode {
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub padding: f32,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    #[serde(default)]
    pub justify: Option<String>,
    #[serde(default)]
    pub align_items: Option<String>,
    #[serde(default)]
    pub layout: ScreenLayout,
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
    pub pending_root: Option<PendingScreenRoot>,
    pub stale_roots: Vec<StaleScreenRoot>,
    pub waiting: Option<std::sync::mpsc::Sender<crate::script::ScriptResponse>>,
}

pub struct PendingScreenRoot {
    pub entity: Entity,
    pub previous: Option<Entity>,
    pub wait_images: Vec<Handle<Image>>,
    pub ready_frames_remaining: u8,
    pub done: std::sync::mpsc::Sender<crate::script::ScriptResponse>,
}

#[derive(Clone, Debug)]
pub struct StaleScreenRoot {
    pub entity: Entity,
    pub frames_remaining: u8,
    pub wait_images: Vec<Handle<Image>>,
}

#[derive(Component)]
pub struct ScreenUiRoot;

#[derive(Component)]
pub struct ScreenUiNode;

#[derive(Component, Clone)]
pub struct ScreenUiButton {
    pub root: Entity,
    pub value: StoredValue,
    pub normal_background: Color,
    pub hovered_background: Color,
    pub pressed_background: Color,
}

fn default_screen_panel() -> bool {
    true
}

fn default_text_size() -> f32 {
    26.0
}

fn default_button_size() -> f32 {
    28.0
}

fn default_bar_max() -> f32 {
    1.0
}

fn default_bar_width() -> f32 {
    320.0
}

fn default_bar_height() -> f32 {
    18.0
}

pub fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}
