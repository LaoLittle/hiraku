use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state::StoredValue;

/// A modal screen described by Rhai data.
///
/// `ScreenSpec` is the engine-side form of the map passed to `screen(#{ ... })`.
/// It intentionally mirrors common Ren'Py screen-language concepts: a full-screen
/// root, an optional default panel, background imagery, and a list of child nodes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenSpec {
    /// Optional title rendered by the default panel mode.
    pub title: Option<String>,
    /// When `true`, wraps children in the default centered panel.
    ///
    /// Set this to `false` for Ren'Py-style full-screen layouts where each child
    /// controls its own position with `left`, `right`, `top`, or `bottom`.
    #[serde(default = "default_screen_panel")]
    pub panel: bool,
    /// Width of the default panel, in logical pixels.
    #[serde(default)]
    pub width: Option<f32>,
    /// Full-screen catalog texture drawn behind all screen children.
    #[serde(default)]
    pub background_texture: Option<ScreenTexture>,
    /// Horizontal alignment for the default panel: `0.0` left, `0.5` center, `1.0` right.
    #[serde(default)]
    pub xalign: f32,
    /// Vertical alignment for the default panel: `0.0` top, `0.5` center, `1.0` bottom.
    #[serde(default)]
    pub yalign: f32,
    /// Padding applied to the default panel, in logical pixels.
    #[serde(default)]
    pub padding: f32,
    /// Gap between children in the default panel, in logical pixels.
    #[serde(default)]
    pub gap: f32,
    /// Full-screen overlay color in `[r, g, b, a]` form.
    #[serde(default)]
    pub overlay: Option<[f32; 4]>,
    /// Default panel background color in `[r, g, b, a]` form.
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    /// Default panel border color in `[r, g, b, a]` form.
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    /// Child nodes rendered inside the screen root or default panel.
    #[serde(default)]
    pub children: Vec<ScreenNode>,
}

/// A single displayable in a Rhai screen tree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ScreenNode {
    /// Static UI text.
    Text(TextNode),
    /// A clickable text button that returns `value` from `screen(...)`.
    Button(ButtonNode),
    /// A UI image resolved from the texture catalog.
    Image(ScreenImageNode),
    /// A clickable texture with optional hover artwork.
    ImageButton(ScreenImageButtonNode),
    /// A non-interactive horizontal progress bar.
    Bar(BarNode),
    /// A horizontal flex container.
    Row(ContainerNode),
    /// A vertical flex container.
    Column(ContainerNode),
    /// Empty fixed-size space.
    Spacer(SpacerNode),
}

/// Shared layout options supported by most screen nodes.
///
/// Pixel fields and percent fields intentionally coexist. If both are set, the
/// percent value wins. Position fields switch the node to absolute positioning.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScreenLayout {
    /// Fixed width in logical pixels.
    #[serde(default)]
    pub width: Option<f32>,
    /// Fixed height in logical pixels.
    #[serde(default)]
    pub height: Option<f32>,
    /// Width as a percent of the parent.
    #[serde(default)]
    pub width_percent: Option<f32>,
    /// Height as a percent of the parent.
    #[serde(default)]
    pub height_percent: Option<f32>,
    /// Minimum width in logical pixels.
    #[serde(default)]
    pub min_width: Option<f32>,
    /// Absolute left inset in logical pixels.
    #[serde(default)]
    pub left: Option<f32>,
    /// Absolute left inset as a percent of the parent width.
    #[serde(default)]
    pub left_percent: Option<f32>,
    /// Absolute right inset in logical pixels.
    #[serde(default)]
    pub right: Option<f32>,
    /// Absolute right inset as a percent of the parent width.
    #[serde(default)]
    pub right_percent: Option<f32>,
    /// Absolute top inset in logical pixels.
    #[serde(default)]
    pub top: Option<f32>,
    /// Absolute top inset as a percent of the parent height.
    #[serde(default)]
    pub top_percent: Option<f32>,
    /// Absolute bottom inset in logical pixels.
    #[serde(default)]
    pub bottom: Option<f32>,
    /// Absolute bottom inset as a percent of the parent height.
    #[serde(default)]
    pub bottom_percent: Option<f32>,
}

