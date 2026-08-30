use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state::StoredValue;

/// Runtime metadata for an ordinary HKS expression captured by a declarative
/// UI property. It is skipped by serialization; UI authors never manipulate
/// this type or a signal handle directly.
#[derive(Clone, Debug)]
pub struct UiReactiveBinding {
    pub(crate) program: hiraku_script::LinkedProgram,
    pub(crate) closure: hiraku_script::native::HksClosure,
    pub(crate) globals: BTreeMap<String, hiraku_script::Value>,
}

/// A rendered declarative HKS UI root produced by `screen { ... }` or
/// `canvas { ... }`. Whether the root is modal is decided by its mount API:
/// `ui.open` blocks for a result, while `ui.mount` remains non-modal.
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

/// A single displayable in an HKS screen tree.
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
    /// Static visibility used before or instead of a live visibility binding.
    #[serde(default)]
    pub hidden: bool,
    /// Optional live Bool signal controlling whether this node participates in rendering/layout.
    #[serde(default)]
    pub visible_binding: Option<String>,
    #[serde(skip)]
    pub reactive_visibility: Option<UiReactiveBinding>,
    /// Optional engine-owned entrance/timeline animation specification.
    #[serde(default)]
    pub animation: Option<crate::script::AnimationSpec>,
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

/// Static text in an HKS screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNode {
    /// Text content.
    pub text: String,
    /// Optional normalized live template. Static story values are captured
    /// during component evaluation; unresolved fields are read from UiSignals.
    #[serde(default)]
    pub binding: Option<String>,
    #[serde(skip)]
    pub reactive_text: Option<UiReactiveBinding>,
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

/// A typed value published to declarative HKS UI.
#[derive(Clone, Debug, PartialEq)]
pub enum UiSignalValue {
    Bool(bool),
    Number(f64),
    String(String),
}

impl From<bool> for UiSignalValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

macro_rules! impl_number_signal {
    ($($type:ty),* $(,)?) => {$(
        impl From<$type> for UiSignalValue {
            fn from(value: $type) -> Self {
                Self::Number(value as f64)
            }
        }
    )*};
}

impl_number_signal!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

impl From<String> for UiSignalValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for UiSignalValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl UiSignalValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn display(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => value.clone(),
        }
    }
}

/// Engine- and game-provided typed signals consumed by live HKS UI bindings.
///
/// This resource is intentionally generic: embedding applications can publish
/// values without registering new UI native functions or exposing ECS to HKS.
#[derive(Resource, Default)]
pub struct UiSignals {
    values: BTreeMap<String, UiSignalValue>,
    revision: u64,
}

impl UiSignals {
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<UiSignalValue>) {
        let name = name.into();
        let value = value.into();
        if self.values.get(&name) == Some(&value) {
            return;
        }
        self.values.insert(name, value);
        self.revision = self
            .revision
            .checked_add(1)
            .expect("UI signal revision must not be exhausted");
    }

    pub fn remove(&mut self, name: &str) {
        if self.values.remove(name).is_some() {
            self.revision = self
                .revision
                .checked_add(1)
                .expect("UI signal revision must not be exhausted");
        }
    }

    pub fn get(&self, name: &str) -> Option<&UiSignalValue> {
        self.values.get(name)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = (&str, &UiSignalValue)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
}

/// Runtime binding attached only to text entities which read live signals.
#[derive(Component)]
pub(crate) struct UiTextBinding {
    pub(crate) template: String,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiVisibilityBinding {
    pub(crate) signal: String,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiEnabledBinding {
    pub(crate) signal: String,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiProgressBinding {
    pub(crate) signal: String,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiReactiveTextBinding {
    pub(crate) expression: UiReactiveBinding,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiReactiveVisibilityBinding {
    pub(crate) expression: UiReactiveBinding,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiReactiveEnabledBinding {
    pub(crate) expression: UiReactiveBinding,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiReactiveProgressBinding {
    pub(crate) expression: UiReactiveBinding,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) rendered_revision: u64,
}

#[derive(Component)]
pub(crate) struct UiAnimationPlayer {
    pub(crate) spec: crate::script::AnimationSpec,
    pub(crate) elapsed: f32,
}

/// A clickable text button in an HKS screen.
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
    #[serde(default)]
    pub enabled_binding: Option<String>,
    #[serde(skip)]
    pub reactive_enabled: Option<UiReactiveBinding>,
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

/// An image displayable in an HKS screen.
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
    #[serde(default)]
    pub enabled_binding: Option<String>,
    #[serde(skip)]
    pub reactive_enabled: Option<UiReactiveBinding>,
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
    #[serde(default)]
    pub binding: Option<String>,
    #[serde(skip)]
    pub reactive_value: Option<UiReactiveBinding>,
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
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// A flex container used by `vbox`, `hbox`, and `frame` HKS nodes.
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
    #[serde(default)]
    pub layout: ScreenLayout,
}

/// Runtime state for modal declarative HKS screens.
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
    /// Stable request currently blocked in `ui.open(...)`.
    pub waiting: Option<crate::script::ScriptRequestId>,
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
    /// Stable request completed by a button intent.
    pub done: Option<crate::script::ScriptRequestId>,
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

/// Marker component for entities that belong to a declarative HKS screen.
#[derive(Component)]
pub struct ScreenUiNode;

/// Runtime interaction state for a screen button.
#[derive(Component, Clone)]
pub struct ScreenUiButton {
    /// Root this button belongs to; stale/pending roots are ignored.
    pub root: Entity,
    /// Intent value returned to the story runtime when the button is pressed.
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
    /// Intent value returned to the story runtime when pressed.
    pub value: StoredValue,
    /// Whether press interactions should produce a value.
    pub enabled: bool,
    /// Whether a disabled button still displays its hover artwork.
    pub hovered_when_disabled: bool,
    /// Idle source rectangle.
    pub normal_rect: Option<Rect>,
    /// Idle image handle.
    pub normal_texture: Handle<Image>,
    /// Idle atlas section, when the texture came from the catalog.
    pub normal_atlas: Option<TextureAtlas>,
    /// Hover source rectangle, when supplied by the script.
    pub hovered_rect: Option<Rect>,
    /// Hover image handle, when supplied by the script.
    pub hovered_texture: Option<Handle<Image>>,
    /// Hover atlas section, when supplied by the script.
    pub hovered_atlas: Option<TextureAtlas>,
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

/// Converts an HKS `[r, g, b, a]` color into Bevy's sRGB color type.
pub fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}