/// Static text in a Rhai screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNode {
    /// Text content.
    pub text: String,
    /// Font size in logical pixels.
    #[serde(default = "default_text_size")]
    pub size: f32,
    /// Text color in `[r, g, b, a]` form.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// Text alignment: `0.0` left, `0.5` center, `1.0` right.
    #[serde(default)]
    pub align: Option<f32>,
    /// Size and absolute positioning.
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// A clickable text button in a Rhai screen.
///
/// The button is intentionally text-first because that is the common Ren'Py
/// `textbutton` case. Image-backed buttons can be added later without changing
/// the existing ergonomic map shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ButtonNode {
    /// Label shown inside the button.
    pub text: String,
    /// Value returned from `screen(...)` when the button is pressed.
    ///
    /// Modal buttons normally use `value`. Non-modal overlays often use
    /// `action` instead.
    #[serde(default)]
    pub value: Option<StoredValue>,
    /// Built-in action for non-modal overlay buttons.
    ///
    /// Current actions are `quick_save`, `quick_load`, `menu`, `return`, and
    /// `main_menu`. This keeps common system UI script-defined while reusing
    /// engine services such as save/load.
    #[serde(default)]
    pub action: Option<String>,
    /// Whether the button can be pressed.
    ///
    /// Disabled buttons remain visible and use insensitive colors, matching
    /// Ren'Py's `insensitive` state.
    #[serde(default = "default_button_enabled")]
    pub enabled: bool,
    /// Label font size in logical pixels.
    #[serde(default = "default_button_size")]
    pub size: f32,
    /// Idle label color.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// Hover label color.
    #[serde(default)]
    pub hovered_color: Option<[f32; 4]>,
    /// Pressed label color.
    #[serde(default)]
    pub pressed_color: Option<[f32; 4]>,
    /// Disabled label color.
    #[serde(default)]
    pub insensitive_color: Option<[f32; 4]>,
    /// Idle background color.
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    /// Border color.
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    /// Hover background color.
    #[serde(default)]
    pub hovered_background: Option<[f32; 4]>,
    /// Pressed background color.
    #[serde(default)]
    pub pressed_background: Option<[f32; 4]>,
    /// Label alignment inside the button: `0.0` left, `0.5` center, `1.0` right.
    #[serde(default)]
    pub align: Option<f32>,
    /// Horizontal padding in logical pixels.
    #[serde(default)]
    pub padding_x: Option<f32>,
    /// Vertical padding in logical pixels.
    #[serde(default)]
    pub padding_y: Option<f32>,
    /// Border width in logical pixels.
    #[serde(default)]
    pub border_width: Option<f32>,
    /// Corner radius in logical pixels.
    #[serde(default)]
    pub radius: Option<f32>,
    /// Size and absolute positioning.
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// An image displayable in a Rhai screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenImageNode {
    pub texture: ScreenTexture,
    /// Size and absolute positioning.
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// A texture path and optional source rectangle resolved during script evaluation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenTexture {
    pub path: String,
    /// Source rectangle `[left, top, width, height]` in texture pixels.
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
}

/// A catalog-backed image button with optional hover artwork.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenImageButtonNode {
    pub texture: ScreenTexture,
    #[serde(default)]
    pub hovered_texture: Option<ScreenTexture>,
    /// Size and absolute positioning while the pointer hovers the button.
    #[serde(default)]
    pub hovered_layout: Option<ScreenLayout>,
    /// Value returned from `screen(...)` when the button is pressed.
    pub value: StoredValue,
    /// Whether the button can be pressed.
    #[serde(default = "default_button_enabled")]
    pub enabled: bool,
    /// Whether a disabled button still displays its hover artwork.
    #[serde(default)]
    pub hovered_when_disabled: bool,
    /// Size and absolute positioning.
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// A non-interactive horizontal progress bar.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BarNode {
    /// Current value.
    pub value: f32,
    /// Minimum value.
    #[serde(default)]
    pub min: f32,
    /// Maximum value.
    #[serde(default = "default_bar_max")]
    pub max: f32,
    /// Bar width in logical pixels.
    #[serde(default = "default_bar_width")]
    pub width: f32,
    /// Bar height in logical pixels.
    #[serde(default = "default_bar_height")]
    pub height: f32,
    /// Track background color.
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    /// Filled portion color.
    #[serde(default)]
    pub fill: Option<[f32; 4]>,
    /// Border color.
    #[serde(default)]
    pub border: Option<[f32; 4]>,
}

/// A flex container used by `vbox`, `hbox`, and `frame` Rhai nodes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerNode {
    /// Gap between children in logical pixels.
    #[serde(default)]
    pub gap: f32,
    /// Uniform padding in logical pixels.
    #[serde(default)]
    pub padding: f32,
    /// Container background color.
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    /// Container border color.
    #[serde(default)]
    pub border: Option<[f32; 4]>,
    /// Main-axis distribution: `start`, `center`, `end`, `between`, `around`, or `evenly`.
    #[serde(default)]
    pub justify: Option<String>,
    /// Cross-axis alignment: `start`, `center`, `end`, or `stretch`.
    #[serde(default)]
    pub align_items: Option<String>,
    /// Size and absolute positioning.
    #[serde(default)]
    pub layout: ScreenLayout,
    /// Child nodes.
    #[serde(default)]
    pub children: Vec<ScreenNode>,
}

/// Fixed empty space inside a container.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpacerNode {
    /// Width in logical pixels.
    #[serde(default)]
    pub width: f32,
    /// Height in logical pixels.
    #[serde(default)]
    pub height: f32,
}

/// Runtime state for modal Rhai screens.
///
/// The renderer keeps old and pending roots alive during transitions so a screen
/// replacement never exposes an empty frame while images are loading.
#[derive(Resource, Default)]
pub struct ScreenUiState {
    /// Currently interactive screen root.
    pub active_root: Option<Entity>,
    /// Newly spawned root waiting for image readiness and warm-up frames.
    pub pending_root: Option<PendingScreenRoot>,
    /// Old roots kept under the active screen for flicker-free replacement.
    pub stale_roots: Vec<StaleScreenRoot>,
    /// Response channel for the script currently blocked in `screen(...)`.
    pub waiting: Option<std::sync::mpsc::Sender<crate::script::ScriptResponse>>,
}

/// Active non-modal overlays spawned by `show_overlay(...)`.
#[derive(Resource, Default)]
pub struct OverlayUiState {
    /// Overlay roots keyed by script-provided name.
    pub roots: HashMap<String, Entity>,
}

/// A screen root that has been spawned but is not interactive yet.
pub struct PendingScreenRoot {
    /// Pending UI root entity.
    pub entity: Entity,
    /// Previous root kept visible until this pending root is ready.
    pub previous: Option<Entity>,
    /// Images that must be loaded before this screen becomes active.
    pub wait_images: Vec<Handle<Image>>,
    /// Extra update frames after image readiness, allowing layout and texture preparation to settle.
    pub ready_frames_remaining: u8,
    /// Response channel for the blocked script call.
    pub done: std::sync::mpsc::Sender<crate::script::ScriptResponse>,
}

/// An old screen root kept briefly under a replacement screen.
#[derive(Clone, Debug)]
pub struct StaleScreenRoot {
    /// Root entity to despawn after it is safe.
    pub entity: Entity,
    /// Minimum frames to keep the old root alive.
    pub frames_remaining: u8,
    /// Optional images to wait on before despawning.
    pub wait_images: Vec<Handle<Image>>,
}

/// Marker component for a screen root.
#[derive(Component)]
pub struct ScreenUiRoot;

/// Marker component for entities that belong to a Rhai screen.
#[derive(Component)]
pub struct ScreenUiNode;

/// Runtime interaction state for a screen button.
#[derive(Component, Clone)]
pub struct ScreenUiButton {
    /// Root this button belongs to; stale/pending roots are ignored.
    pub root: Entity,
    /// Value returned to Rhai when the button is pressed.
    pub value: StoredValue,
    /// Whether press interactions should produce a value.
    pub enabled: bool,
    /// Text child whose color changes with interaction state.
    pub text_entity: Entity,
    /// Idle background color.
    pub normal_background: Color,
    /// Hover background color.
    pub hovered_background: Color,
    /// Pressed background color.
    pub pressed_background: Color,
    /// Disabled background color.
    pub insensitive_background: Color,
    /// Idle label color.
    pub normal_text_color: Color,
    /// Hover label color.
    pub hovered_text_color: Color,
    /// Pressed label color.
    pub pressed_text_color: Color,
    /// Disabled label color.
    pub insensitive_text_color: Color,
}

/// Marker component for the text child of a screen button.
#[derive(Component)]
pub struct ScreenUiButtonText;

/// Runtime interaction state for a texture-backed screen button.
#[derive(Component, Clone)]
pub struct ScreenUiImageButton {
    /// Root this button belongs to; stale and pending roots are ignored.
    pub root: Entity,
    /// Value returned to Rhai when pressed.
    pub value: StoredValue,
    /// Whether press interactions should produce a value.
    pub enabled: bool,
    /// Whether a disabled button still displays its hover artwork.
    pub hovered_when_disabled: bool,
    /// Idle source rectangle.
    pub normal_rect: Option<Rect>,
    /// Idle image handle.
    pub normal_texture: Handle<Image>,
    /// Hover source rectangle, when supplied by the script.
    pub hovered_rect: Option<Rect>,
    /// Hover image handle, when supplied by the script.
    pub hovered_texture: Option<Handle<Image>>,
    /// Layout restored while the hovered artwork is displayed.
    pub hovered_node: Option<Node>,
    /// Layout restored while the idle artwork is displayed.
    pub normal_node: Node,
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

fn default_button_enabled() -> bool {
    true
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

/// Converts a Rhai-friendly `[r, g, b, a]` color into Bevy's sRGB color type.
pub fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}
