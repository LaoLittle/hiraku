use std::collections::BTreeMap;

use hiraku_script::native::{
    FromHksValue, HksBindable, HksBinding, HksCallable, HksClosure, IntoHksValue, NativeError,
    NativeRegistry,
};
use hiraku_script::{LinkedVm, LinkedVmEvent, RenderOptions, ScriptType, StatementValue, Value};
use thiserror::Error;

use crate::{
    glossary::{TermCatalog, TermId},
    state::StoredValue,
    texture::TextureCatalog,
    ui::{
        BarNode, ButtonNode, ContainerNode, PropertyComputation, ScreenImageButtonNode,
        ScreenImageNode, ScreenLayout, ScreenNode, ScreenSpec, ScreenTexture, ScrollableNode,
        SpacerNode, TextNode, ToggleNode, UiCallback, UiEffect, UiPhaseAnimation,
    },
};

use super::{
    animation::{AnimationPhase, AnimationSpec, register_animation_api},
    navigation::{NavigationOptions, NavigationRequest, NavigationResetValue},
    ui_runtime::UiContext,
};

const UI_NODE_HANDLE_TYPE: u32 = 0x5549_4e4f;
const UI_EFFECT_HANDLE_TYPE: u32 = 0x5549_4546;
const UI_STDLIB_PATH: &str = "hiraku://std/ui.hks";
const UI_STDLIB_SOURCE: &str = include_str!("std/ui.hks");

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "UiNode", handle_type = UI_NODE_HANDLE_TYPE)]
struct UiNodeHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "UiEffect", handle_type = UI_EFFECT_HANDLE_TYPE)]
struct UiEffectHandle(u64);

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum UiPosition {
    Absolute(f64, f64),
    Relative(f64, f64),
}

impl UiPosition {
    fn abs(x: f64, y: f64) -> UiPosition { Self::Absolute(x, y) }
    fn rel(x: f64, y: f64) -> UiPosition { Self::Relative(x, y) }
}
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum UiSize {
    Absolute(f64, f64),
    Relative(f64, f64),
    Fit,
}

impl UiSize {
    fn fit() -> UiSize { Self::Fit }
    fn abs(width: f64, height: f64) -> UiSize { Self::Absolute(width, height) }
    fn rel(width: f64, height: f64) -> UiSize { Self::Relative(width, height) }
}
}

#[derive(Clone, Debug)]
enum UiDraftKind {
    Screen,
    Column,
    Row,
    Scrollable,
    Toggle(HksBindable<bool>),
    Checkbox(HksBindable<bool>),
    Slider(HksBindable<f64>, f64, f64),
    TextInput(HksBindable<String>),
    ChoiceOptions(HksCallable),
    Image(String),
    Text(HksBindable<String>),
    Term(TermId),
    Button(Value),
    Progress {
        value: HksBindable<f64>,
        min: f32,
        max: f32,
    },
    Spacer,
}

#[derive(Clone, Debug)]
struct UiDraft {
    allowed_overlays: Vec<String>,
    fade_seconds: f32,
    timers_paused: bool,
    pauses_scene: bool,
    timers: Vec<(f32, HksCallable)>,
    slider_skin: Option<[String; 3]>,
    kind: UiDraftKind,
    content: Option<HksClosure>,
    hovered: Option<HksClosure>,
    pressed: Option<HksClosure>,
    checked: Option<HksClosure>,
    on_click: Option<HksClosure>,
    on_change: Option<HksCallable>,
    on_commit: Option<HksCallable>,
    placeholder: String,
    layout: ScreenLayout,
    panel: bool,
    enabled: bool,
    enabled_binding: Option<HksBinding<bool>>,
    visible: bool,
    visible_binding: Option<HksBinding<bool>>,
    hovered_when_disabled: bool,
    hover_scale: f32,
    hover_active: Option<HksBinding<bool>>,
    text_reveal: Option<HksBinding<i64>>,
    press_scale: f32,
    scroll_speed: f32,
    default_scroll_anchor: crate::ui::ScrollAnchor,
    gap: f32,
    padding: f32,
    surface: Option<[f32; 4]>,
    hovered_surface: Option<[f32; 4]>,
    text_size: Option<f32>,
    text_color: Option<[f32; 4]>,
    text_align: Option<f32>,
    centered: bool,
    background_texture: Option<String>,
    button_background_texture: Option<String>,
    button_hovered_background_texture: Option<String>,
    overlay: Option<[f32; 4]>,
    animation: Option<AnimationSpec>,
    phase_animation: Option<UiPhaseAnimation>,
}

impl UiDraft {
    fn new(kind: UiDraftKind, content: Option<HksClosure>) -> Self {
        Self {
            allowed_overlays: Vec::new(),
            timers: Vec::new(),
            timers_paused: false,
            fade_seconds: 0.0,
            pauses_scene: false,
            kind,
            slider_skin: None,
            content,
            hovered: None,
            pressed: None,
            checked: None,
            on_click: None,
            on_change: None,
            on_commit: None,
            placeholder: String::new(),
            layout: ScreenLayout::default(),
            panel: true,
            enabled: true,
            enabled_binding: None,
            visible: true,
            visible_binding: None,
            hovered_when_disabled: false,
            hover_scale: 1.0,
            hover_active: None,
            text_reveal: None,
            press_scale: 1.0,
            scroll_speed: 48.0,
            default_scroll_anchor: crate::ui::ScrollAnchor::Top,
            gap: 12.0,
            padding: 0.0,
            surface: None,
            hovered_surface: None,
            text_size: None,
            text_color: None,
            text_align: None,
            centered: false,
            background_texture: None,
            button_background_texture: None,
            button_hovered_background_texture: None,
            overlay: None,
            animation: None,
            phase_animation: None,
        }
    }
}

struct UiVmContext {
    owned_globals: std::collections::BTreeSet<String>,
    local_globals: BTreeMap<String, Value>,
    values: UiContext,
    terms: TermCatalog,
    next_node: u64,
    nodes: BTreeMap<u64, UiDraft>,
    next_effect: u64,
    effects: BTreeMap<u64, UiEffect>,
    navigation_origin: Option<String>,
}

impl UiVmContext {
    fn new(values: UiContext, terms: TermCatalog) -> Self {
        Self {
            owned_globals: Default::default(),
            local_globals: Default::default(),
            values,
            terms,
            next_node: 0,
            nodes: BTreeMap::new(),
            next_effect: 0,
            effects: BTreeMap::new(),
            navigation_origin: None,
        }
    }

    fn with_navigation_origin(mut self, origin: impl Into<String>) -> Self {
        self.navigation_origin = Some(origin.into());
        self
    }

    fn insert(&mut self, draft: UiDraft) -> UiNodeHandle {
        self.next_node += 1;
        self.nodes.insert(self.next_node, draft);
        UiNodeHandle(self.next_node)
    }

    fn node_mut(&mut self, handle: UiNodeHandle) -> Result<&mut UiDraft, NativeError> {
        self.nodes
            .get_mut(&handle.0)
            .ok_or_else(|| NativeError::message(format!("unknown UiNode handle {}", handle.0)))
    }

    fn insert_effect(&mut self, effect: UiEffect) -> UiEffectHandle {
        self.next_effect += 1;
        self.effects.insert(self.next_effect, effect);
        UiEffectHandle(self.next_effect)
    }
}

#[hiraku_script::hks_module]
mod native_ui {
    use super::*;

    #[hks(name = "__uiScreen")]
    fn ui_screen(
        context: &mut UiVmContext,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Screen, Some(content))))
    }

    #[hks(name = "__uiColumn")]
    fn ui_column(
        context: &mut UiVmContext,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Column, Some(content))))
    }

    #[hks(name = "__uiRow")]
    fn ui_row(context: &mut UiVmContext, content: HksClosure) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Row, Some(content))))
    }

    #[hks(name = "__uiScrollable")]
    fn ui_scrollable(
        context: &mut UiVmContext,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Scrollable, Some(content))))
    }

    #[hks(name = "toggle")]
    fn ui_toggle(
        context: &mut UiVmContext,
        value: HksBindable<bool>,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Toggle(value), Some(content))))
    }

    #[hks(name = "choiceOptions")]
    fn ui_choice_options(
        context: &mut UiVmContext,
        renderer: HksCallable,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::ChoiceOptions(renderer), None)))
    }

    #[hks(name = "checkbox")]
    fn checkbox(
        context: &mut UiVmContext,
        value: HksBindable<bool>,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Checkbox(value), None)))
    }

    #[hks(name = "slider")]
    fn slider(
        context: &mut UiVmContext,
        value: HksBindable<f64>,
        min: f64,
        max: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        if !min.is_finite() || !max.is_finite() || min >= max {
            return Err(NativeError::message("slider requires finite min < max"));
        }
        Ok(context.insert(UiDraft::new(UiDraftKind::Slider(value, min, max), None)))
    }

    #[hks(name = "skin", receiver)]
    fn slider_skin(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        track: String,
        fill: String,
        thumb: String,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context
            .nodes
            .get_mut(&node.0)
            .ok_or_else(|| NativeError::message("unknown UI node"))?;
        if !matches!(draft.kind, UiDraftKind::Slider(..)) {
            return Err(NativeError::message("skin requires a slider"));
        }
        draft.slider_skin = Some([track, fill, thumb]);
        Ok(node)
    }

    #[hks(name = "textInput")]
    fn text_input(
        context: &mut UiVmContext,
        value: HksBindable<String>,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::TextInput(value), None)))
    }

    #[hks(name = "placeholder", receiver)]
    fn placeholder(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        text: String,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::TextInput(_)) {
            return Err(NativeError::message("placeholder requires textInput"));
        }
        draft.placeholder = text;
        Ok(node)
    }

    #[hks(name = "onChange", receiver)]
    fn on_change(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        handler: HksCallable,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(
            draft.kind,
            UiDraftKind::Toggle(_)
                | UiDraftKind::Checkbox(_)
                | UiDraftKind::Slider(..)
                | UiDraftKind::TextInput(_)
        ) {
            return Err(NativeError::message("onChange requires an input widget"));
        }
        draft.on_change = Some(handler);
        Ok(node)
    }

    #[hks(name = "pauseTimers", receiver)]
    fn pause_timers(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        paused: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Screen) {
            return Err(NativeError::message(
                "pauseTimers requires a screen or canvas",
            ));
        }
        draft.timers_paused = paused;
        Ok(node)
    }

    #[hks(name = "fade", receiver)]
    fn fade(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        seconds: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Screen) {
            return Err(NativeError::message("fade requires a screen or canvas"));
        }
        if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
            return Err(NativeError::message(
                "fade duration must be within 0..60 seconds",
            ));
        }
        draft.fade_seconds = seconds as f32;
        Ok(node)
    }

    #[hks(name = "hoverBrightness", receiver)]
    fn hover_brightness(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        factor: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        if !factor.is_finite() || !(0.0..=4.0).contains(&factor) {
            return Err(NativeError::message("hover brightness must be within 0..4"));
        }
        context.node_mut(node)?.layout.hover_brightness = Some(factor as f32);
        Ok(node)
    }

    #[hks(name = "keyframes", receiver)]
    fn keyframes(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        frames: Vec<crate::ui::UiKeyframe>,
    ) -> Result<UiNodeHandle, NativeError> {
        if !crate::scene::ui_keyframes::validate(&frames) {
            return Err(NativeError::message(
                "keyframes require increasing non-negative times, finite rectangles and alpha in 0..1",
            ));
        }
        context.node_mut(node)?.layout.keyframes = frames;
        Ok(node)
    }

    /// Read-only collection size; element values never leave the Any boundary.
    #[hks]
    fn count(_context: &mut UiVmContext, values: Vec<Value>) -> Result<i32, NativeError> {
        i32::try_from(values.len())
            .map_err(|_| NativeError::message("collection size exceeds Int range"))
    }

    #[hks]
    fn item(
        _context: &mut UiVmContext,
        values: Vec<Value>,
        index: i32,
    ) -> Result<Value, NativeError> {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .cloned()
            .ok_or_else(|| NativeError::message("collection index out of bounds"))
    }

    #[hks(name = "shader", receiver)]
    fn shader(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        path: String,
        textures: Vec<String>,
        keys: Vec<crate::ui::UiShaderKeyframe>,
    ) -> Result<UiNodeHandle, NativeError> {
        if textures.len() > 3 || !crate::render::ui_quad::valid_shader_keys(&keys) {
            return Err(NativeError::message(
                "shader accepts up to three auxiliary textures and increasing finite parameter keyframes",
            ));
        }
        let path =
            bevy::asset::AssetPath::parse(context.navigation_origin.as_deref().unwrap_or(""))
                .resolve_embed_str(&path)
                .map_err(|e| NativeError::message(e.to_string()))?
                .to_string();
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Image(_)) {
            return Err(NativeError::message("shader requires an image node"));
        }
        draft.layout.shader = Some(crate::ui::UiShaderSpec {
            path,
            textures,
            keys,
            blend: crate::ui::UiShaderBlend::Alpha,
        });
        Ok(node)
    }

    #[hks(name = "blend", receiver)]
    fn blend(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        mode: String,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        let shader = draft
            .layout
            .shader
            .as_mut()
            .ok_or_else(|| NativeError::message("blend requires shader(...) on an image"))?;
        shader.blend = match mode.as_str() {
            "alpha" => crate::ui::UiShaderBlend::Alpha,
            "multiply" => crate::ui::UiShaderBlend::Multiply,
            "additive" => crate::ui::UiShaderBlend::Additive,
            _ => {
                return Err(NativeError::message(
                    "UI shader blend must be alpha, multiply or additive",
                ));
            }
        };
        Ok(node)
    }

    /// Opt into scene suspension without teaching the engine about menus or
    /// minigames. UI timers/animations still belong to their own screen clock.
    #[hks(name = "allowOverlay", receiver)]
    fn allow_overlay(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        name: String,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Screen) {
            return Err(NativeError::message(
                "allowOverlay requires a screen or canvas",
            ));
        }
        if !draft.allowed_overlays.contains(&name) {
            draft.allowed_overlays.push(name);
        }
        Ok(node)
    }

    /// Constrain wrapping without fixing the text's content-driven height.
    #[hks(name = "width", receiver)]
    fn width(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        width: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        let layout = &mut context.node_mut(node)?.layout;
        layout.width = Some(non_negative(width, "UI width")?);
        layout.width_percent = None;
        Ok(node)
    }

    #[hks(name = "pauseScene", receiver)]
    fn pause_scene(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        paused: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Screen) {
            return Err(NativeError::message(
                "pauseScene requires a screen or canvas",
            ));
        }
        draft.pauses_scene = paused;
        Ok(node)
    }

    #[hks(name = "after", receiver)]
    fn after(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        seconds: f64,
        handler: HksCallable,
    ) -> Result<UiNodeHandle, NativeError> {
        if !seconds.is_finite() || !(0.0..=86400.0).contains(&seconds) {
            return Err(NativeError::message(
                "after requires a finite duration in 0..=86400 seconds",
            ));
        }
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Screen) {
            return Err(NativeError::message("after requires a screen or canvas"));
        }
        draft.timers.push((seconds as f32, handler));
        Ok(node)
    }

    #[hks(name = "onCommit", receiver)]
    fn on_commit(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        handler: HksCallable,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(
            draft.kind,
            UiDraftKind::Slider(..) | UiDraftKind::TextInput(_)
        ) {
            return Err(NativeError::message(
                "onCommit requires slider or textInput",
            ));
        }
        draft.on_commit = Some(handler);
        Ok(node)
    }

    #[hks(name = "__uiImage")]
    fn ui_image(context: &mut UiVmContext, path: String) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Image(path), None)))
    }

    #[hks(name = "text")]
    fn ui_text(
        context: &mut UiVmContext,
        value: HksBindable<String>,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Text(value), None)))
    }

    #[hks(name = "prefix", receiver)]
    fn string_prefix(
        _context: &mut UiVmContext,
        value: String,
        characters: i64,
    ) -> Result<String, NativeError> {
        let count = usize::try_from(characters)
            .map_err(|_| NativeError::message("prefix count must be nonnegative"))?;
        Ok(value.chars().take(count).collect())
    }

    #[hks(name = "richText")]
    fn rich_text(
        context: &mut UiVmContext,
        value: HksBindable<String>,
    ) -> Result<UiNodeHandle, NativeError> {
        let mut draft = UiDraft::new(UiDraftKind::Text(value), None);
        draft.layout.rich_text = true;
        Ok(context.insert(draft))
    }

    #[hks(name = "reveal", receiver)]
    fn text_reveal(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        count: HksBindable<i64>,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !draft.layout.rich_text {
            return Err(NativeError::message("reveal requires richText"));
        }
        match count {
            HksBindable::Value(value) => {
                draft.layout.text_reveal = Some(u32::try_from(value).map_err(|_| {
                    NativeError::message("reveal count must fit a nonnegative 32-bit integer")
                })?);
                draft.text_reveal = None;
            }
            HksBindable::Binding(binding) => draft.text_reveal = Some(binding),
        }
        Ok(node)
    }

    #[hks(name = "__uiTerm")]
    fn ui_term(context: &mut UiVmContext, id: String) -> Result<UiNodeHandle, NativeError> {
        let term = context
            .terms
            .resolve(&id)
            .ok_or_else(|| NativeError::message(format!("term `{id}` is not defined")))?;
        Ok(context.insert(UiDraft::new(UiDraftKind::Term(term), None)))
    }

    #[hks(name = "button", raw)]
    fn ui_button(
        context: &mut UiVmContext,
        call: &hiraku_script::BuiltinCall,
    ) -> Result<Value, NativeError> {
        let (value, content) = match call.arguments.as_slice() {
            [content] => (Value::Unit, HksClosure::from_hks_value(&content.value)?),
            [value, content] => (
                value.value.clone(),
                HksClosure::from_hks_value(&content.value)?,
            ),
            arguments => {
                return Err(NativeError::Arity {
                    expected: 2,
                    actual: arguments.len(),
                });
            }
        };
        Ok(context
            .insert(UiDraft::new(UiDraftKind::Button(value), Some(content)))
            .into_hks_value())
    }

    #[hks(name = "__uiSpacer")]
    fn ui_spacer(context: &mut UiVmContext) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Spacer, None)))
    }

    #[hks(name = "progress")]
    fn ui_progress(
        context: &mut UiVmContext,
        value: HksBindable<f64>,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(
            UiDraftKind::Progress {
                value,
                min: 0.0,
                max: 1.0,
            },
            None,
        )))
    }

    #[hks(name = "at", receiver)]
    fn ui_at(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        position: UiPosition,
    ) -> Result<UiNodeHandle, NativeError> {
        let layout = &mut context.node_mut(node)?.layout;
        match position {
            UiPosition::Absolute(x, y) => {
                layout.left = Some(finite_f32(x, "absolute UI x")?);
                layout.top = Some(finite_f32(y, "absolute UI y")?);
            }
            UiPosition::Relative(x, y) => {
                layout.left_percent = Some(percent(x, "relative UI x")?);
                layout.top_percent = Some(percent(y, "relative UI y")?);
            }
        }
        Ok(node)
    }

    #[hks(name = "size", receiver)]
    fn ui_size(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        size: UiSize,
    ) -> Result<UiNodeHandle, NativeError> {
        let layout = &mut context.node_mut(node)?.layout;
        match size {
            UiSize::Fit => {
                layout.fit_content = true;
                layout.width = None;
                layout.height = None;
                layout.width_percent = None;
                layout.height_percent = None;
            }
            UiSize::Absolute(width, height) => {
                layout.width = Some(non_negative(width, "absolute UI width")?);
                layout.height = Some(non_negative(height, "absolute UI height")?);
            }
            UiSize::Relative(width, height) => {
                layout.width_percent = Some(percent(width, "relative UI width")?);
                layout.height_percent = Some(percent(height, "relative UI height")?);
            }
        }
        Ok(node)
    }

    #[hks(name = "gap", receiver)]
    fn ui_gap(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        gap: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.gap = non_negative(gap, "UI gap")?;
        Ok(node)
    }

    #[hks(name = "padding", receiver)]
    fn ui_padding(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        padding: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.padding = non_negative(padding, "UI padding")?;
        Ok(node)
    }

    #[hks(name = "surface", receiver)]
    fn ui_surface(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.surface = Some([
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ]);
        Ok(node)
    }

    /// Set a custom-content button's hover surface without replacing its children.
    #[hks(name = "hoveredSurface", receiver)]
    fn ui_hovered_surface(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hovered_surface = Some([
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ]);
        Ok(node)
    }

    #[hks(name = "fontSize", receiver)]
    fn ui_font_size(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        size: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.text_size = Some(non_negative(size, "UI font size")?);
        Ok(node)
    }

    /// Horizontal text alignment: 0 = left, 0.5 = center, 1 = right.
    #[hks(name = "textAlign", receiver)]
    fn ui_text_align(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        align: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        if !align.is_finite() || !(0.0..=1.0).contains(&align) {
            return Err(NativeError::message(
                "text alignment must be between 0 and 1",
            ));
        }
        context.node_mut(node)?.text_align = Some(align as f32);
        Ok(node)
    }

    #[hks(name = "color", receiver)]
    fn ui_color(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.text_color = Some([
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ]);
        Ok(node)
    }

    #[hks(name = "centered", receiver)]
    fn centered(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(
            draft.kind,
            UiDraftKind::Row | UiDraftKind::Column | UiDraftKind::ChoiceOptions(_)
        ) {
            return Err(NativeError::message(
                "centered requires row, column or choiceOptions",
            ));
        }
        draft.centered = true;
        Ok(node)
    }

    #[hks(name = "rotation", receiver)]
    fn rotation(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        degrees: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        if !degrees.is_finite() {
            return Err(NativeError::message("rotation requires a finite angle"));
        }
        context.node_mut(node)?.layout.rotation = (degrees % 360.0) as f32;
        Ok(node)
    }

    #[hks(name = "panel", receiver)]
    fn ui_panel(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        panel: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.panel = panel;
        Ok(node)
    }

    #[hks(name = "background", receiver)]
    fn ui_background(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        texture: String,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.background_texture = Some(texture);
        Ok(node)
    }

    #[hks(name = "buttonImage", receiver)]
    fn ui_button_image(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        texture: String,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.button_background_texture = Some(texture);
        Ok(node)
    }

    #[hks(name = "hoveredButtonImage", receiver)]
    fn ui_hovered_button_image(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        texture: String,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.button_hovered_background_texture = Some(texture);
        Ok(node)
    }

    #[hks(name = "overlay", receiver)]
    fn ui_overlay(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.overlay = Some([
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ]);
        Ok(node)
    }

    #[hks(name = "enabled", receiver)]
    fn ui_enabled(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        enabled: HksBindable<bool>,
    ) -> Result<UiNodeHandle, NativeError> {
        match enabled {
            HksBindable::Value(enabled) => context.node_mut(node)?.enabled = enabled,
            HksBindable::Binding(enabled) => {
                context.node_mut(node)?.enabled_binding = Some(enabled)
            }
        }
        Ok(node)
    }

    #[hks(name = "visible", receiver)]
    fn ui_visible(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        visible: HksBindable<bool>,
    ) -> Result<UiNodeHandle, NativeError> {
        match visible {
            HksBindable::Value(visible) => context.node_mut(node)?.visible = visible,
            HksBindable::Binding(visible) => {
                context.node_mut(node)?.visible_binding = Some(visible)
            }
        }
        Ok(node)
    }

    #[hks(name = "clip", receiver)]
    fn ui_clip(context: &mut UiVmContext, node: UiNodeHandle) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.layout.clip = true;
        Ok(node)
    }

    #[hks(name = "stretch", receiver)]
    fn ui_stretch(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Image(_)) {
            return Err(NativeError::message("stretch requires an image node"));
        }
        draft.layout.image_stretch = true;
        Ok(node)
    }

    #[hks(name = "tint", receiver)]
    fn ui_tint(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        let tint = [
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ];
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Image(_)) {
            return Err(NativeError::message("tint requires an image node"));
        }
        draft.layout.image_tint = Some(tint);
        Ok(node)
    }

    #[hks(name = "textShadow", receiver)]
    fn ui_text_shadow(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        enabled: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.layout.text_shadow = Some(enabled);
        Ok(node)
    }

    #[hks(name = "fitText", receiver)]
    fn ui_fit_text(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Text(_)) || draft.layout.rich_text {
            return Err(NativeError::message(
                "fitText requires a plain text node; richText uses wrapping layout",
            ));
        }
        draft.layout.text_fit = true;
        Ok(node)
    }

    #[hks(name = "flipX", receiver)]
    fn ui_flip_x(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Image(_)) {
            return Err(NativeError::message("flipX requires an image node"));
        }
        draft.layout.flip_x = true;
        Ok(node)
    }

    #[hks(name = "range", receiver)]
    fn ui_range(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        min: f64,
        max: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        let min = finite_f32(min, "progress minimum")?;
        let max = finite_f32(max, "progress maximum")?;
        if max <= min {
            return Err(NativeError::message(
                "progress maximum must be greater than its minimum",
            ));
        }
        let UiDraftKind::Progress {
            min: draft_min,
            max: draft_max,
            ..
        } = &mut context.node_mut(node)?.kind
        else {
            return Err(NativeError::message(
                "range is only valid on progress nodes",
            ));
        };
        *draft_min = min;
        *draft_max = max;
        Ok(node)
    }

    #[hks(name = "hoveredWhenDisabled", receiver)]
    fn ui_hovered_when_disabled(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        enabled: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hovered_when_disabled = enabled;
        Ok(node)
    }

    #[hks(name = "hoverScale", receiver)]
    fn ui_hover_scale(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        scale: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hover_scale = positive(scale, "UI hover scale")?;
        Ok(node)
    }

    /// Moves a node and its descendants on hover; active optionally holds that pose.
    #[hks(name = "hoverOffset", receiver)]
    fn ui_hover_offset(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        x: f64,
        y: f64,
        active: Option<HksBindable<bool>>,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        draft.layout.hover_offset = Some([finite_f32(x, "hover x")?, finite_f32(y, "hover y")?]);
        match active.unwrap_or(HksBindable::Value(false)) {
            HksBindable::Value(active) => {
                draft.layout.hover_active = active;
                draft.hover_active = None;
            }
            HksBindable::Binding(binding) => draft.hover_active = Some(binding),
        }
        Ok(node)
    }

    #[hks(name = "pressScale", receiver)]
    fn ui_press_scale(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        scale: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.press_scale = positive(scale, "UI press scale")?;
        Ok(node)
    }

    #[hks(name = "scrollSpeed", receiver)]
    fn ui_scroll_speed(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        speed: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.scroll_speed = positive(speed, "UI scroll speed")?;
        Ok(node)
    }

    #[hks(name = "defaultScrollAnchor", receiver)]
    fn default_scroll_anchor(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        anchor: crate::ui::ScrollAnchor,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Scrollable) {
            return Err(NativeError::message(
                "defaultScrollAnchor requires scrollable",
            ));
        }
        draft.default_scroll_anchor = anchor;
        Ok(node)
    }

    #[hks(name = "checked", receiver)]
    fn ui_checked(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Toggle(_)) {
            return Err(NativeError::message(
                "checked content is only valid on toggle nodes",
            ));
        }
        draft.checked = Some(content);
        Ok(node)
    }

    #[hks(name = "hovered", receiver)]
    fn ui_hovered(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hovered = Some(content);
        Ok(node)
    }

    #[hks(name = "pressed", receiver)]
    fn ui_pressed(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Button(_)) {
            return Err(NativeError::message(
                "pressed content is only valid on image buttons",
            ));
        }
        draft.pressed = Some(content);
        Ok(node)
    }

    /// Builds a one-shot sound effect for a button's click handler. Calling
    /// this function is pure; playback starts only after the button accepts a
    /// release event.
    #[hks(name = "sfx")]
    fn ui_sfx(
        context: &mut UiVmContext,
        name: String,
        volume: Option<f64>,
    ) -> Result<UiEffectHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message(
                "UI sound effect name must not be empty",
            ));
        }
        let volume = volume.unwrap_or(1.0);
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(NativeError::message(
                "UI sound effect volume must be between 0 and 1",
            ));
        }
        Ok(context.insert_effect(UiEffect::PlaySfx {
            name,
            volume: volume as f32,
        }))
    }

    #[hks(name = "onClick", receiver)]
    fn ui_on_click(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        handler: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        let draft = context.node_mut(node)?;
        if !matches!(draft.kind, UiDraftKind::Button(_)) {
            return Err(NativeError::message(
                "onClick can only be applied to button nodes",
            ));
        }
        draft.on_click = Some(handler);
        Ok(node)
    }

    #[hks(name = "time", receiver)]
    fn ui_time(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        seconds: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        let spec = ui_animation_spec(context, node)?.with_time(seconds)?;
        ui_animation(context, node, spec)
    }
    #[hks(name = "easing", receiver)]
    fn ui_easing(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        easing: crate::script::animation::Easing,
    ) -> Result<UiNodeHandle, NativeError> {
        let spec = ui_animation_spec(context, node)?.with_easing(easing)?;
        ui_animation(context, node, spec)
    }
    #[hks(name = "repeatForever", receiver)]
    fn ui_repeat_forever(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<UiNodeHandle, NativeError> {
        let spec = ui_animation_spec(context, node)?.repeat_forever();
        ui_animation(context, node, spec)
    }

    fn ui_animation_spec(
        context: &mut UiVmContext,
        node: UiNodeHandle,
    ) -> Result<AnimationSpec, NativeError> {
        let draft = context.node_mut(node)?;
        Ok(draft
            .phase_animation
            .as_ref()
            .map(|p| p.spec)
            .or(draft.animation)
            .unwrap_or(AnimationSpec::Linear(0.3, false)))
    }
    fn ui_animation(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        animation: AnimationSpec,
    ) -> Result<UiNodeHandle, NativeError> {
        if !animation.duration().is_finite() || animation.duration() <= 0.0 {
            return Err(NativeError::message(
                "UI animation duration must be greater than zero",
            ));
        }
        let draft = context.node_mut(node)?;
        if let Some(phase) = draft.phase_animation.as_mut() {
            phase.spec = animation;
        } else {
            draft.animation = Some(animation);
        }
        Ok(node)
    }

    #[allow(non_snake_case)]
    #[hks(name = "phaseAnimator", receiver)]
    fn ui_phase_animator(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        phases: Vec<AnimationPhase>,
    ) -> Result<UiNodeHandle, NativeError> {
        let animation = ui_animation_spec(context, node)?;
        validate_phase_animation(&phases, animation)?;
        context.node_mut(node)?.phase_animation = Some(UiPhaseAnimation {
            phases,
            spec: animation,
            continuous_rotation: false,
        });
        Ok(node)
    }

    #[hks(name = "spin", receiver)]
    fn ui_spin(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        seconds: Option<f64>,
    ) -> Result<UiNodeHandle, NativeError> {
        let seconds = animation_seconds(seconds, 1.0)?;
        context.node_mut(node)?.phase_animation = Some(UiPhaseAnimation {
            phases: vec![
                AnimationPhase::Transform(0.0, 1.0, 0.0, 0.0),
                AnimationPhase::Transform(360.0, 1.0, 0.0, 0.0),
            ],
            spec: AnimationSpec::Linear(seconds, true),
            continuous_rotation: true,
        });
        Ok(node)
    }

    #[hks(name = "pulse", receiver)]
    fn ui_pulse(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        seconds: Option<f64>,
    ) -> Result<UiNodeHandle, NativeError> {
        let seconds = animation_seconds(seconds, 1.0)?;
        context.node_mut(node)?.phase_animation = Some(UiPhaseAnimation {
            phases: vec![
                AnimationPhase::Transform(0.0, 1.0, 0.0, 0.0),
                AnimationPhase::Transform(0.0, 1.06, 0.0, 0.0),
            ],
            spec: AnimationSpec::EaseInOut(seconds, true),
            continuous_rotation: false,
        });
        Ok(node)
    }

    #[hks(name = "bob", receiver)]
    fn ui_bob(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        distance: Option<f64>,
        seconds: Option<f64>,
    ) -> Result<UiNodeHandle, NativeError> {
        let seconds = animation_seconds(seconds, 1.0)?;
        let distance = distance.unwrap_or(8.0);
        if !distance.is_finite() {
            return Err(NativeError::message("bob distance must be finite"));
        }
        context.node_mut(node)?.phase_animation = Some(UiPhaseAnimation {
            phases: vec![
                AnimationPhase::Transform(0.0, 1.0, 0.0, 0.0),
                AnimationPhase::Transform(0.0, 1.0, 0.0, distance),
            ],
            spec: AnimationSpec::EaseInOut(seconds, true),
            continuous_rotation: false,
        });
        Ok(node)
    }
}

#[hiraku_script::hks_module("ui")]
mod ui_actions {
    use super::*;

    #[hks(raw)]
    fn native_open(
        context: &mut UiVmContext,
        call: &hiraku_script::BuiltinCall,
    ) -> Result<Value, NativeError> {
        let Some(first) = call.arguments.first() else {
            return Err(NativeError::message(
                "ui.open requires a role or component path",
            ));
        };
        let role = String::from_hks_value(&first.value)?;
        if role.trim().is_empty() {
            return Err(NativeError::message("UI role must not be empty"));
        }
        let arguments = call
            .arguments
            .iter()
            .skip(1)
            .map(|argument| ui_argument_to_stored(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(context
            .insert_effect(UiEffect::OpenUi {
                role,
                origin: context.navigation_origin.clone(),
                arguments,
            })
            .into_hks_value())
    }
}

/// The current persisted UI input boundary accepts plain data. Never silently
/// drop unsupported list elements or fields, or erase nominal type metadata.
pub(crate) fn ui_argument_to_stored(value: &Value) -> Result<StoredValue, NativeError> {
    match value {
        Value::Bool(value) => Ok(StoredValue::Bool(*value)),
        Value::Number(value) => Ok(StoredValue::Float(*value)),
        Value::Int(value) => Ok(StoredValue::Int(*value)),
        Value::UInt(value) => Ok(StoredValue::UInt(*value)),
        Value::String(value) => Ok(StoredValue::String(value.clone())),
        Value::List(values) => values
            .iter()
            .map(ui_argument_to_stored)
            .collect::<Result<Vec<_>, _>>()
            .map(StoredValue::Array),
        Value::Map(fields) => fields
            .iter()
            .map(|(key, value)| ui_argument_to_stored(value).map(|value| (key.clone(), value)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(StoredValue::Map),
        _ => Err(NativeError::message(
            "UI arguments currently require plain String/Bool/number/List/record data; optional, tuple, nominal values, functions and live handles require a richer transport",
        )),
    }
}

fn close_ui(
    context: &mut UiVmContext,
    call: &hiraku_script::BuiltinCall,
) -> Result<Value, NativeError> {
    ui_result(context, call, false)
}
fn complete_ui(
    context: &mut UiVmContext,
    call: &hiraku_script::BuiltinCall,
) -> Result<Value, NativeError> {
    ui_result(context, call, true)
}
fn ui_result(
    context: &mut UiVmContext,
    call: &hiraku_script::BuiltinCall,
    complete: bool,
) -> Result<Value, NativeError> {
    let value = match call.arguments.as_slice() {
        [] => Value::Unit,
        [argument] => argument.value.clone(),
        _ => {
            return Err(NativeError::message(
                "ui.close expects zero or one result value",
            ));
        }
    };
    validate_ui_result(&value)?;
    Ok(context
        .insert_effect(if complete {
            UiEffect::CompleteUi { value }
        } else {
            UiEffect::CloseUi { value }
        })
        .into_hks_value())
}

fn validate_ui_result(value: &Value) -> Result<(), NativeError> {
    match value {
        Value::Unit
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::Int(_)
        | Value::UInt(_)
        | Value::Percent(_)
        | Value::String(_)
        | Value::Optional(None) => Ok(()),
        Value::Optional(Some(value)) => validate_ui_result(value),
        Value::Map(fields) => fields.values().try_for_each(validate_ui_result),
        Value::List(values) | Value::Tuple(values) => {
            values.iter().try_for_each(validate_ui_result)
        }
        Value::Typed { .. } => Err(NativeError::message(
            "named UI results require shared type metadata between the UI and story modules; return plain data until type linking is available",
        )),
        _ => Err(NativeError::message(
            "UI results must be owned data, not function references, live handles, or UI nodes",
        )),
    }
}

#[hiraku_script::hks_module("storage")]
mod storage_actions {
    use super::*;

    #[hks]
    fn thumbnail(_context: &mut UiVmContext, slot: String) -> Result<Option<String>, NativeError> {
        if !crate::storage::save_slot_exists(&slot)
            .map_err(|e| NativeError::message(e.to_string()))?
        {
            return Ok(None);
        }
        match crate::storage::load_save_metadata(&slot) {
            Ok(data) => Ok(data
                .has_thumbnail
                .then(|| format!("save-thumbnail://{slot}"))),
            Err(error) => {
                bevy::log::warn!("cannot read preview for slot `{slot}`: {error}");
                Ok(None)
            }
        }
    }

    /// Validate the snapshot independently of its metadata and preview.
    /// Runtime fingerprints are still checked when restoration is requested.
    #[hks]
    fn problem(_context: &mut UiVmContext, slot: String) -> Result<Option<String>, NativeError> {
        if !crate::storage::save_slot_exists(&slot)
            .map_err(|e| NativeError::message(e.to_string()))?
        {
            return Ok(None);
        }
        Ok(match crate::storage::load_save_metadata(&slot) {
            Ok(data) if data.version == crate::state::CURRENT_SAVE_VERSION => {
                crate::storage::load_save_data(&slot)
                    .err()
                    .map(|error| error.to_string())
            }
            Ok(data) => Some(format!("Incompatible save version {}", data.version)),
            Err(error) => Some(error.to_string()),
        })
    }

    #[hks(name = "exists")]
    fn slot_exists(_context: &mut UiVmContext, slot: String) -> Result<bool, NativeError> {
        crate::storage::save_slot_exists(&slot)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks]
    fn native_save(context: &mut UiVmContext, slot: String) -> Result<UiEffectHandle, NativeError> {
        if slot.trim().is_empty() {
            return Err(NativeError::message("save slot must not be empty"));
        }
        Ok(context.insert_effect(UiEffect::Save { slot }))
    }

    #[hks]
    fn native_load(context: &mut UiVmContext, slot: String) -> Result<UiEffectHandle, NativeError> {
        if slot.trim().is_empty() {
            return Err(NativeError::message("save slot must not be empty"));
        }
        Ok(context.insert_effect(UiEffect::Load { slot }))
    }
}

#[hiraku_script::hks_module("preferences")]
mod preference_actions {
    use super::*;
    use crate::storage::{PreferenceChange, UserSettings};

    fn change(
        context: &mut UiVmContext,
        value: PreferenceChange,
    ) -> Result<UiEffectHandle, NativeError> {
        UserSettings::default()
            .apply(&value)
            .map_err(NativeError::message)?;
        Ok(context.insert_effect(UiEffect::SetPreference(value)))
    }

    #[hks(name = "masterVolume")]
    fn master_volume(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(f64::from(context.values.preferences().master_volume))
    }
    #[hks(name = "textSpeed")]
    fn text_speed(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(f64::from(context.values.preferences().text_speed))
    }
    #[hks(name = "autoDelay")]
    fn auto_delay(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(f64::from(context.values.preferences().auto_delay))
    }
    #[hks(name = "fullscreen")]
    fn fullscreen(context: &mut UiVmContext) -> Result<bool, NativeError> {
        Ok(context.values.preferences().fullscreen)
    }
    #[hks(name = "displayAvailable")]
    fn display_available(context: &mut UiVmContext) -> Result<bool, NativeError> {
        Ok(context.values.preferences().display_available)
    }
    #[hks(name = "setMasterVolume")]
    fn set_master_volume(
        context: &mut UiVmContext,
        value: f64,
    ) -> Result<UiEffectHandle, NativeError> {
        change(context, PreferenceChange::MasterVolume(value as f32))
    }
    #[hks(name = "setTextSpeed")]
    fn set_text_speed(
        context: &mut UiVmContext,
        value: f64,
    ) -> Result<UiEffectHandle, NativeError> {
        change(context, PreferenceChange::TextSpeed(value as f32))
    }
    #[hks(name = "setAutoDelay")]
    fn set_auto_delay(
        context: &mut UiVmContext,
        value: f64,
    ) -> Result<UiEffectHandle, NativeError> {
        change(context, PreferenceChange::AutoDelay(value as f32))
    }
    #[hks(name = "setFullscreen")]
    fn set_fullscreen(
        context: &mut UiVmContext,
        value: bool,
    ) -> Result<UiEffectHandle, NativeError> {
        change(context, PreferenceChange::Fullscreen(value))
    }
    #[hks(name = "setResolution")]
    fn set_resolution(
        context: &mut UiVmContext,
        width: u32,
        height: u32,
    ) -> Result<UiEffectHandle, NativeError> {
        change(context, PreferenceChange::Resolution { width, height })
    }
    #[hks(name = "setAutoDialogue")]
    fn set_auto_dialogue(
        context: &mut UiVmContext,
        enabled: bool,
    ) -> Result<UiEffectHandle, NativeError> {
        Ok(context.insert_effect(UiEffect::SetAutoDialogue(enabled)))
    }

    #[hks(name = "setFastForward")]
    fn set_fast_forward(
        context: &mut UiVmContext,
        enabled: bool,
    ) -> Result<UiEffectHandle, NativeError> {
        Ok(context.insert_effect(UiEffect::SetFastForward(enabled)))
    }
}

#[hiraku_script::hks_module("audio")]
mod settings_actions {
    use super::*;

    #[hks(name = "stopVoice")]
    fn stop_voice(context: &mut UiVmContext) -> Result<UiEffectHandle, NativeError> {
        Ok(context.insert_effect(UiEffect::StopVoice))
    }

    fn current_volume(context: &UiVmContext, channel: &str) -> f64 {
        let settings = context.values.preferences();
        match channel {
            "bgmVolume" => settings.bgm_volume.into(),
            "voiceVolume" => settings.voice_volume.into(),
            "sfxVolume" => settings.sfx_volume.into(),
            _ => 1.0,
        }
    }

    #[hks(name = "bgmVolume")]
    fn bgm_volume(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(current_volume(context, "bgmVolume"))
    }

    #[hks(name = "voiceVolume")]
    fn voice_volume(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(current_volume(context, "voiceVolume"))
    }

    #[hks(name = "sfxVolume")]
    fn sfx_volume(context: &mut UiVmContext) -> Result<f64, NativeError> {
        Ok(current_volume(context, "sfxVolume"))
    }

    fn volume(
        context: &mut UiVmContext,
        channel: &str,
        value: f64,
    ) -> Result<UiEffectHandle, NativeError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(NativeError::message(
                "volume must be finite and between 0 and 1",
            ));
        }
        Ok(context.insert_effect(UiEffect::SetVolume {
            channel: channel.into(),
            value: value as f32,
        }))
    }

    #[hks(name = "setBgmVolume")]
    fn bgm(context: &mut UiVmContext, value: f64) -> Result<UiEffectHandle, NativeError> {
        volume(context, "bgmVolume", value)
    }

    #[hks(name = "setVoiceVolume")]
    fn voice(context: &mut UiVmContext, value: f64) -> Result<UiEffectHandle, NativeError> {
        volume(context, "voiceVolume", value)
    }

    #[hks(name = "setSfxVolume")]
    fn sfx(context: &mut UiVmContext, value: f64) -> Result<UiEffectHandle, NativeError> {
        volume(context, "sfxVolume", value)
    }
}

#[hiraku_script::hks_module("story")]
mod story_actions {
    use super::*;

    #[hks]
    fn native_next(context: &mut UiVmContext) -> Result<UiEffectHandle, NativeError> {
        Ok(context.insert_effect(UiEffect::NextDialogue))
    }

    #[hks(name = "goto")]
    fn native_goto_story(
        context: &mut UiVmContext,
        _path: String,
        _options: Option<NavigationOptions>,
    ) -> Result<hiraku_script::native::Never, NativeError> {
        let _ = context;
        Err(NativeError::message(
            "story.goto is only available in an onClick execution",
        ))
    }
}

fn animation_seconds(value: Option<f64>, default: f64) -> Result<f64, NativeError> {
    let value = value.unwrap_or(default);
    if !value.is_finite() || value <= 0.0 {
        return Err(NativeError::message(
            "animation duration must be greater than zero",
        ));
    }
    Ok(value)
}

fn validate_phase_animation(
    phases: &[AnimationPhase],
    animation: AnimationSpec,
) -> Result<(), NativeError> {
    if phases.len() < 2 {
        return Err(NativeError::message(
            "phaseAnimator requires at least two phases",
        ));
    }
    animation_seconds(Some(animation.duration() as f64), 1.0)?;
    if phases.iter().any(|phase| {
        let (rotation, scale, x, y) = phase.values();
        !rotation.is_finite()
            || !scale.is_finite()
            || scale < 0.0
            || !x.is_finite()
            || !y.is_finite()
    }) {
        return Err(NativeError::message(
            "animation phases require finite rotation/offset and non-negative finite scale",
        ));
    }
    Ok(())
}

fn finite_f32(value: f64, label: &str) -> Result<f32, NativeError> {
    value
        .is_finite()
        .then_some(value as f32)
        .ok_or_else(|| NativeError::message(format!("{label} must be finite")))
}

fn non_negative(value: f64, label: &str) -> Result<f32, NativeError> {
    if !value.is_finite() || value < 0.0 {
        return Err(NativeError::message(format!(
            "{label} must be a non-negative number"
        )));
    }
    Ok(value as f32)
}

fn positive(value: f64, label: &str) -> Result<f32, NativeError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(NativeError::message(format!(
            "{label} must be greater than zero"
        )));
    }
    Ok(value as f32)
}

fn percent(value: f64, label: &str) -> Result<f32, NativeError> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(NativeError::message(format!(
            "{label} must be between 0 and 100"
        )));
    }
    Ok(value as f32)
}

fn color_component(value: f64) -> Result<f32, NativeError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(NativeError::message(
            "UI color components must be between 0 and 1",
        ));
    }
    Ok(value as f32)
}

fn ui_registry(values: &UiContext) -> NativeRegistry<UiVmContext> {
    let mut registry = NativeRegistry::new();
    crate::ui::ScrollAnchor::register_hks(&mut registry).expect("scroll anchor registration");
    UiPosition::register_hks(&mut registry)
        .expect("UiPosition registration must be internally consistent");
    UiSize::register_hks(&mut registry).expect("UiSize registration must be internally consistent");
    crate::ui::UiKeyframe::register_hks(&mut registry).expect("UI keyframe registration");
    crate::ui::UiShaderKeyframe::register_hks(&mut registry)
        .expect("UI shader keyframe registration");
    register_animation_api(&mut registry)
        .expect("animation API registration must be internally consistent");
    NavigationResetValue::register_hks(&mut registry)
        .expect("navigation reset API registration must be internally consistent");
    // Register the nominal result type before compiling the HKS standard library.
    let ui_node = registry.define_type("UiNode");
    let ui_effect = registry.define_type("UiEffect");
    native_ui::register_hks(&mut registry)
        .expect("UI native primitives must be internally consistent");
    ui_actions::register_hks(&mut registry).expect("UI actions must be internally consistent");
    profile_api::register_hks(&mut registry).expect("profile API must register once");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("ui.open"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String],
                variadic: Some(ScriptType::Any),
                result: ScriptType::Named(ui_effect),
            },
        )
        .expect("UI open primitive is registered");
    for (name, handler) in [
        (
            "complete",
            complete_ui
                as fn(&mut UiVmContext, &hiraku_script::BuiltinCall) -> Result<Value, NativeError>,
        ),
        ("close", close_ui),
    ] {
        let close = registry
            .register_selector_raw_fn("ui", name, handler)
            .expect("UI result primitive is unique");
        registry
            .set_signature(
                close,
                hiraku_script::FunctionSignature {
                    receiver: None,
                    parameters: Vec::new(),
                    variadic: Some(ScriptType::Any),
                    result: ScriptType::Named(ui_effect),
                },
            )
            .expect("UI close signature is registered");
    }
    preference_actions::register_hks(&mut registry).expect("preference API registers once");
    storage_actions::register_hks(&mut registry)
        .expect("storage actions must be internally consistent");
    settings_actions::register_hks(&mut registry)
        .expect("settings actions must be internally consistent");
    story_actions::register_hks(&mut registry)
        .expect("story actions must be internally consistent");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("button"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![
                    ScriptType::Any,
                    ScriptType::Optional(Box::new(ScriptType::Function)),
                ],
                variadic: None,
                result: ScriptType::Named(ui_node),
            },
        )
        .expect("button signature must target its raw native implementation");
    registry
        .define_global(
            "time",
            ScriptType::Record(BTreeMap::from([
                ("elapsedSeconds".to_string(), ScriptType::Float),
                ("unixSeconds".to_string(), ScriptType::Float),
            ])),
        )
        .expect("built-in UI time model must be defined once");
    registry
        .define_global(
            "dialogue",
            ScriptType::Record(BTreeMap::from([
                ("speaker".to_string(), ScriptType::String),
                ("text".to_string(), ScriptType::String),
                ("visible".to_string(), ScriptType::Bool),
                ("revealedCharacters".to_string(), ScriptType::Int),
                ("canAdvance".to_string(), ScriptType::Bool),
                ("autoEnabled".to_string(), ScriptType::Bool),
                ("fastForwardEnabled".to_string(), ScriptType::Bool),
            ])),
        )
        .expect("built-in dialogue model must be defined once");
    for (name, value) in values.story_values() {
        if name == "time" || name == "dialogue" {
            continue;
        }
        registry
            .define_global(name, stored_value_type(value))
            .expect("UI context keys must be unique");
    }
    registry
}

fn stored_value_type(value: &StoredValue) -> ScriptType {
    match value {
        StoredValue::Bool(_) => ScriptType::Bool,
        StoredValue::Int(_) => ScriptType::Int,
        StoredValue::UInt(_) => ScriptType::UInt,
        StoredValue::Float(_) => ScriptType::Float,
        StoredValue::String(_) => ScriptType::String,
        StoredValue::Array(values) => {
            let mut types = values.iter().map(stored_value_type);
            let first = types.next().unwrap_or(ScriptType::Any);
            let element = if types.all(|ty| ty == first) {
                first
            } else {
                ScriptType::Any
            };
            ScriptType::List(Box::new(element))
        }
        StoredValue::Map(values) => ScriptType::Record(
            values
                .iter()
                .map(|(name, value)| (name.clone(), stored_value_type(value)))
                .collect(),
        ),
    }
}

#[derive(Debug, Error)]
pub enum UiVmError {
    #[error("declarative UI runtime failed: {0}")]
    Runtime(String),
    #[error("invalid declarative UI: {0}")]
    Invalid(String),
}

#[cfg(test)]
fn evaluate_ui_component_named(
    path: &str,
    source: &str,
    values: UiContext,
    textures: &TextureCatalog,
    terms: &TermCatalog,
) -> Result<ScreenSpec, UiVmError> {
    evaluate_ui_component_named_with_args(path, source, values, textures, terms, &[])
}

pub fn evaluate_ui_component_named_with_args(
    path: &str,
    source: &str,
    values: UiContext,
    textures: &TextureCatalog,
    terms: &TermCatalog,
    arguments: &[StoredValue],
) -> Result<ScreenSpec, UiVmError> {
    let registry = ui_registry(&values);
    let document = hiraku_ui::UiDocument::compile(
        path,
        ui_sources(path, source),
        &registry.manifest(),
        RenderOptions::terminal(),
    )
    .map_err(|error| UiVmError::Invalid(error.to_string()))?;
    let composition = UiComposition {
        document,
        values,
        path: path.to_owned(),
        arguments: arguments.to_vec(),
        globals: BTreeMap::new(),
    };
    composition.render(&BTreeMap::new(), textures, terms)
}

/// Compiled UI code plus per-mount inputs. Recomposition never recompiles source
/// or imports local state into the story namespace.
#[derive(Clone, Debug)]
pub(crate) struct UiComposition {
    pub(crate) document: hiraku_ui::UiDocument,
    values: UiContext,
    path: String,
    arguments: Vec<StoredValue>,
    pub(crate) globals: BTreeMap<String, Value>,
}

impl UiComposition {
    pub(crate) fn models_changed(&self, models: &crate::ui::UiModels) -> bool {
        self.document.plan.structural_paths.iter().any(|path| {
            let root = path.split('.').next().expect("dependency root");
            // Invocation inputs can exist only in UiContext, not in UiModels.
            !self.document.owned_globals.contains(root)
                && models.get(root).is_some()
                && models.get(path) != self.values.story_value(path)
        })
    }

    pub(crate) fn with_models(&self, models: &crate::ui::UiModels) -> Self {
        let mut next = self.clone();
        next.values.update_models(models);
        next
    }

    pub(crate) fn render(
        &self,
        globals: &BTreeMap<String, Value>,
        textures: &TextureCatalog,
        terms: &TermCatalog,
    ) -> Result<ScreenSpec, UiVmError> {
        let registry = ui_registry(&self.values);
        let materialize_program = self.document.program.clone();
        let mut context =
            UiVmContext::new(self.values.clone(), terms.clone()).with_navigation_origin(&self.path);
        context.owned_globals = self.document.owned_globals.clone();
        context.local_globals = self.globals.clone();
        context.local_globals.extend(
            globals
                .iter()
                .filter(|(name, _)| self.document.owned_globals.contains(*name))
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        if let Some(initializer) = self
            .document
            .initializer()
            .map_err(|error| UiVmError::Runtime(error.to_string()))?
        {
            if !collect_nodes(initializer, &registry, &mut context)?.is_empty() {
                return Err(UiVmError::Invalid(
                    "an @ui document must emit nodes inside its entrypoint, not at module scope"
                        .into(),
                ));
            }
        }
        let vm = self
            .document
            .invocation(self.arguments.iter().map(stored_to_hks).collect())
            .map_err(|error| UiVmError::Runtime(error.to_string()))?;
        let roots = collect_nodes(vm, &registry, &mut context)?;
        if roots.len() != 1 {
            return Err(UiVmError::Invalid(format!(
                "a UI document must produce exactly one root node, got {}",
                roots.len()
            )));
        }
        let mut screen = materialize_screen(
            roots[0],
            &materialize_program,
            &registry,
            &mut context,
            textures,
        )?;
        let mut next = self.clone();
        next.globals = context.local_globals;
        screen.composition = Some(std::sync::Arc::new(next));
        Ok(screen)
    }
}

fn ui_sources(path: &str, source: &str) -> Vec<hiraku_script::ScriptSource> {
    vec![
        hiraku_script::ScriptSource {
            path: UI_STDLIB_PATH.into(),
            namespace: Some("ui.widgets".into()),
            source: UI_STDLIB_SOURCE.into(),
        },
        hiraku_script::ScriptSource {
            path: path.into(),
            namespace: None,
            source: source.into(),
        },
    ]
}

/// Offline validation uses the same compiler and entrypoint rules as mounting.
pub(crate) fn validate_ui_source(path: &str, source: &str) -> Result<(), String> {
    hiraku_ui::UiDocument::compile(
        path,
        ui_sources(path, source),
        &ui_registry(&UiContext::default()).manifest(),
        RenderOptions::plain(),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn collect_nodes(
    vm: LinkedVm,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let globals = context_globals(context);
    let owned = context.owned_globals.clone();
    let mut nodes = Vec::new();
    let mut committed_node = context.next_node;
    let state = hiraku_ui::compose(
        vm,
        registry,
        context,
        &globals,
        &owned,
        100_000,
        |context| {
            nodes.extend(((committed_node + 1)..=context.next_node).map(UiNodeHandle));
            committed_node = context.next_node;
        },
    )
    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
    context.local_globals.extend(state);
    Ok(nodes)
}

fn closure_children(
    closure: Option<HksClosure>,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let Some(closure) = closure else {
        return Ok(Vec::new());
    };
    let callable = closure.into_hks_value();
    let vm = LinkedVm::from_callable(program.clone(), &callable, Vec::new())
        .map_err(|error| UiVmError::Runtime(error.to_string()))?;
    collect_nodes(vm, registry, context)
}

fn closure_children_with_args(
    callable: HksCallable,
    arguments: Vec<Value>,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let callable = callable.into_value();
    let vm = LinkedVm::from_callable(program.clone(), &callable, arguments)
        .map_err(|error| UiVmError::Runtime(error.to_string()))?;
    collect_nodes(vm, registry, context)
}

#[cfg(test)]
fn evaluate_ui_callback(
    callback: &UiCallback,
    globals: &BTreeMap<String, Value>,
    models: &crate::ui::UiModels,
) -> Result<(Vec<UiEffect>, BTreeMap<String, Value>), UiVmError> {
    evaluate_ui_callback_with_args(callback, globals, models, Vec::new())
}

pub(crate) fn evaluate_ui_callback_with_args(
    callback: &UiCallback,
    globals: &BTreeMap<String, Value>,
    models: &crate::ui::UiModels,
    arguments: Vec<Value>,
) -> Result<(Vec<UiEffect>, BTreeMap<String, Value>), UiVmError> {
    let mut current_globals = callback.globals.clone();
    current_globals.extend(globals.clone());
    for (name, value) in models.roots() {
        current_globals.insert(name.to_owned(), stored_to_hks(value));
    }
    let values = UiContext::default();
    let registry = ui_registry(&values);
    let mut context = UiVmContext::new(values, TermCatalog::default());
    context.navigation_origin = callback.origin.clone();
    let goto = registry.manifest().resolve_selector("story", "goto");
    let mut vm = LinkedVm::from_callable(callback.program.clone(), &callback.callable, arguments)
        .map_err(|error| UiVmError::Runtime(error.to_string()))?;
    vm.set_current_globals(&current_globals)
        .map_err(|error| UiVmError::Runtime(error.to_string()))?;
    vm.set_read_only_globals(
        current_globals
            .keys()
            .filter(|name| !callback.owned_globals.contains(*name))
            .cloned()
            .collect(),
    );
    let mut effects = Vec::new();
    let mut committed_effect = context.next_effect;
    let mut budget = 100_000;
    loop {
        match vm
            .step_with_budget(&mut budget)
            .map_err(|error| UiVmError::Runtime(error.to_string()))?
        {
            Some(LinkedVmEvent::BudgetExhausted) => {
                return Err(UiVmError::Runtime(
                    "UI invocation exceeded its instruction budget".into(),
                ));
            }
            Some(LinkedVmEvent::Call(call)) => {
                if Some(call.builtin) == goto {
                    let request = NavigationRequest::from_goto_call(&call)
                        .map_err(|error| UiVmError::Runtime(error.to_string()))?
                        .with_origin(context.navigation_origin.clone());
                    effects.push(UiEffect::Navigate(request));
                    return Ok((
                        effects,
                        vm.current_globals()
                            .map_err(|error| UiVmError::Runtime(error.to_string()))?,
                    ));
                }
                let value = registry
                    .call(&mut context, &call)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
                vm.resume(value)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
            }
            Some(LinkedVmEvent::Statement(StatementValue::Value(_) | StatementValue::Commit)) => {
                effects.extend(
                    context
                        .effects
                        .range((committed_effect + 1)..)
                        .map(|(_, effect)| effect.clone()),
                );
                committed_effect = context.next_effect;
            }
            Some(LinkedVmEvent::Statement(
                StatementValue::String(_) | StatementValue::TextTemplate(_),
            )) => {
                return Err(UiVmError::Invalid(
                    "bare strings are not valid onClick effects".into(),
                ));
            }
            Some(LinkedVmEvent::Completed(_)) => {
                effects.extend(
                    context
                        .effects
                        .range((committed_effect + 1)..)
                        .map(|(_, effect)| effect.clone()),
                );
                return Ok((
                    effects,
                    vm.current_globals()
                        .map_err(|error| UiVmError::Runtime(error.to_string()))?,
                ));
            }
            None => {
                return Err(UiVmError::Runtime(
                    "onClick handler stopped without completing".into(),
                ));
            }
        }
    }
}

fn context_globals(context: &UiVmContext) -> BTreeMap<String, Value> {
    let mut globals = context
        .values
        .story_values()
        .iter()
        .map(|(name, value)| (name.clone(), stored_to_hks(value)))
        .collect::<BTreeMap<_, _>>();
    globals.insert(
        "time".to_string(),
        Value::Map(BTreeMap::from([
            ("elapsedSeconds".to_string(), Value::Int(0)),
            ("unixSeconds".to_string(), Value::Int(0)),
        ])),
    );
    globals.extend(context.local_globals.clone());
    globals
}

fn stored_to_hks(value: &StoredValue) -> Value {
    match value {
        StoredValue::Bool(value) => Value::Bool(*value),
        StoredValue::Int(value) => Value::Int(*value),
        StoredValue::UInt(value) => Value::UInt(*value),
        StoredValue::Float(value) => Value::Number(*value),
        StoredValue::String(value) => Value::String(value.clone()),
        StoredValue::Array(values) => Value::List(values.iter().map(stored_to_hks).collect()),
        StoredValue::Map(values) => Value::Map(
            values
                .iter()
                .map(|(name, value)| (name.clone(), stored_to_hks(value)))
                .collect(),
        ),
    }
}

fn reactive_binding<T>(
    binding: &HksBinding<T>,
    program: &hiraku_script::LinkedProgram,
    context: &UiVmContext,
) -> PropertyComputation {
    PropertyComputation::new(
        program.clone(),
        binding.getter().value().clone(),
        context_globals(context),
    )
}

fn evaluate_binding_value(
    binding: &PropertyComputation,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Value, UiVmError> {
    binding
        .evaluate(registry, context, 100_000)
        .map_err(|error| UiVmError::Runtime(error.to_string()))
}
pub(crate) fn refresh_ui_property_models(
    binding: &mut PropertyComputation,
    models: &crate::ui::UiModels,
) -> bool {
    let mut changed = false;
    for name in &binding.dependencies {
        if let Some(value) = models.get(name) {
            let next = stored_to_hks(value);
            if binding.globals.get(name) != Some(&next) {
                binding.globals.insert(name.clone(), next);
                changed = true;
            }
        }
    }
    changed
}

/// Per-system registry: immutable native definitions are reused, while each
/// evaluation keeps its own VM/context and cannot leak state to another UI.
pub(crate) struct UiPropertyEvaluator {
    registry: NativeRegistry<UiVmContext>,
}

impl Default for UiPropertyEvaluator {
    fn default() -> Self {
        Self {
            registry: ui_registry(&UiContext::default()),
        }
    }
}

impl UiPropertyEvaluator {
    pub(crate) fn evaluate(
        &self,
        binding: &PropertyComputation,
        models: &crate::ui::UiModels,
    ) -> Result<Value, UiVmError> {
        let mut binding = binding.clone();
        for name in &binding.dependencies {
            if let Some(value) = models.get(name) {
                binding.globals.insert(name.clone(), stored_to_hks(value));
            }
        }
        let values = UiContext::default();
        let mut context = UiVmContext::new(values, TermCatalog::default());
        evaluate_binding_value(&binding, &self.registry, &mut context)
    }
}

#[cfg(test)]
pub(crate) fn evaluate_ui_reactive_binding(
    binding: &PropertyComputation,
    models: &crate::ui::UiModels,
) -> Result<Value, UiVmError> {
    UiPropertyEvaluator::default().evaluate(binding, models)
}

fn materialize_screen(
    root: UiNodeHandle,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
    textures: &TextureCatalog,
) -> Result<ScreenSpec, UiVmError> {
    let draft = context
        .nodes
        .get(&root.0)
        .cloned()
        .ok_or_else(|| UiVmError::Invalid("UI root handle no longer exists".into()))?;
    if !matches!(draft.kind, UiDraftKind::Screen) {
        return Err(UiVmError::Invalid(
            "a UI document root must be screen { ... } or canvas { ... }".into(),
        ));
    }
    let child_handles = closure_children(draft.content, program, registry, context)?;
    let children = child_handles
        .into_iter()
        .map(|child| materialize_node(child, program, registry, context, textures))
        .collect::<Result<Vec<_>, _>>()?;
    let background_texture = draft
        .background_texture
        .as_deref()
        .map(|name| resolve_texture(textures, name))
        .transpose()?;
    Ok(ScreenSpec {
        allowed_overlays: draft.allowed_overlays,
        timers_paused: draft.timers_paused,
        fade_seconds: draft.fade_seconds,
        pauses_scene: draft.pauses_scene,
        timers: draft
            .timers
            .into_iter()
            .map(|(seconds, handler)| (seconds, ui_callback(handler, program, context)))
            .collect(),
        composition: None,
        title: None,
        panel: draft.panel,
        width: draft.layout.width,
        background_texture,
        xalign: 0.5,
        yalign: 0.5,
        padding: 24.0,
        gap: draft.gap,
        overlay: draft.overlay,
        background: None,
        border: None,
        children,
    })
}

fn disable_choice_buttons(node: &mut ScreenNode) {
    match node {
        ScreenNode::Button(button) => {
            button.enabled = false;
            button.enabled_binding = None;
            button.reactive_enabled = None;
        }
        ScreenNode::ImageButton(button) => {
            button.enabled = false;
            button.enabled_binding = None;
            button.reactive_enabled = None;
        }
        ScreenNode::Column(container) | ScreenNode::Row(container) => {
            for child in &mut container.children {
                disable_choice_buttons(child);
            }
        }
        ScreenNode::Scrollable(container) => {
            for child in &mut container.children {
                disable_choice_buttons(child);
            }
        }
        _ => {}
    }
}

fn ui_callback(
    handler: HksCallable,
    program: &hiraku_script::LinkedProgram,
    context: &UiVmContext,
) -> UiCallback {
    UiCallback {
        owned_globals: context.owned_globals.clone(),
        program: program.clone(),
        callable: handler.into_value(),
        globals: context_globals(context),
        origin: context.navigation_origin.clone(),
    }
}

fn input_value<T: IntoHksValue + hiraku_script::native::HksScriptType>(
    value: HksBindable<T>,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<(Value, Option<PropertyComputation>), UiVmError> {
    match value {
        HksBindable::Value(value) => Ok((value.into_hks_value(), None)),
        HksBindable::Binding(binding) => {
            let reactive = reactive_binding(&binding, program, context);
            let value = evaluate_binding_value(&reactive, registry, context)?;
            Ok((value, Some(reactive)))
        }
    }
}

fn input_node(
    draft: &UiDraft,
    kind: crate::ui::InputKind,
    value: Value,
    reactive_value: Option<PropertyComputation>,
    program: &hiraku_script::LinkedProgram,
    context: &UiVmContext,
) -> Result<ScreenNode, UiVmError> {
    let expected = match kind {
        crate::ui::InputKind::Checkbox => ScriptType::Bool,
        crate::ui::InputKind::Slider { .. } => ScriptType::Float,
        crate::ui::InputKind::TextInput { .. } => ScriptType::String,
    };
    for handler in [&draft.on_change, &draft.on_commit].into_iter().flatten() {
        validate_input_handler(handler, &expected, program)?;
    }
    let value = match (&kind, value) {
        (crate::ui::InputKind::Checkbox, Value::Bool(value)) => StoredValue::Bool(value),
        (crate::ui::InputKind::Slider { min, max }, Value::Number(value)) if value.is_finite() => {
            StoredValue::Float(value.clamp(*min, *max))
        }
        (crate::ui::InputKind::TextInput { .. }, Value::String(value)) => {
            StoredValue::String(value)
        }
        _ => {
            return Err(UiVmError::Invalid(format!(
                "input value requires {expected:?}; numeric values must be finite"
            )));
        }
    };
    Ok(ScreenNode::Input(crate::ui::InputNode {
        slider_skin: None,
        kind,
        value,
        enabled: draft.enabled,
        reactive_enabled: draft
            .enabled_binding
            .as_ref()
            .map(|binding| reactive_binding(binding, program, context)),
        layout: draft.layout.clone(),
        text_size: draft.text_size,
        text_color: draft.text_color,
        background: draft.surface,
        reactive_value,
        on_change: draft
            .on_change
            .clone()
            .map(|handler| ui_callback(handler, program, context)),
        on_commit: draft
            .on_commit
            .clone()
            .map(|handler| ui_callback(handler, program, context)),
    }))
}

fn validate_input_handler(
    handler: &HksCallable,
    expected: &ScriptType,
    program: &hiraku_script::LinkedProgram,
) -> Result<(), UiVmError> {
    let signature = callable_signature(handler, program);
    if signature.is_some_and(|signature| {
        signature.parameters == [expected.clone()]
            && matches!(signature.result, ScriptType::Unit | ScriptType::Never)
    }) {
        return Ok(());
    }
    Err(UiVmError::Invalid(format!(
        "input handler requires ({expected:?}) -> Unit; annotate the callback parameter, for example {{ value: {expected:?} -> ... }}"
    )))
}

fn callable_signature<'a>(
    handler: &HksCallable,
    program: &'a hiraku_script::LinkedProgram,
) -> Option<&'a hiraku_script::FunctionSignature> {
    match handler.value() {
        Value::Closure {
            module: Some(module),
            region,
            ..
        } => program
            .modules
            .get(*module as usize)
            .and_then(|module| module.bytecode.regions.get(*region as usize))
            .map(|region| &region.signature),
        Value::Function {
            module: Some(module),
            symbol,
        } => program
            .modules
            .get(*module as usize)
            .and_then(|module| {
                module
                    .bytecode
                    .functions
                    .iter()
                    .find(|function| function.name == *symbol)
            })
            .map(|function| &function.signature),
        _ => None,
    }
}

fn materialize_node(
    handle: UiNodeHandle,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
    textures: &TextureCatalog,
) -> Result<ScreenNode, UiVmError> {
    let mut draft = context
        .nodes
        .get(&handle.0)
        .cloned()
        .ok_or_else(|| UiVmError::Invalid(format!("unknown UiNode handle {}", handle.0)))?;
    draft.layout.hidden = !draft.visible;
    draft.layout.animation = draft.animation;
    draft.layout.phase_animation = draft.phase_animation.clone();
    if draft.layout.hover_offset.is_some() && draft.phase_animation.is_some() {
        return Err(UiVmError::Invalid(
            "hoverOffset and a phase animation require separate nested nodes".into(),
        ));
    }
    if draft.layout.hover_offset.is_some()
        && draft.animation.is_some_and(|animation| animation.repeats())
    {
        return Err(UiVmError::Invalid(
            "hoverOffset requires a non-repeating transition".into(),
        ));
    }
    if let Some(binding) = &draft.hover_active {
        let reactive = reactive_binding(binding, program, context);
        let value = evaluate_binding_value(&reactive, registry, context)?;
        draft.layout.hover_active =
            bool::from_hks_value(&value).map_err(|error| UiVmError::Invalid(error.to_string()))?;
        draft.layout.reactive_hover_active = Some(reactive);
    }
    if let Some(binding) = &draft.text_reveal {
        let reactive = reactive_binding(binding, program, context);
        let value = evaluate_binding_value(&reactive, registry, context)?;
        draft.layout.text_reveal = Some(
            u32::try_from(
                i64::from_hks_value(&value)
                    .map_err(|error| UiVmError::Invalid(error.to_string()))?,
            )
            .map_err(|_| {
                UiVmError::Invalid("reveal count must fit a nonnegative 32-bit integer".into())
            })?,
        );
        draft.layout.reactive_text_reveal = Some(reactive);
    }
    draft.layout.visible_binding = None;
    if let Some(binding) = &draft.enabled_binding {
        let reactive = reactive_binding(binding, program, context);
        let value = evaluate_binding_value(&reactive, registry, context)?;
        draft.enabled =
            bool::from_hks_value(&value).map_err(|error| UiVmError::Invalid(error.to_string()))?;
    }
    if let Some(binding) = &draft.visible_binding {
        let reactive = reactive_binding(binding, program, context);
        let value = evaluate_binding_value(&reactive, registry, context)?;
        draft.layout.hidden =
            !bool::from_hks_value(&value).map_err(|error| UiVmError::Invalid(error.to_string()))?;
        draft.layout.reactive_visibility = Some(reactive);
    }
    match draft.kind {
        UiDraftKind::Checkbox(ref value) => {
            let (value, reactive_value) = input_value(value.clone(), program, registry, context)?;
            input_node(
                &draft,
                crate::ui::InputKind::Checkbox,
                value,
                reactive_value,
                program,
                context,
            )
        }
        UiDraftKind::Slider(ref value, min, max) => {
            let (value, reactive_value) = input_value(value.clone(), program, registry, context)?;
            let mut node = input_node(
                &draft,
                crate::ui::InputKind::Slider { min, max },
                value,
                reactive_value,
                program,
                context,
            )?;
            if let ScreenNode::Input(input) = &mut node {
                input.slider_skin = draft
                    .slider_skin
                    .as_ref()
                    .map(|names| {
                        Ok::<_, UiVmError>([
                            resolve_texture(textures, &names[0])?,
                            resolve_texture(textures, &names[1])?,
                            resolve_texture(textures, &names[2])?,
                        ])
                    })
                    .transpose()?;
            }
            Ok(node)
        }
        UiDraftKind::TextInput(ref value) => {
            let (value, reactive_value) = input_value(value.clone(), program, registry, context)?;
            input_node(
                &draft,
                crate::ui::InputKind::TextInput {
                    placeholder: draft.placeholder.clone(),
                },
                value,
                reactive_value,
                program,
                context,
            )
        }
        UiDraftKind::Screen => Err(UiVmError::Invalid(
            "screen nodes may only appear at the document root".into(),
        )),
        UiDraftKind::Image(path) => {
            let mut layout = draft.layout;
            if let Some(shader) = &mut layout.shader {
                if layout.keyframes.is_empty() {
                    return Err(UiVmError::Invalid(
                        "shader images require a keyframe timeline".into(),
                    ));
                }
                for key in &mut shader.textures {
                    let texture = resolve_texture(textures, key)?;
                    if texture.rect.is_some() {
                        return Err(UiVmError::Invalid(
                            "shader auxiliary textures must be whole textures".into(),
                        ));
                    }
                    *key = texture.path;
                }
            }
            let texture = resolve_texture(textures, &path)?;
            if (layout.shader.is_some()
                || layout.keyframes.iter().any(|key| {
                    matches!(
                        key,
                        crate::ui::UiKeyframe::Quad(..) | crate::ui::UiKeyframe::QuadStepAlpha(..)
                    )
                }))
                && (texture.rect.is_some() || layout.flip_x)
            {
                return Err(UiVmError::Invalid("projected shader tracks require whole, unflipped images; encode reflection in quad axes".into()));
            }
            Ok(ScreenNode::Image(ScreenImageNode { texture, layout }))
        }
        UiDraftKind::Text(binding) => {
            let (text, reactive) = match binding {
                HksBindable::Value(value) => (value, None),
                HksBindable::Binding(binding) => {
                    let reactive = reactive_binding(&binding, program, context);
                    let value = evaluate_binding_value(&reactive, registry, context)?;
                    let value = String::from_hks_value(&value)
                        .map_err(|error| UiVmError::Invalid(error.to_string()))?;
                    (value, Some(reactive))
                }
            };
            let is_template = reactive.is_none() && text.contains("${");
            let text = if is_template {
                context
                    .values
                    .expand_binding(&text)
                    .map_err(|error| UiVmError::Invalid(error.to_string()))?
            } else {
                text
            };
            if draft.layout.rich_text {
                crate::rich_text::parse(&text).map_err(UiVmError::Invalid)?;
            }
            Ok(ScreenNode::Text(TextNode {
                binding: is_template.then(|| text.clone()),
                reactive_text: if is_template { None } else { reactive },
                text,
                size: draft.text_size.unwrap_or(28.0),
                color: draft.text_color,
                align: draft.text_align,
                layout: draft.layout,
            }))
        }
        UiDraftKind::Term(term) => {
            let definition = context
                .terms
                .get(term)
                .ok_or_else(|| UiVmError::Invalid("interned term no longer exists".into()))?;
            Ok(ScreenNode::Text(TextNode {
                text: definition.name.clone(),
                binding: None,
                reactive_text: None,
                size: 28.0,
                color: None,
                align: None,
                layout: draft.layout,
            }))
        }
        UiDraftKind::Spacer => Ok(ScreenNode::Spacer(SpacerNode {
            width: draft.layout.width.unwrap_or(0.0),
            height: draft.layout.height.unwrap_or(0.0),
            layout: draft.layout,
        })),
        UiDraftKind::Progress { value, min, max } => {
            let (value, reactive) = match value {
                HksBindable::Value(value) => (value, None),
                HksBindable::Binding(binding) => {
                    let reactive = reactive_binding(&binding, program, context);
                    let value = evaluate_binding_value(&reactive, registry, context)?;
                    let value = f64::from_hks_value(&value)
                        .map_err(|error| UiVmError::Invalid(error.to_string()))?;
                    (value, Some(reactive))
                }
            };
            Ok(ScreenNode::Bar(BarNode {
                value: finite_f32(value, "progress value")
                    .map_err(|error| UiVmError::Invalid(error.to_string()))?,
                binding: None,
                reactive_value: reactive,
                min,
                max,
                width: draft.layout.width.unwrap_or(320.0),
                height: draft.layout.height.unwrap_or(18.0),
                background: None,
                fill: None,
                border: None,
                layout: draft.layout,
            }))
        }
        UiDraftKind::Scrollable => {
            let child_handles = closure_children(draft.content, program, registry, context)?;
            let children = child_handles
                .into_iter()
                .map(|child| materialize_node(child, program, registry, context, textures))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ScreenNode::Scrollable(ScrollableNode {
                children,
                speed: draft.scroll_speed,
                default_scroll_anchor: draft.default_scroll_anchor,
                layout: draft.layout,
            }))
        }
        UiDraftKind::Toggle(value) => {
            if let Some(handler) = &draft.on_change {
                validate_input_handler(handler, &ScriptType::Bool, program)?;
            }
            let (value, reactive_value) = input_value(value, program, registry, context)?;
            let value = bool::from_hks_value(&value)
                .map_err(|error| UiVmError::Invalid(error.to_string()))?;
            let normal_handles = closure_children(draft.content, program, registry, context)?;
            let checked_handles = closure_children(draft.checked, program, registry, context)?;
            let [normal_handle] = normal_handles.as_slice() else {
                return Err(UiVmError::Invalid(
                    "toggle content must produce exactly one image node".into(),
                ));
            };
            let normal = materialize_node(*normal_handle, program, registry, context, textures)?;
            let ScreenNode::Image(unchecked) = normal else {
                return Err(UiVmError::Invalid(
                    "toggle content must produce image(...)".into(),
                ));
            };
            let checked = match checked_handles.as_slice() {
                [] => unchecked.clone(),
                [handle] => {
                    let checked = materialize_node(*handle, program, registry, context, textures)?;
                    let ScreenNode::Image(checked) = checked else {
                        return Err(UiVmError::Invalid(
                            "checked content must produce image(...)".into(),
                        ));
                    };
                    checked
                }
                _ => {
                    return Err(UiVmError::Invalid(
                        "checked content must produce at most one image node".into(),
                    ));
                }
            };
            Ok(ScreenNode::Toggle(ToggleNode {
                unchecked,
                checked,
                value,
                reactive_value,
                on_change: draft
                    .on_change
                    .map(|handler| ui_callback(handler, program, context)),
            }))
        }
        UiDraftKind::ChoiceOptions(renderer) => {
            let arity =
                callable_signature(&renderer, program).map(|signature| signature.parameters.len());
            if !matches!(arity, Some(2 | 3)) {
                return Err(UiVmError::Invalid("choiceOptions requires (index: Int, label: String) or (index: Int, label: String, parameters: T)".into()));
            }
            let parameters = context
                .values
                .story_values()
                .get("choice")
                .and_then(|choice| match choice {
                    StoredValue::Map(fields) => fields.get("parameters"),
                    _ => None,
                })
                .and_then(|value| match value {
                    StoredValue::Map(values) => Some(values.clone()),
                    _ => None,
                });
            let enabled = context
                .values
                .story_values()
                .get("choice")
                .and_then(|choice| match choice {
                    StoredValue::Map(fields) => fields.get("enabled"),
                    _ => None,
                })
                .and_then(|value| match value {
                    StoredValue::Array(values) => Some(values.clone()),
                    _ => None,
                });
            let options = context
                .values
                .story_values()
                .get("choice")
                .and_then(|choice| match choice {
                    StoredValue::Map(fields) => fields.get("options"),
                    _ => None,
                })
                .and_then(|options| match options {
                    StoredValue::Array(options) => Some(options.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    UiVmError::Invalid(
                        "choiceOptions requires the engine-owned choice.options model".into(),
                    )
                })?;
            let mut children = Vec::with_capacity(options.len());
            for (index, option) in options.into_iter().enumerate() {
                let StoredValue::String(label) = option else {
                    return Err(UiVmError::Invalid(
                        "choice.options entries must be strings".into(),
                    ));
                };
                let mut arguments = vec![Value::Int(index as i64), Value::String(label)];
                if arity == Some(3) {
                    let value = parameters.as_ref().and_then(|values| values.get(&index.to_string()))
                        .ok_or_else(|| UiVmError::Invalid(format!("choice option {index} is missing .params(value), required by its three-parameter UI renderer")))?;
                    arguments.push(stored_to_hks(value));
                }
                let rendered = closure_children_with_args(
                    renderer.clone(),
                    arguments,
                    program,
                    registry,
                    context,
                )?;
                if rendered.len() != 1 {
                    return Err(UiVmError::Invalid(
                        "choice option renderer must return exactly one UiNode".into(),
                    ));
                }
                let mut node = materialize_node(rendered[0], program, registry, context, textures)?;
                if enabled.as_ref().and_then(|values| values.get(index))
                    == Some(&StoredValue::Bool(false))
                {
                    disable_choice_buttons(&mut node);
                }
                children.push(node);
            }
            Ok(ScreenNode::Column(ContainerNode {
                gap: draft.gap,
                padding: draft.padding,
                background: draft.surface,
                border: None,
                justify: draft.centered.then(|| "center".into()),
                align_items: Some(if draft.centered { "center" } else { "stretch" }.into()),
                layout: draft.layout,
                children,
            }))
        }
        UiDraftKind::Column | UiDraftKind::Row => {
            let row = matches!(draft.kind, UiDraftKind::Row);
            let child_handles = closure_children(draft.content, program, registry, context)?;
            let children = child_handles
                .into_iter()
                .map(|child| materialize_node(child, program, registry, context, textures))
                .collect::<Result<Vec<_>, _>>()?;
            let container = ContainerNode {
                gap: draft.gap,
                padding: draft.padding,
                background: draft.surface,
                border: None,
                justify: draft.centered.then(|| "center".into()),
                align_items: draft.centered.then(|| "center".into()),
                layout: draft.layout,
                children,
            };
            Ok(if row {
                ScreenNode::Row(container)
            } else {
                ScreenNode::Column(container)
            })
        }
        UiDraftKind::Button(value) => {
            let on_click = draft.on_click.map(|closure| UiCallback {
                owned_globals: context.owned_globals.clone(),
                program: program.clone(),
                callable: closure.into_hks_value(),
                globals: context_globals(context),
                origin: context.navigation_origin.clone(),
            });
            let (enabled, reactive_enabled) = if let Some(binding) = &draft.enabled_binding {
                let reactive = reactive_binding(binding, program, context);
                let value = evaluate_binding_value(&reactive, registry, context)?;
                let enabled = bool::from_hks_value(&value)
                    .map_err(|error| UiVmError::Invalid(error.to_string()))?;
                (enabled, Some(reactive))
            } else {
                (draft.enabled, None)
            };
            let normal_handles = closure_children(draft.content, program, registry, context)?;
            if normal_handles.len() != 1 {
                return Err(UiVmError::Invalid(format!(
                    "button content must produce exactly one text, image, row, column, or spacer node, got {}",
                    normal_handles.len()
                )));
            }
            let normal = materialize_node(normal_handles[0], program, registry, context, textures)?;
            if draft.pressed.is_some() && !matches!(&normal, ScreenNode::Image(_)) {
                return Err(UiVmError::Invalid(
                    "pressed content is only valid on image buttons".into(),
                ));
            }
            let value = if matches!(&value, Value::Unit) {
                None
            } else {
                Some(stored_value(value)?)
            };
            match normal {
                normal @ (ScreenNode::Text(_) | ScreenNode::Column(_) | ScreenNode::Row(_) | ScreenNode::Spacer(_)) => {
                    if draft.hovered.is_some() {
                        return Err(UiVmError::Invalid(
                            "hovered artwork requires an image button".into(),
                        ));
                    }
                    let (text, size, align, children, text_color, text_shadow) = match normal {
                        ScreenNode::Text(text) => (text.text, text.size, text.align, Vec::new(), text.color, text.layout.text_shadow),
                        content => (String::new(), 16.0, None, vec![content], None, None),
                    };
                    let custom_content = !children.is_empty();
                    let mut layout = draft.layout;
                    if layout.text_shadow.is_none() { layout.text_shadow = text_shadow; }
                    Ok(ScreenNode::Button(ButtonNode {
                        hovered_when_disabled: draft.hovered_when_disabled,
                        children,
                        text,
                        value,
                        on_click,
                        enabled,
                        enabled_binding: None,
                        reactive_enabled,
                        size,
                        color: text_color,
                        hovered_color: None,
                        pressed_color: None,
                        insensitive_color: None,
                        background: draft.surface.or(custom_content.then_some([0.0; 4])),
                        border: custom_content.then_some([0.0; 4]),
                        hovered_background: draft.hovered_surface.or(draft.surface).or(custom_content.then_some([0.0; 4])),
                        pressed_background: draft.hovered_surface.or(draft.surface).or(custom_content.then_some([0.0; 4])),
                        background_texture: draft
                            .button_background_texture
                            .as_deref()
                            .map(|name| resolve_texture(textures, name))
                            .transpose()?,
                        hovered_background_texture: draft
                            .button_hovered_background_texture
                            .as_deref()
                            .map(|name| resolve_texture(textures, name))
                            .transpose()?,
                        hover_scale: draft.hover_scale,
                        press_scale: draft.press_scale,
                        align,
                        padding_x: custom_content.then_some(0.0),
                        padding_y: custom_content.then_some(0.0),
                        border_width: custom_content.then_some(0.0),
                        radius: custom_content.then_some(0.0),
                        layout,
                    }))
                }
                ScreenNode::Image(image) => {
                    let pressed = if let Some(content) = draft.pressed {
                        let handles = closure_children(Some(content), program, registry, context)?;
                        let [handle] = handles.as_slice() else {
                            return Err(UiVmError::Invalid(
                                "pressed content must produce exactly one image node".into(),
                            ));
                        };
                        let ScreenNode::Image(image) =
                            materialize_node(*handle, program, registry, context, textures)?
                        else {
                            return Err(UiVmError::Invalid(
                                "pressed content must produce image(...)".into(),
                            ));
                        };
                        Some(image)
                    } else {
                        None
                    };
                    let hovered = closure_children(draft.hovered, program, registry, context)?;
                    let hovered = match hovered.as_slice() {
                        [] => None,
                        [handle] => Some(materialize_node(
                            *handle, program, registry, context, textures,
                        )?),
                        _ => {
                            return Err(UiVmError::Invalid(
                                "hovered content must produce at most one image node".into(),
                            ));
                        }
                    };
                    let (hovered_texture, hovered_layout) = match hovered {
                        Some(ScreenNode::Image(image)) => (Some(image.texture), Some(image.layout)),
                        Some(_) => {
                            return Err(UiVmError::Invalid(
                                "an image button's hovered state must produce image(...)".into(),
                            ));
                        }
                        None => (None, None),
                    };
                    Ok(ScreenNode::ImageButton(ScreenImageButtonNode {
                        pressed_texture: pressed.as_ref().map(|image| image.texture.clone()),
                        pressed_layout: pressed.map(|image| image_button_layout(image.layout, &draft.layout)),
                        texture: image.texture,
                        hovered_texture,
                        hovered_layout: hovered_layout.map(|layout| image_button_layout(layout, &draft.layout)),
                        hover_scale: draft.hover_scale,
                        press_scale: draft.press_scale,
                        value,
                        on_click,
                        enabled,
                        enabled_binding: None,
                        reactive_enabled,
                        hovered_when_disabled: draft.hovered_when_disabled,
                        layout: image_button_layout(image.layout, &draft.layout),
                    }))
                }
                _ => Err(UiVmError::Invalid(
                    "button content must be text(...), image(...), row {...}, column {...}, or spacer(...)".into(),
                )),
            }
        }
    }
}

/// The compact image-button representation must retain the outer button's
/// placement. Explicit button dimensions/insets override the image defaults.
fn image_button_layout(mut image: ScreenLayout, button: &ScreenLayout) -> ScreenLayout {
    if button.fit_content {
        image.fit_content = true;
        image.width = None;
        image.width_percent = None;
        image.height = None;
        image.height_percent = None;
    }
    macro_rules! dimension {
        ($px:ident, $percent:ident) => {
            if button.$px.is_some() || button.$percent.is_some() {
                image.$px = button.$px;
                image.$percent = button.$percent;
            }
        };
    }
    dimension!(width, width_percent);
    dimension!(height, height_percent);
    dimension!(left, left_percent);
    dimension!(right, right_percent);
    dimension!(top, top_percent);
    dimension!(bottom, bottom_percent);
    image.min_width = button.min_width.or(image.min_width);
    image.hover_brightness = button.hover_brightness.or(image.hover_brightness);
    image.hidden |= button.hidden;
    image.clip |= button.clip;
    image
}

fn stored_value(value: Value) -> Result<StoredValue, UiVmError> {
    match value {
        Value::Bool(value) => Ok(StoredValue::Bool(value)),
        Value::Int(value) => Ok(StoredValue::Int(value)),
        Value::UInt(value) => Ok(StoredValue::UInt(value)),
        Value::Number(value) => Ok(StoredValue::Float(value)),
        Value::String(value) | Value::Symbol(value) => Ok(StoredValue::String(value)),
        Value::List(values) | Value::Tuple(values) => values
            .into_iter()
            .map(stored_value)
            .collect::<Result<Vec<_>, _>>()
            .map(StoredValue::Array),
        Value::Map(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key, stored_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(StoredValue::Map),
        _ => Err(UiVmError::Invalid(
            "button values must be persistable HKS values".into(),
        )),
    }
}

fn resolve_texture(textures: &TextureCatalog, name: &str) -> Result<ScreenTexture, UiVmError> {
    if let Some(slot) = name.strip_prefix("save-thumbnail://") {
        hiraku_storage::validate_key(slot).map_err(|e| UiVmError::Invalid(e.to_string()))?;
        return Ok(ScreenTexture {
            path: name.into(),
            rect: None,
        });
    }
    let texture = textures
        .resolve(name)
        .ok_or_else(|| UiVmError::Invalid(format!("texture `{name}` is not defined")))?;
    Ok(ScreenTexture {
        path: texture.path.clone(),
        rect: texture.rect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_button_preserves_outer_placement_in_all_visual_states() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://alice.ui.hks",
            r#"import ui.widgets.*
            canvas {
                button { image("save-thumbnail://alice").size(.rel(10, 10)) }
                    .hovered { image("save-thumbnail://bob") }
                    .pressed { image("save-thumbnail://alice") }
                    .at(.abs(2260, 0)).size(.abs(300, 238))
                    .onClick { ui.close() }
            }"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("image button compiles");
        let ScreenNode::ImageButton(button) = &screen.children[0] else {
            panic!("expected image button")
        };
        assert!(button.on_click.is_some());
        for layout in [
            &button.layout,
            button.hovered_layout.as_ref().expect("hover"),
            button.pressed_layout.as_ref().expect("press"),
        ] {
            assert_eq!(layout.left, Some(2260.0));
            assert_eq!(layout.top, Some(0.0));
            assert_eq!(layout.width, Some(300.0));
            assert_eq!(layout.width_percent, None);
            assert_eq!(layout.height, Some(238.0));
        }
    }

    #[test]
    fn history_rows_keep_intrinsic_height_and_close_is_outside_scrollable() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://history.ui.hks",
            r#"import ui.widgets.*
            canvas {
                image("save-thumbnail://alice").at(.abs(0, 0)).size(.abs(2560, 1440))
                scrollable {
                    column {
                        var index = 0
                        while index < 128 {
                            column {
                                text("Alice").fontSize(66)
                                text("A history entry").width(1338)
                            }.size(.fit()).width(1648)
                            index += 1
                        }
                    }.size(.fit()).width(1648)
                }.at(.abs(460, 241)).size(.abs(1648, 939))
                button { image("save-thumbnail://bob") }.at(.abs(2260, 0)).size(.abs(300, 238)).onClick { ui.close() }
            }"#,
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("full history builds");
        assert_eq!(screen.children.len(), 3);
        let ScreenNode::Scrollable(scroll) = &screen.children[1] else {
            panic!("scrollable expected")
        };
        let ScreenNode::Column(content) = &scroll.children[0] else {
            panic!("content expected")
        };
        assert!(content.layout.fit_content);
        assert_eq!(content.layout.height, None);
        assert_eq!(content.children.len(), 128);
        for row in &content.children {
            let ScreenNode::Column(row) = row else {
                panic!("row expected")
            };
            assert!(row.layout.fit_content);
            assert_eq!(row.children.len(), 2);
        }
        assert!(
            matches!(&screen.children[2], ScreenNode::ImageButton(button) if button.on_click.is_some() && button.layout.left == Some(2260.0))
        );
    }

    #[test]
    fn rich_text_keeps_markup_and_reveal_as_separate_properties() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://ruby.ui.hks",
            "import ui.widgets.*; global var count = 1; canvas { richText(\"{ruby:reader}Alice{/ruby}\").reveal(count) }",
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("rich text builds");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text")
        };
        assert_eq!(text.text, "{ruby:reader}Alice{/ruby}");
        assert!(text.layout.rich_text);
        assert_eq!(text.layout.text_reveal, Some(1));
        assert!(text.layout.reactive_text_reveal.is_some());
        assert!(
            evaluate_ui_component_named_with_args(
                "memory://invalid.ui.hks",
                "import ui.widgets.*; canvas { richText(\"{ruby:reader}Alice\") }",
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn image_button_has_independent_pressed_artwork() {
        let evaluate = |source: &str| {
            evaluate_ui_component_named_with_args(
                "memory://button.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &[],
            )
        };
        let screen = evaluate(
            r#"
            import ui.widgets.*
            canvas {
                button { image("save-thumbnail://alice") }
                    .hovered { image("save-thumbnail://bob") }
                    .pressed { image("save-thumbnail://pressed").size(.abs(32, 24)) }
            }
        "#,
        )
        .expect("image states build");
        let ScreenNode::ImageButton(button) = &screen.children[0] else {
            panic!("expected image button")
        };
        assert_eq!(button.texture.path, "save-thumbnail://alice");
        assert_eq!(
            button.hovered_texture.as_ref().expect("hover image").path,
            "save-thumbnail://bob"
        );
        assert_eq!(
            button.pressed_texture.as_ref().expect("press image").path,
            "save-thumbnail://pressed"
        );
        assert!(button.pressed_layout.is_some());
        for content in [
            "",
            "text(\"invalid\")",
            "image(\"save-thumbnail://alice\"); image(\"save-thumbnail://bob\")",
        ] {
            let source = format!(
                "import ui.widgets.*\ncanvas {{ button {{ image(\"save-thumbnail://alice\") }}.pressed {{ {content} }} }}"
            );
            assert!(
                evaluate(&source).is_err(),
                "invalid pressed content must be rejected"
            );
        }
    }

    #[test]
    fn button_container_keeps_hit_bounds_separate_from_artwork() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://controls.ui.hks",
            r#"
                import ui.widgets.*
                canvas {
                    button {
                        column {
                            image("save-thumbnail://alice").at(.abs(8, 6)).size(.abs(40, 30))
                        }.size(.abs(64, 48))
                    }.at(.abs(100, 20)).size(.abs(64, 48))
                        .onClick { sfx("ui/confirm"); ui.close() }
                }
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("container button compiles");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected content button");
        };
        assert_eq!(button.layout.left, Some(100.0));
        assert_eq!(button.layout.width, Some(64.0));
        assert_eq!(button.background, Some([0.0; 4]));
        assert!(button.on_click.is_some());
        let ScreenNode::Column(column) = &button.children[0] else {
            panic!("expected column");
        };
        let ScreenNode::Image(image) = &column.children[0] else {
            panic!("expected image");
        };
        assert_eq!(image.layout.left, Some(8.0));
        assert_eq!(image.layout.width, Some(40.0));
    }

    #[test]
    fn clipped_container_preserves_mirrored_image_layout() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://portrait.ui.hks",
            r#"
                import ui.widgets.*
                canvas {
                    column {
                        image("save-thumbnail://alice").at(.abs(-10, -8))
                            .size(.abs(100, 100)).flipX().stretch().tint(1, 0.5, 0.25, 0.65)
                    }.size(.abs(80, 84)).clip()
                }
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("clipped image UI compiles");
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("expected clipping container");
        };
        assert!(column.layout.clip);
        let ScreenNode::Image(image) = &column.children[0] else {
            panic!("expected image");
        };
        assert!(image.layout.flip_x);
        assert!(image.layout.image_stretch);
        assert_eq!(image.layout.image_tint, Some([1.0, 0.5, 0.25, 0.65]));
        assert_eq!(image.layout.left, Some(-10.0));
        assert_eq!(image.layout.top, Some(-8.0));
    }

    #[test]
    fn button_keeps_authored_text_color_and_shadow() {
        let evaluate = |source: &str| {
            evaluate_ui_component_named_with_args(
                "memory://typography.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &[],
            )
        };
        let screen = evaluate(
            r#"
            import ui.widgets.*
            canvas {
                button { text("Alice").color(0.2, 0.3, 0.4, 1).textShadow(false) }
                text("Bob").textShadow(false).fitText()
            }
        "#,
        )
        .expect("authored typography");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected button");
        };
        assert_eq!(button.color, Some([0.2, 0.3, 0.4, 1.0]));
        assert_eq!(button.layout.text_shadow, Some(false));
        let ScreenNode::Text(text) = &screen.children[1] else {
            panic!("expected text");
        };
        assert_eq!(text.layout.text_shadow, Some(false));
        assert!(text.layout.text_fit);
        assert!(evaluate("import ui.widgets.*; canvas { column {}.fitText() }").is_err());
        assert!(evaluate("import ui.widgets.*; canvas { text(\"Alice\").stretch() }").is_err());
    }

    #[test]
    fn hover_offset_keeps_optional_reactive_selection() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://tabs.ui.hks",
            r#"
                import ui.widgets.*
                global var selected: Bool = true
                canvas {
                    column { text("alice") }
                        .hoverOffset(-84, 0, ${selected}).time(0.2).easing(.linear)
                    column { text("bob") }.hoverOffset(-20, 0)
                }
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("hover UI builds with optional closure arguments");
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("expected column")
        };
        assert_eq!(column.layout.hover_offset, Some([-84.0, 0.0]));
        assert!(column.layout.hover_active);
        let mut binding = column
            .layout
            .reactive_hover_active
            .clone()
            .expect("selection is reactive");
        binding
            .globals
            .insert("selected".into(), Value::Bool(false));
        assert_eq!(
            evaluate_ui_reactive_binding(&binding, &crate::ui::UiModels::default())
                .expect("selection reevaluates"),
            Value::Bool(false)
        );
        let ScreenNode::Column(column) = &screen.children[1] else {
            panic!("expected column")
        };
        assert!(!column.layout.hover_active);
    }

    #[test]
    fn reactive_name_fallback_clears_ids_when_custom_names_are_shown() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://names.ui.hks",
            r#"
                import ui.widgets.*
                global var speaker = "alice"
                fn fallback(name: String) -> String {
                    if name == "alice" { return "" }
                    if name == "bob" { return "" }
                    return name
                }
                canvas { text(${fallback(speaker)}) }
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("name UI compiles");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text")
        };
        assert_eq!(text.text, "");
        let mut binding = text.reactive_text.clone().expect("reactive name");
        for (speaker, expected) in [("bob", ""), ("guest", "guest"), ("alice", "")] {
            binding
                .globals
                .insert("speaker".into(), Value::String(speaker.into()));
            assert_eq!(
                evaluate_ui_reactive_binding(&binding, &crate::ui::UiModels::default())
                    .expect("name evaluates"),
                Value::String(expected.into())
            );
        }
    }

    #[test]
    fn numeric_label_helper_keeps_its_reactive_dependency() {
        let source = r#"
            import ui.widgets.*
            global var level: Float = 0.54
            fn label(value: Float) -> String {
                let tenths = value * 10 + 0.5
                let rounded = tenths.toInt().toFloat() / 10
                rounded.toString()
            }
            canvas { text(${label(level)}) }
        "#;
        let screen = evaluate_ui_component_named_with_args(
            "memory://numeric.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("numeric UI builds");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text")
        };
        assert_eq!(text.text, "0.5");
        let mut binding = text
            .reactive_text
            .clone()
            .expect("reactive expression retained");
        binding.globals.insert("level".into(), Value::Number(0.87));
        assert_eq!(
            evaluate_ui_reactive_binding(&binding, &crate::ui::UiModels::default())
                .expect("updated label evaluates"),
            Value::String("0.9".into())
        );
    }

    #[test]
    fn text_alignment_is_script_owned_and_validated() {
        let evaluate = |source: &str| {
            evaluate_ui_component_named_with_args(
                "memory://alignment.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &[],
            )
        };
        let screen = evaluate("import ui.widgets.*\ncanvas { text(\"alice\").textAlign(0.5) }")
            .expect("centered text compiles");
        assert!(matches!(&screen.children[0], ScreenNode::Text(text) if text.align == Some(0.5)));
        assert!(evaluate("import ui.widgets.*\ncanvas { text(\"bob\").textAlign(2) }").is_err());
    }

    #[test]
    fn screen_can_admit_a_persistent_overlay_and_wrap_auto_height_text() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://alice.ui.hks",
            "import ui.widgets.*; canvas { text(\"Alice\").width(400) }.allowOverlay(\"tools\")",
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("screen policy compiles");
        assert_eq!(screen.allowed_overlays, ["tools"]);
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text")
        };
        assert_eq!(text.layout.width, Some(400.0));
        assert_eq!(text.layout.height, None);
    }

    #[test]
    fn inline_button_can_use_intrinsic_content_size() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://inline.ui.hks",
            r#"import ui.widgets.*
                canvas {
                    button { row { text("Alice") }.size(.fit()) }.size(.fit())
                }.pauseScene(true)
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("intrinsic button");
        assert!(screen.pauses_scene);
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("button")
        };
        assert!(button.layout.fit_content);
        assert_eq!(button.padding_x, Some(0.0));
        let ScreenNode::Row(row) = &button.children[0] else {
            panic!("row")
        };
        assert!(row.layout.fit_content);
    }

    #[test]
    fn save_thumbnail_sources_do_not_require_a_texture_descriptor() {
        let textures = TextureCatalog::default();
        let source = resolve_texture(&textures, "save-thumbnail://manual-1")
            .expect("valid thumbnail source");
        assert_eq!(source.path, "save-thumbnail://manual-1");
        assert!(resolve_texture(&textures, "save-thumbnail://../alice").is_err());
    }

    #[test]
    fn ui_close_returns_owned_data_and_preserves_unit_and_null() {
        for (expression, expected) in [
            ("ui.close()", Value::Unit),
            ("ui.close(null)", Value::Optional(None)),
            (
                "ui.close(.{ a: 1 })",
                Value::Map(BTreeMap::from([("a".into(), Value::Int(1))])),
            ),
        ] {
            let source = format!(
                "import ui.widgets.*\nscreen {{ button {{ text(\"Confirm\") }}.onClick {{ {expression} }} }}"
            );
            let screen = evaluate_ui_component_named(
                "memory://result.ui.hks",
                &source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("UI compiles");
            let ScreenNode::Button(button) = &screen.children[0] else {
                panic!("button");
            };
            let (effects, _) = evaluate_ui_callback(
                button.on_click.as_ref().expect("callback"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default(),
            )
            .expect("callback returns data");
            assert_eq!(effects, vec![UiEffect::CloseUi { value: expected }]);
        }
        assert!(validate_ui_result(&Value::Handle { type_id: 1, id: 1 }).is_err());
    }

    #[test]
    fn optional_float_widget_arguments_infer_integer_literals() {
        for expression in [
            "text(\"Bob\").bob(6, 1.2)",
            "text(\"Bob\").bob(-6, 1)",
            "text(\"Bob\").bob()",
            "text(\"Pulse\").pulse(2)",
        ] {
            let source = format!("import ui.widgets.*\ncanvas {{ {expression} }}");
            evaluate_ui_component_named(
                "memory://animation.ui.hks",
                &source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
        let error = evaluate_ui_component_named("memory://animation.ui.hks",
            "import ui.widgets.*\nlet distance: Int = 6\ncanvas { text(\"Bob\").bob(distance, 1.2) }",
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default())
            .expect_err("an explicitly typed Int requires toFloat");
        assert!(error.to_string().contains("got Int"), "{error}");
    }

    #[test]
    fn input_widgets_deliver_typed_proposals_without_running_during_mount() {
        let source = r#"import ui.widgets.*
global var enabled: Bool = false
global var volume: Float = 0.5
global var name: String = "alice"
fn accept(value: Float) { volume = value }
screen {
    checkbox(${enabled}).onChange { value: Bool -> enabled = value }
    slider(${volume}, 0.0, 1.0).onChange(accept)
    textInput(${name}).placeholder("Name").onChange { value: String -> name = value }
}"#;
        let screen = evaluate_ui_component_named(
            "memory://inputs.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("inputs compile and mount");
        for (index, argument, global) in [
            (0, Value::Bool(true), "enabled"),
            (1, Value::Number(0.8), "volume"),
            (2, Value::String("bob".into()), "name"),
        ] {
            let ScreenNode::Input(input) = &screen.children[index] else {
                panic!("expected input")
            };
            let callback = input.on_change.as_ref().expect("callback retained");
            assert_ne!(
                callback.globals.get(global),
                Some(&argument),
                "mount must not invoke a change callback"
            );
            let (effects, globals) = evaluate_ui_callback_with_args(
                callback,
                &BTreeMap::new(),
                &crate::ui::UiModels::default(),
                vec![argument.clone()],
            )
            .expect("typed handler executes");
            assert!(effects.is_empty());
            assert_eq!(globals.get(global), Some(&argument));
        }
    }

    #[test]
    fn ui_callback_cannot_write_story_globals() {
        let screen = evaluate_ui_component_named(
            "memory://readonly.ui.hks",
            "import ui.widgets.*\nscreen { textInput(${name}).onChange { value: String -> name = value } }",
            UiContext::new(BTreeMap::from([("name".into(), StoredValue::String("alice".into()))])),
            &TextureCatalog::default(), &TermCatalog::default(),
        ).expect("reading a story global is permitted");
        let ScreenNode::Input(input) = &screen.children[0] else {
            panic!("expected input");
        };
        let error = evaluate_ui_callback_with_args(
            input.on_change.as_ref().expect("handler"),
            &BTreeMap::new(),
            &crate::ui::UiModels::default(),
            vec![Value::String("bob".into())],
        )
        .expect_err("a UI cannot write its story inputs");
        assert!(error.to_string().contains("read-only"), "{error}");
    }

    #[test]
    fn restored_record_fields_keep_numeric_comparison_types_in_ui() {
        let source = "import ui.widgets.*\nscreen { if progress.version > 0 { text(\"Alice\") } }";
        let context = UiContext::new(BTreeMap::from([(
            "progress".into(),
            StoredValue::Map(BTreeMap::from([("version".into(), StoredValue::Int(1))])),
        )]));
        let screen = evaluate_ui_component_named(
            "memory://record.ui.hks",
            source,
            context,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("numeric fields compile");
        assert_eq!(screen.children.len(), 1);
        assert!(
            evaluate_ui_component_named(
                "memory://record.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default()
            )
            .is_err(),
            "missing saved state must not become dynamically truthy"
        );
    }

    #[test]
    fn captured_history_properties_do_not_depend_on_unrelated_models() {
        let screen = evaluate_ui_component_named(
            "memory://history.ui.hks",
            r#"import ui.widgets.*
            @ui
            global fn main() -> UiNode {
                let entry = .{ text: "Alice" }
                canvas { richText(entry.text) }
            }"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("synthetic history");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("text node")
        };
        let mut property = text.reactive_text.clone().expect("captured property");
        assert!(
            property.dependencies.is_empty(),
            "immutable capture has no global reads"
        );
        assert!(
            property.globals.is_empty(),
            "do not retain the document's unrelated globals"
        );
        let mut models = crate::ui::UiModels::default();
        models.set("time", StoredValue::Int(1));
        models.set("history", StoredValue::String("Bob".into()));
        assert!(!refresh_ui_property_models(&mut property, &models));
        assert_eq!(
            evaluate_ui_reactive_binding(&property, &models).expect("captured text"),
            Value::String("Alice".into())
        );
    }

    #[test]
    fn property_dependencies_include_script_helpers() {
        let screen = evaluate_ui_component_named(
            "memory://helper.ui.hks",
            "import ui.widgets.*\nglobal var name: String = \"Alice\"\nfn label() -> String { name }\ncanvas { text(label()) }",
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(),
        ).expect("helper UI");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("text node")
        };
        let property = text.reactive_text.as_ref().expect("property");
        assert!(property.dependencies.contains("name"));
    }

    #[test]
    fn scrollable_accepts_contextual_default_anchors() {
        for (name, expected) in [
            ("top", crate::ui::ScrollAnchor::Top),
            ("center", crate::ui::ScrollAnchor::Center),
            ("bottom", crate::ui::ScrollAnchor::Bottom),
        ] {
            let source = format!(
                "import ui.widgets.*\ncanvas {{ scrollable {{ text(\"Alice\") }}.defaultScrollAnchor(.{name}) }}"
            );
            let screen = evaluate_ui_component_named(
                "memory://scroll.ui.hks",
                &source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("scroll anchor is a typed modifier");
            let ScreenNode::Scrollable(scroll) = &screen.children[0] else {
                panic!("scrollable");
            };
            assert_eq!(scroll.default_scroll_anchor, expected);
        }
        assert!(
            evaluate_ui_component_named(
                "memory://invalid.ui.hks",
                "import ui.widgets.*\ncanvas { text(\"Alice\").defaultScrollAnchor(.bottom) }",
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn ui_imports_survive_numeric_inference_in_parameterized_entry() {
        let source = r#"
            import ui.widgets.*
            @ui global fn main(fraction: Float) -> UiNode {
                var amount = fraction
                if amount < 0 { amount = 0 }
                if amount > 1 { amount = 1 }
                canvas {
                    column {}.size(.abs(320 * amount, 20)).surface(1, 1, 1, 1)
                    text("Alice")
                }
            }
        "#;
        let screen = evaluate_ui_component_named_with_args(
            "memory://progress.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[StoredValue::Float(0.5)],
        )
        .expect("numeric rechecking must preserve widget imports");
        assert_eq!(screen.children.len(), 2);
    }

    #[test]
    fn ui_entry_initializes_private_state_and_callbacks_share_it() {
        let source = r#"import ui.widgets.*
global var name: String = "alice"
@ui global fn main() -> UiNode {
    screen { textInput(${name}).onChange { value: String -> name = value } }
}"#;
        let mount = || {
            evaluate_ui_component_named(
                "memory://local.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("mount")
        };
        let screen = mount();
        let ScreenNode::Input(input) = &screen.children[0] else {
            panic!("input");
        };
        assert_eq!(input.value, StoredValue::String("alice".into()));
        let (_, local) = evaluate_ui_callback_with_args(
            input.on_change.as_ref().expect("handler"),
            &BTreeMap::new(),
            &crate::ui::UiModels::default(),
            vec![Value::String("bob".into())],
        )
        .expect("local write");
        let mut binding = input.reactive_value.clone().expect("binding");
        binding.globals.extend(local);
        assert_eq!(
            evaluate_ui_reactive_binding(&binding, &crate::ui::UiModels::default())
                .expect("updated"),
            Value::String("bob".into())
        );
        let other = mount();
        let ScreenNode::Input(input) = &other.children[0] else {
            panic!("input");
        };
        assert_eq!(
            input.value,
            StoredValue::String("alice".into()),
            "another screen has independent state"
        );
    }

    #[test]
    fn ui_entry_cannot_mutate_an_input_record() {
        let source = r#"import ui.widgets.*
@ui global fn main(player: .{ name: String }) -> UiNode {
    let alias = player
    alias.name = "bob"
    screen { text(player.name) }
}"#;
        let error = evaluate_ui_component_named_with_args(
            "memory://readonly_input.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[StoredValue::Map(BTreeMap::from([(
                "name".into(),
                StoredValue::String("alice".into()),
            )]))],
        )
        .expect_err("UI input records are read-only");
        assert!(
            error.to_string().contains("ReadOnlyValue") || error.to_string().contains("read-only"),
            "{error}"
        );
    }

    #[test]
    fn input_rejects_wrong_callback_type_before_interaction() {
        let error = evaluate_ui_component_named(
            "memory://invalid_input.ui.hks",
            "import ui.widgets.*\nscreen { checkbox(false).onChange { value: String -> () } }",
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("Bool input rejects String callback");
        assert!(error.to_string().contains("(Bool) -> Unit"), "{error}");
    }

    #[derive(Default)]
    struct NamespaceTestContext;

    #[hiraku_script::hks_module("ui.widgets")]
    mod namespaced_test_api {
        use super::*;

        #[hks]
        fn native_label(
            _context: &mut NamespaceTestContext,
            value: String,
        ) -> Result<String, NativeError> {
            Ok(value)
        }

        #[hks]
        fn native_visible(
            _context: &mut NamespaceTestContext,
            _value: hiraku_script::native::HksBinding<bool>,
        ) -> Result<(), NativeError> {
            Ok(())
        }
    }

    #[test]
    fn module_macro_registers_a_namespaced_selector_and_signature() {
        let mut registry = NativeRegistry::<NamespaceTestContext>::new();
        namespaced_test_api::register_hks(&mut registry)
            .expect("namespaced module should register");
        let manifest = registry.manifest();
        let builtin = manifest
            .resolve_selector("ui.widgets", "label")
            .expect("module namespace should become the selector namespace");
        let signature = manifest
            .signature(builtin)
            .expect("module macro should generate a signature");
        assert_eq!(
            signature.parameters,
            vec![hiraku_script::ScriptType::String]
        );
        assert_eq!(signature.result, hiraku_script::ScriptType::String);
        let visible = manifest
            .resolve_selector("ui.widgets", "visible")
            .expect("reactive function should be namespaced");
        assert_eq!(
            manifest
                .signature(visible)
                .expect("reactive signature exists")
                .parameters,
            vec![hiraku_script::ScriptType::Binding(Box::new(
                hiraku_script::ScriptType::Bool,
            ))]
        );
    }

    #[test]
    fn statement_boundaries_commit_allocated_nodes_not_returned_handles() {
        let source = r#"
            import ui.widgets.*
            fn heading() -> UiNode { text("alice") }
            screen {
                let title = heading()
                let alias = title
                alias
                42
                text("bob")
            }
        "#;
        let screen = evaluate_ui_component_named(
            "memory://boundaries.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("node collection does not depend on statement values");
        assert_eq!(
            screen.children.len(),
            2,
            "returned aliases do not emit duplicate nodes"
        );
        let ScreenNode::Text(first) = &screen.children[0] else {
            panic!("expected text")
        };
        let ScreenNode::Text(second) = &screen.children[1] else {
            panic!("expected text")
        };
        assert_eq!(first.text, "alice");
        assert_eq!(second.text, "bob");
    }

    #[test]
    fn local_tab_callback_recomposes_branches_without_resetting_state() {
        let screen = evaluate_ui_component_named(
            "memory://tabs.ui.hks",
            r#"
            import ui.widgets.*
            global var category = "alice"
            fn tab(value: String) -> UiNode {
                button { text(value) }.onClick { category = value }
            }
            @ui
            global fn main() -> UiNode {
                canvas {
                    if category == "alice" { text("Alice panel") }
                    if category == "bob" { column { text("Bob panel") } }
                    tab("bob")
                }
            }
        "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("tabs compile");
        let ScreenNode::Button(button) = &screen.children[1] else {
            panic!("tab button");
        };
        let callback = button.on_click.as_ref().expect("click callback");
        let renderer = screen.composition.as_ref().expect("compiled composition");
        let (effects, globals) = evaluate_ui_callback_with_args(
            callback,
            &renderer.globals,
            &crate::ui::UiModels::default(),
            vec![],
        )
        .expect("click");
        assert!(effects.is_empty());
        let next = renderer
            .render(
                &globals,
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("recompose");
        let ScreenNode::Column(panel) = &next.children[0] else {
            panic!("selected branch must change");
        };
        let ScreenNode::Text(text) = &panel.children[0] else {
            panic!("panel text");
        };
        assert_eq!(text.text, "Bob panel");
        assert_eq!(
            next.composition.as_ref().expect("state").globals["category"],
            Value::String("bob".into())
        );
        assert_eq!(
            renderer.globals["category"],
            Value::String("alice".into()),
            "another mount's initial state remains independent"
        );
    }

    #[test]
    fn evaluates_script_defined_compose_ui() {
        let source = r#"
import ui.widgets.*

screen {
    column {
        text("Hello ${playerName}")
        button("continue") {
            text("Continue")
        }.onClick {
            sfx("ui/confirm")
        }
    }.gap(18)
}
"#;
        let values = UiContext::new(BTreeMap::from([(
            "playerName".to_string(),
            StoredValue::String("alice".to_string()),
        )]));

        let screen = evaluate_ui_component_named(
            "memory://compose.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("the declarative UI should compile and evaluate");

        assert_eq!(screen.children.len(), 1);
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("the root child should be a column");
        };
        assert_eq!(column.gap, 18.0);
        assert_eq!(column.children.len(), 2);
        let ScreenNode::Text(text) = &column.children[0] else {
            panic!("the first column child should be text");
        };
        assert_eq!(text.text, "Hello alice");
        let ScreenNode::Button(button) = &column.children[1] else {
            panic!("the second column child should be a button");
        };
        assert_eq!(button.text, "Continue");
        assert_eq!(button.value, Some(StoredValue::String("continue".into())));
        assert_eq!(
            evaluate_ui_callback(
                button.on_click.as_ref().expect("callback retained"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default()
            )
            .expect("click executes")
            .0,
            vec![UiEffect::PlaySfx {
                name: "ui/confirm".into(),
                volume: 1.0,
            }]
        );
    }

    #[test]
    fn custom_button_hover_surface_preserves_content_and_geometry() {
        let screen = evaluate_ui_component_named(
            "memory://buttons.ui.hks",
            r#"
import ui.widgets.*
canvas {
    button { column { text("Alice") } }
        .size(.abs(240, 120)).surface(0, 0, 0, 0).hoveredSurface(1, 1, 1, 1)
}
"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("custom button renders");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected custom-content button");
        };
        assert_eq!(button.background, Some([0.0; 4]));
        assert_eq!(button.hovered_background, Some([1.0; 4]));
        assert_eq!(button.pressed_background, Some([1.0; 4]));
        assert_eq!(button.hover_scale, 1.0);
        assert_eq!(button.press_scale, 1.0);
        assert!(matches!(
            button.children.as_slice(),
            [ScreenNode::Column(_)]
        ));
    }

    #[test]
    fn slot_button_accepts_empty_and_thumbnail_branches() {
        for thumbnail in ["null", "\"save-thumbnail://alice\""] {
            let source = format!(
                r#"
import ui.widgets.*
fn slot(preview: String?) -> UiNode {{
    button {{
        if preview != null {{
            image(preview!).size(.abs(240, 135))
        }} else {{
            spacer().size(.abs(240, 135))
        }}
    }}.size(.abs(260, 155)).onClick {{ storage.save("alice") }}
}}
canvas {{ slot({thumbnail}) }}
"#
            );
            let screen = evaluate_ui_component_named(
                "memory://slots.ui.hks",
                &source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("both slot branches must materialize");
            let callback = match &screen.children[0] {
                ScreenNode::Button(button) => {
                    assert_eq!(thumbnail, "null");
                    assert!(button.text.is_empty());
                    assert!(matches!(
                        button.children.as_slice(),
                        [ScreenNode::Spacer(_)]
                    ));
                    button.on_click.as_ref().expect("empty slot callback")
                }
                ScreenNode::ImageButton(button) => {
                    assert_ne!(thumbnail, "null");
                    assert_eq!(button.texture.path, "save-thumbnail://alice");
                    button.on_click.as_ref().expect("thumbnail slot callback")
                }
                _ => panic!("expected slot button"),
            };
            let (effects, _) =
                evaluate_ui_callback(callback, &BTreeMap::new(), &crate::ui::UiModels::default())
                    .expect("slot click produces an effect without writing storage");
            assert_eq!(
                effects,
                vec![UiEffect::Save {
                    slot: "alice".into()
                }]
            );
        }
    }

    #[test]
    fn ui_callback_passes_image_arguments_to_another_document() {
        let source = r#"
import ui.widgets.*
@ui
global fn main(imageName: String) -> UiNode {
    canvas {
        button { text("View") }.onClick { ui.open("viewer.ui.hks", imageName, "Alice") }
    }
}
"#;
        let screen = evaluate_ui_component_named_with_args(
            "memory://ui/gallery.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[StoredValue::String("save-thumbnail://alice".into())],
        )
        .expect("compile gallery");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected button")
        };
        let (effects, _) = evaluate_ui_callback(
            button.on_click.as_ref().expect("click callback"),
            &BTreeMap::new(),
            &crate::ui::UiModels::default(),
        )
        .expect("capture parameterized open effect");
        let UiEffect::OpenUi {
            role,
            origin,
            arguments,
        } = &effects[0]
        else {
            panic!("expected open")
        };
        assert_eq!(role, "viewer.ui.hks");
        assert_eq!(origin.as_deref(), Some("memory://ui/gallery.ui.hks"));
        assert_eq!(
            arguments,
            &vec![
                StoredValue::String("save-thumbnail://alice".into()),
                StoredValue::String("Alice".into())
            ]
        );
        let viewer = r#"
import ui.widgets.*
@ui
global fn viewer(imageName: String, title: String) -> UiNode {
    canvas { image(imageName); text(title) }
}
"#;
        let view = evaluate_ui_component_named_with_args(
            "memory://ui/viewer.ui.hks",
            viewer,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            arguments,
        )
        .expect("typed viewer receives captured arguments");
        assert_eq!(view.children.len(), 2);
        let ScreenNode::Image(image) = &view.children[0] else {
            panic!("expected image")
        };
        assert_eq!(image.texture.path, "save-thumbnail://alice");
        assert!(
            evaluate_ui_component_named_with_args(
                "memory://ui/viewer.ui.hks",
                viewer,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &[
                    StoredValue::Bool(false),
                    StoredValue::String("Alice".into())
                ],
            )
            .is_err(),
            "incorrect entry argument types must fail"
        );
    }

    #[test]
    fn ui_arguments_never_silently_drop_unsupported_nested_values() {
        assert!(
            ui_argument_to_stored(&Value::List(vec![
                Value::String("alice".into()),
                Value::Unit
            ]))
            .is_err()
        );
        assert!(
            ui_argument_to_stored(&Value::Map(BTreeMap::from([
                ("name".into(), Value::String("alice".into())),
                ("unsupported".into(), Value::Unit),
            ])))
            .is_err()
        );
        assert_eq!(
            ui_argument_to_stored(&Value::List(vec![Value::String("bob".into())]))
                .expect("plain list"),
            StoredValue::Array(vec![StoredValue::String("bob".into())])
        );
    }

    #[test]
    fn projected_images_accept_material_tracks_and_relative_shader_paths() {
        let screen=evaluate_ui_component_named_with_args(
            "memory://screens/alice.ui.hks",
            r#"import ui.widgets.*
                canvas {
                    image("save-thumbnail://alice")
                        .keyframes([.quad(0, 0, 0, 20, 0, 0, 30, 1), .quad(1, 20, 0, -20, 0, 0, 30, 0)])
                        .shader("shaders/fade.wgsl", [], [.at(0, 1, 0, 0, 0), .at(1, 0, 0, 0, 0)])
                        .blend("multiply")
                }
            "#,
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("projected material UI compiles");
        let ScreenNode::Image(image) = &screen.children[0] else {
            panic!("image node")
        };
        let shader = image.layout.shader.as_ref().expect("shader spec");
        assert_eq!(shader.blend, crate::ui::UiShaderBlend::Multiply);
        assert_eq!(shader.path, "memory://screens/shaders/fade.wgsl");
        assert_eq!(shader.keys.len(), 2);
    }

    #[test]
    fn typed_collection_arguments_render_and_callbacks_capture_the_selected_record() {
        let source = r#"import ui.widgets.*
            fn part(span: .{ text: String, id: String }) -> UiNode {
                button { text(span.text) }.onClick { ui.close(span.id) }
            }
            @ui global fn main(rows: List<List<.{ text: String, id: String }>>) -> UiNode {
                canvas { column {
                    var rowIndex=0
                    while rowIndex<count(rows) {
                        let spans=item(rows,rowIndex) as! List<.{ text: String, id: String }>
                        row {
                            var index=0
                            while index<count(spans) {
                                part(item(spans,index) as! .{ text: String, id: String })
                                index+=1
                            }
                        }
                        rowIndex+=1
                    }
                } }
            }"#;
        let arguments = [StoredValue::Array(vec![StoredValue::Array(vec![
            StoredValue::Map(BTreeMap::from([
                ("text".into(), StoredValue::String("Alice".into())),
                ("id".into(), StoredValue::String("bob".into())),
            ])),
        ])])];
        let screen = evaluate_ui_component_named_with_args(
            "memory://list.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &arguments,
        )
        .expect("typed rows render");
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("column")
        };
        let ScreenNode::Row(row) = &column.children[0] else {
            panic!("row")
        };
        let ScreenNode::Button(button) = &row.children[0] else {
            panic!("button")
        };
        let (effects, _) = evaluate_ui_callback(
            button.on_click.as_ref().expect("callback"),
            &screen.composition.as_ref().expect("composition").globals,
            &crate::ui::UiModels::default(),
        )
        .expect("captured record callback");
        assert_eq!(
            effects,
            vec![UiEffect::CloseUi {
                value: Value::String("bob".into())
            }]
        );
    }

    #[test]
    fn nested_confirmation_can_complete_its_owner_and_animate_without_story_builtins() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://confirmation.ui.hks",
            r#"import ui.widgets.*
                canvas {
                    button { text("Alice") }.hoverBrightness(1.33333).onClick { ui.complete("bob") }
                    text("Track").keyframes([.rect(0, 0, 0, 20, 10, 0), .rect(1, 100, 0, 20, 10, 1)])
                }.fade(0.2)
            "#,
            UiContext::default(), &TextureCatalog::default(), &TermCatalog::default(), &[],
        ).expect("confirmation and keyframes compile");
        assert_eq!(screen.fade_seconds, 0.2);
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("confirmation button")
        };
        let (effects, _) = evaluate_ui_callback(
            button.on_click.as_ref().expect("callback"),
            &screen.composition.as_ref().expect("composition").globals,
            &crate::ui::UiModels::default(),
        )
        .expect("completion callback");
        assert_eq!(
            effects,
            vec![UiEffect::CompleteUi {
                value: Value::String("bob".into())
            }]
        );
    }

    #[test]
    fn confirmation_pauses_mount_timers_and_cancel_retains_local_state() {
        let screen = evaluate_ui_component_named_with_args(
            "memory://confirmation.ui.hks",
            r#"import ui.widgets.*
                global var selected = false
                canvas {
                    if selected {
                        button { column { text("Confirm") } }
                            .buttonImage("save-thumbnail://alice")
                            .hoveredButtonImage("save-thumbnail://bob")
                            .onClick { audio.stopVoice(); ui.close("alice") }
                        button { text("Cancel") }.onClick { selected = false }
                    } else {
                        button { text("Bob") }.onClick { selected = true }
                    }
                }.pauseTimers(selected).after(2) { ui.close("") }
            "#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("confirmation UI");
        let composition = screen.composition.as_ref().expect("composition");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("select button")
        };
        let (effects, selected) = evaluate_ui_callback(
            button.on_click.as_ref().expect("click"),
            &composition.globals,
            &crate::ui::UiModels::default(),
        )
        .expect("select");
        assert!(effects.is_empty());
        let paused = composition
            .render(
                &selected,
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("confirmation tree");
        assert!(paused.timers_paused);
        let ScreenNode::Button(confirm) = &paused.children[0] else {
            panic!("confirm")
        };
        assert_eq!(confirm.padding_x, Some(0.0));
        assert!(confirm.background_texture.is_some());
        assert!(confirm.hovered_background_texture.is_some());
        let (effects, _) = evaluate_ui_callback(
            confirm.on_click.as_ref().expect("confirm handler"),
            &selected,
            &crate::ui::UiModels::default(),
        )
        .expect("confirm");
        assert_eq!(
            effects,
            vec![
                UiEffect::StopVoice,
                UiEffect::CloseUi {
                    value: Value::String("alice".into())
                }
            ]
        );
        let ScreenNode::Button(cancel) = &paused.children[1] else {
            panic!("cancel")
        };
        let (effects, resumed) = evaluate_ui_callback(
            cancel.on_click.as_ref().expect("cancel handler"),
            &selected,
            &crate::ui::UiModels::default(),
        )
        .expect("cancel");
        assert!(
            effects.is_empty(),
            "cancel does not close or reopen the modal"
        );
        let resumed = composition
            .render(
                &resumed,
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("resumed tree");
        assert!(!resumed.timers_paused);
    }

    #[test]
    fn main_ui_function_receives_typed_positional_arguments() {
        let source = r#"
import ui.widgets.*

@ui
global fn card(label: String, count: Int) -> UiNode {
    screen {
        column {
            text(label)
            progress(count.toFloat()).range(0, 10)
        }
    }


}
"#;
        let screen = evaluate_ui_component_named_with_args(
            "memory://card.ui.hks",
            source,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[StoredValue::String("Items".into()), StoredValue::Int(3)],
        )
        .expect("the UI main function must receive persisted arguments");
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("the main function must produce its screen tree")
        };
        assert!(matches!(&column.children[0], ScreenNode::Text(text) if text.text == "Items"));
        assert!(matches!(&column.children[1], ScreenNode::Bar(bar) if bar.value == 3.0));
    }

    #[test]
    fn modal_availability_arguments_filter_nodes_and_callbacks_return_room_ids() {
        let source = r#"
            import ui.widgets.*
            fn room(id: String) -> UiNode { button { text(id) }.onClick { ui.close(id) } }
            @ui
            global fn main(north: Bool, south: Bool, east: Bool, west: Bool,
                upper: Bool, lower: Bool, inner: Bool, outer: Bool) -> UiNode {
                canvas {
                    if north == false { room("north") }
                    if south == false { room("south") }
                    if east == false { room("east") }
                    if west == false { room("west") }
                    if upper == false { room("upper") }
                    if lower == false { room("lower") }
                    if inner == false { room("inner") }
                    if outer == false { room("outer") }
                }
            }
        "#;
        for mask in 0u32..256 {
            let args = (0..8)
                .map(|i| StoredValue::Bool(mask & (1 << i) != 0))
                .collect::<Vec<_>>();
            let screen = evaluate_ui_component_named_with_args(
                "memory://rooms.ui.hks",
                source,
                UiContext::default(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
                &args,
            )
            .expect("availability UI builds");
            let remaining = [
                "north", "south", "east", "west", "upper", "lower", "inner", "outer",
            ]
            .into_iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) == 0)
            .map(|(_, name)| name)
            .collect::<Vec<_>>();
            assert_eq!(screen.children.len(), remaining.len());
            for (node, id) in screen.children.iter().zip(remaining) {
                let ScreenNode::Button(button) = node else {
                    panic!("expected room button")
                };
                let (effects, _) = evaluate_ui_callback(
                    button.on_click.as_ref().expect("selection callback"),
                    &BTreeMap::new(),
                    &crate::ui::UiModels::default(),
                )
                .expect("select room");
                assert_eq!(
                    effects,
                    [UiEffect::CloseUi {
                        value: Value::String(id.into())
                    }]
                );
            }
        }
    }

    #[test]
    fn detail_buttons_capture_each_function_invocation_independently() {
        let screen = evaluate_ui_component_named(
            "memory://details.ui.hks",
            r#"import ui.widgets.*
global var selectedTitle: String = ""
global var selectedBody: String = ""
fn entryButton(title: String, body: String) -> UiNode {
    button { text(title) }.onClick {
        selectedTitle = title
        selectedBody = body
    }
}
screen {
    entryButton("Alice", "First entry")
    entryButton("Bob", "Second entry")
}"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("detail screen mounts without invoking handlers");
        for (index, title, body) in [(0, "Alice", "First entry"), (1, "Bob", "Second entry")] {
            let ScreenNode::Button(button) = &screen.children[index] else {
                panic!("expected detail button");
            };
            let (effects, globals) = evaluate_ui_callback(
                button.on_click.as_ref().expect("detail handler"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default(),
            )
            .expect("captured parameters remain available after return");
            assert!(effects.is_empty());
            assert_eq!(
                globals.get("selectedTitle"),
                Some(&Value::String(title.into()))
            );
            assert_eq!(
                globals.get("selectedBody"),
                Some(&Value::String(body.into()))
            );
        }
    }

    #[test]
    fn on_click_reads_current_globals_and_preserves_operation_order() {
        let screen = evaluate_ui_component_named(
            "memory://callback.ui.hks",
            r#"
import ui.widgets.*
screen {
    button { text("Save") }.onClick {
        if playerName == "alice" { storage.save("alice") }
        else { storage.save("bob") }
        ui.close()
    }
}
"#,
            UiContext::new(BTreeMap::from([(
                "playerName".into(),
                StoredValue::String("alice".into()),
            )])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("screen constructs without running its callback");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected button")
        };
        let callback = button.on_click.as_ref().expect("callback retained");
        let globals = BTreeMap::from([("playerName".into(), Value::String("bob".into()))]);
        let (effects, _) =
            evaluate_ui_callback(callback, &globals, &crate::ui::UiModels::default())
                .expect("callback executes against current globals");
        assert_eq!(
            effects,
            vec![
                UiEffect::Save { slot: "bob".into() },
                UiEffect::CloseUi { value: Value::Unit }
            ]
        );
    }

    #[test]
    fn nonterminating_click_does_not_execute_during_mount() {
        let screen = evaluate_ui_component_named(
            "memory://callback.ui.hks",
            "import ui.widgets.*\nscreen { button { text(\"Run\") }.onClick { while true {} } }",
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("mount does not evaluate onClick");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected button")
        };
        let error = evaluate_ui_callback(
            button.on_click.as_ref().expect("callback retained"),
            &BTreeMap::new(),
            &crate::ui::UiModels::default(),
        )
        .expect_err("click budget must terminate evaluation");
        assert!(error.to_string().contains("instruction budget"));
    }

    #[test]
    fn reusable_slot_buttons_keep_independent_callback_arguments() {
        let screen = evaluate_ui_component_named(
            "memory://slots.ui.hks",
            r#"
import ui.widgets.*
fn slot(key: String) -> UiNode {
    button { text(key) }.onClick { storage.save(key); ui.close() }
}
canvas {
    slot("alice")
    slot("bob")
}
"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("reusable slot buttons must evaluate");
        assert_eq!(screen.children.len(), 2);
        for (node, key) in screen.children.iter().zip(["alice", "bob"]) {
            let ScreenNode::Button(button) = node else {
                panic!("expected a slot button")
            };
            let (effects, _) = evaluate_ui_callback(
                button.on_click.as_ref().expect("callback retained"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default(),
            )
            .expect("captured slot callback executes without writing storage");
            assert_eq!(effects.first(), Some(&UiEffect::Save { slot: key.into() }));
            assert!(matches!(effects.get(1), Some(UiEffect::CloseUi { .. })));
            assert_eq!(effects.len(), 2);
        }
    }

    #[test]
    fn preference_getters_do_not_reserve_script_global_names() {
        let preferences = crate::storage::UserSettings {
            display_available: true,
            fullscreen: true,
            bgm_volume: 0.25,
            ..Default::default()
        };
        let context = UiContext::default().with_preferences(preferences);
        assert!(context.story_values().is_empty());
        let screen = evaluate_ui_component_named(
            "memory://preferences.ui.hks",
            r#"import ui.widgets.*
global let displayAvailable = preferences.displayAvailable()
global let fullscreen = preferences.fullscreen()
global let bgmVolume = audio.bgmVolume()
canvas {
    button { text("Settings") }.enabled(displayAvailable)
    button { text("Fullscreen") }.enabled(fullscreen)
    progress(bgmVolume)
}"#,
            context,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("private host preferences must not collide with UI declarations");
        for node in &screen.children[..2] {
            let ScreenNode::Button(button) = node else {
                panic!("expected button")
            };
            assert!(button.enabled);
        }
        let ScreenNode::Bar(bar) = &screen.children[2] else {
            panic!("expected progress")
        };
        assert_eq!(bar.value, 0.25);
    }

    #[test]
    fn on_click_executes_typed_state_actions_without_routes() {
        let screen = evaluate_ui_component_named(
            "memory://actions.ui.hks",
            r#"
import ui.widgets.*
screen {
    button { text("Save") }.onClick {
        sfx("ui/confirm")
        storage.save("quick")
    }
}
"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("typed onClick actions must evaluate");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected a button")
        };
        assert_eq!(
            evaluate_ui_callback(
                button.on_click.as_ref().expect("callback retained"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default()
            )
            .expect("click executes")
            .0,
            vec![
                UiEffect::PlaySfx {
                    name: "ui/confirm".into(),
                    volume: 1.0,
                },
                UiEffect::Save {
                    slot: "quick".into(),
                },
            ]
        );
    }

    #[test]
    fn ui_navigation_is_relative_to_the_declaring_component() {
        let screen = evaluate_ui_component_named(
            "memory://ui/menu.ui.hks",
            r#"
import ui.widgets.*
screen {
    button { text("Return") }.onClick {
        story.goto("../title.hks", .{ reset: .session })
        sfx("ui/should-not-play")
    }
}
"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("typed UI navigation must evaluate");
        let ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected a button")
        };
        assert_eq!(
            evaluate_ui_callback(
                button.on_click.as_ref().expect("callback retained"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default()
            )
            .expect("click executes")
            .0,
            vec![UiEffect::Navigate(NavigationRequest {
                path: "../title.hks".into(),
                kind: super::super::navigation::NavigationKind::Goto,
                reset: super::super::navigation::NavigationReset::Session,
                preload: None,
                origin: Some("memory://ui/menu.ui.hks".into()),
            })]
        );
    }

    #[test]
    fn unknown_ui_node_methods_are_rejected_during_compilation() {
        let error = evaluate_ui_component_named(
            "memory://invalid.ui.hks",
            r#"
import ui.widgets.*
__uiScreen {}.missingMethod()
"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("nominal UI methods must be resolved by the compiler manifest");
        let message = error.to_string();
        assert!(
            message.contains("unknown method `missingMethod` for `UiNode`"),
            "unexpected diagnostic: {message}"
        );
    }

    #[test]
    fn script_defined_choice_renderer_expands_the_engine_choice_model() {
        let source = r#"
import ui.widgets.*

canvas {
    scrollable {
        choiceOptions { index: Int, label: String ->
            button(index) { text(label) }
        }.gap(9)
    }.scrollSpeed(64)
}
"#;
        let values = UiContext::new(BTreeMap::from([(
            "choice".to_string(),
            StoredValue::Map(BTreeMap::from([
                (
                    "options".to_string(),
                    StoredValue::Array(vec![
                        StoredValue::String("Route A".to_string()),
                        StoredValue::String("Route B".to_string()),
                    ]),
                ),
                (
                    "enabled".into(),
                    StoredValue::Array(vec![StoredValue::Bool(false), StoredValue::Bool(true)]),
                ),
            ])),
        )]));

        let screen = evaluate_ui_component_named(
            "memory://choice.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("the project-defined choice renderer should evaluate");

        let ScreenNode::Scrollable(scrollable) = &screen.children[0] else {
            panic!("canvas child should be scrollable");
        };
        assert_eq!(scrollable.speed, 64.0);
        let ScreenNode::Column(options) = &scrollable.children[0] else {
            panic!("choiceOptions should materialize a column");
        };
        assert_eq!(options.gap, 9.0);
        assert_eq!(options.justify, None);
        assert_eq!(options.align_items.as_deref(), Some("stretch"));
        assert!(matches!(
            &options.children[0],
            ScreenNode::Button(button)
                if button.text == "Route A"
                    && !button.enabled
                    && button.value == Some(StoredValue::Int(0))
        ));
        assert!(matches!(
            &options.children[1],
            ScreenNode::Button(button)
                if button.text == "Route B"
                    && button.enabled
                    && button.value == Some(StoredValue::Int(1))
        ));
    }

    #[test]
    fn choice_renderer_receives_typed_option_parameters() {
        let values = UiContext::new(BTreeMap::from([(
            "choice".into(),
            StoredValue::Map(BTreeMap::from([
                (
                    "options".into(),
                    StoredValue::Array(vec![StoredValue::String("Alice".into())]),
                ),
                (
                    "parameters".into(),
                    StoredValue::Map(BTreeMap::from([("0".into(), StoredValue::Bool(true))])),
                ),
            ])),
        )]));
        for renderer in [
            "choiceOptions { index: Int, label: String, danger: Bool -> button(index) { if danger { text(label) } else { text(\"Bob\") } } }",
            "choiceOptions(renderOption)",
        ] {
            let source = format!(
                r#"
                import ui.widgets.*
                fn renderOption(index: Int, label: String, danger: Bool) -> UiNode {{
                    button(index) {{ if danger {{ text(label) }} else {{ text("Bob") }} }}
                }}
                canvas {{ {renderer} }}
            "#
            );
            let screen = evaluate_ui_component_named(
                "memory://choice.ui.hks",
                &source,
                values.clone(),
                &TextureCatalog::default(),
                &TermCatalog::default(),
            )
            .expect("typed choice renders");
            let ScreenNode::Column(options) = &screen.children[0] else {
                panic!("expected options column")
            };
            assert!(
                matches!(&options.children[0], ScreenNode::Button(button) if button.text == "Alice")
            );
        }
        let source = r#"import ui.widgets.*
            canvas { choiceOptions { index: Int, label: String, data: String -> button(index) { text(data) } } }
        "#;
        let error = evaluate_ui_component_named(
            "memory://choice.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("Bool must not be accepted as String");
        assert!(error.to_string().contains("String"), "{error}");
    }

    #[test]
    fn choice_renderer_reports_missing_parameters() {
        let values = UiContext::new(BTreeMap::from([(
            "choice".into(),
            StoredValue::Map(BTreeMap::from([(
                "options".into(),
                StoredValue::Array(vec![StoredValue::String("Alice".into())]),
            )])),
        )]));
        let source = r#"import ui.widgets.*
            canvas { choiceOptions { index: Int, label: String, data: Bool -> button(index) { text(label) } } }
        "#;
        let error = evaluate_ui_component_named(
            "memory://choice.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("missing parameters must be diagnosed");
        assert!(
            error.to_string().contains("missing .params(value)"),
            "{error}"
        );
    }

    #[test]
    fn canvas_choice_options_can_center_fixed_size_buttons() {
        let values = UiContext::new(BTreeMap::from([(
            "choice".into(),
            StoredValue::Map(BTreeMap::from([(
                "options".into(),
                StoredValue::Array(vec![StoredValue::String("Route A".into())]),
            )])),
        )]));
        let screen = evaluate_ui_component_named(
            "memory://choice.ui.hks",
            r#"import ui.widgets.*
canvas {
    choiceOptions { index: Int, label: String ->
        button(index) { text(label) }.size(.abs(600, 80))
    }.gap(16).size(.rel(100, 100)).centered()
}"#,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("choice options should fill and center within a canvas");
        let ScreenNode::Column(options) = &screen.children[0] else {
            panic!("choiceOptions should materialize a column");
        };
        assert!(!screen.panel);
        assert_eq!(options.justify.as_deref(), Some("center"));
        assert_eq!(options.align_items.as_deref(), Some("center"));
        assert_eq!(options.layout.width_percent, Some(100.0));
        assert_eq!(options.layout.height_percent, Some(100.0));
    }

    #[test]
    fn choice_renderer_also_accepts_a_named_script_function() {
        let source = r#"
import ui.widgets.*
fn renderOption(index: Int, label: String) -> UiNode {
    button(index) { text(label) }
}
canvas { choiceOptions(renderOption) }
"#;
        let values = UiContext::new(BTreeMap::from([(
            "choice".to_string(),
            StoredValue::Map(BTreeMap::from([(
                "options".to_string(),
                StoredValue::Array(vec![StoredValue::String("Named".to_string())]),
            )])),
        )]));
        let screen = evaluate_ui_component_named(
            "memory://named-choice.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("a named function should remain a valid choice renderer");
        assert!(matches!(
            &screen.children[0],
            ScreenNode::Column(options)
                if matches!(&options.children[0], ScreenNode::Button(button) if button.text == "Named")
        ));
    }

    #[test]
    fn canvas_is_an_unpanelled_transparent_root() {
        let screen = evaluate_ui_component_named(
            "memory://overlay.ui.hks",
            "import ui.widgets.*\ncanvas { text(\"HUD\") }",
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("canvas UI should evaluate");

        assert!(!screen.panel);
        assert_eq!(screen.overlay, Some([0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn ui_animation_uses_a_shared_fluent_animation_spec() {
        let screen = evaluate_ui_component_named(
            "memory://animated.ui.hks",
            concat!(
                "import ui.widgets.*\n",
                "canvas {\n",
                "  text(\"Pulse\").time(2.0).easing(.cubicBezier(0.25, 0.1, 0.25, 1)).repeatForever()\n",
                "  text(\"Spinner\").spin(1.5)\n",
                "  text(\"Phases\").phaseAnimator([.rotation(0), .rotation(90)]).time(0.4).easing(.easeInOut).repeatForever()\n",
                "  text(\"Pulse\").pulse()\n",
                "  text(\"Bob\").bob()\n",
                "}",
            ),
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("animated UI should evaluate");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("canvas child should be text")
        };
        let animation = text.layout.animation.expect("animation is retained");
        assert_eq!(animation.duration(), 2.0);
        assert_eq!(
            animation.easing(),
            crate::script::animation::Easing::CubicBezier(0.25, 0.1, 0.25, 1.0)
        );
        assert!(animation.repeats());
        let ScreenNode::Text(spinner) = &screen.children[1] else {
            panic!("second child should be text")
        };
        let spin = spinner
            .layout
            .phase_animation
            .as_ref()
            .expect("spin creates a phase timeline");
        assert!(spin.continuous_rotation);
        assert_eq!(spin.spec.duration(), 1.5);
        let ScreenNode::Text(phases) = &screen.children[2] else {
            panic!("third child should be text")
        };
        assert_eq!(
            phases
                .layout
                .phase_animation
                .as_ref()
                .expect("phase animator is retained")
                .phases
                .len(),
            2
        );
        assert!(matches!(
            &screen.children[3],
            ScreenNode::Text(node) if node.layout.phase_animation.is_some()
        ));
        assert!(matches!(
            &screen.children[4],
            ScreenNode::Text(node) if node.layout.phase_animation.is_some()
        ));
    }

    #[test]
    fn text_buttons_can_dispatch_engine_actions_from_declarative_ui() {
        let screen = evaluate_ui_component_named(
            "memory://save-controls.ui.hks",
            concat!(
                "import ui.widgets.*\n",
                "canvas {\n",
                "  button { text(\"Passive\") }\n",
                "  button { text(\"Quick Save\") }.onClick { storage.save(\"quick\") }.hoverScale(1.08).pressScale(0.94)\n",
                "}",
            ),
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("action button should compile");
        let ScreenNode::Button(passive) = &screen.children[0] else {
            panic!("first child should be a closure-only button")
        };
        assert_eq!(passive.value, None);
        assert!(passive.on_click.is_none());
        let ScreenNode::Button(button) = &screen.children[1] else {
            panic!("second child should be an action button")
        };
        assert_eq!(
            evaluate_ui_callback(
                button.on_click.as_ref().expect("callback retained"),
                &BTreeMap::new(),
                &crate::ui::UiModels::default()
            )
            .expect("click executes")
            .0,
            vec![UiEffect::Save {
                slot: "quick".into()
            }]
        );
        assert_eq!(button.value, None);
        assert_eq!(button.hover_scale, 1.08);
        assert_eq!(button.press_scale, 0.94);
    }

    #[test]
    fn dialogue_prefix_accepts_integer_character_counts() {
        let dialogue = |count| {
            StoredValue::Map(BTreeMap::from([
                (
                    "text".into(),
                    StoredValue::String("A\u{e9}\u{1f642}".into()),
                ),
                ("revealedCharacters".into(), StoredValue::Int(count)),
            ]))
        };
        let screen = evaluate_ui_component_named(
            "memory://dialogue_prefix.ui.hks",
            "import ui.widgets.*\ncanvas { text(${dialogue.text.prefix(dialogue.revealedCharacters)}) }",
            UiContext::new(BTreeMap::from([("dialogue".into(), dialogue(2))])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        ).expect("the dialogue model count matches the prefix signature");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text")
        };
        assert_eq!(text.text, "A\u{e9}");
        let binding = text
            .reactive_text
            .as_ref()
            .expect("dialogue text retains its binding");
        let mut models = crate::ui::UiModels::default();
        for (count, expected) in [(0, ""), (3, "A\u{e9}\u{1f642}"), (20, "A\u{e9}\u{1f642}")] {
            models.set("dialogue", dialogue(count));
            assert_eq!(
                evaluate_ui_reactive_binding(binding, &models).expect("updated count evaluates"),
                Value::String(expected.into())
            );
        }
        for source in [
            "import ui.widgets.*\ncanvas { text(\"abc\".prefix(1.5)) }",
            "import ui.widgets.*\ncanvas { text(\"abc\".prefix(-1)) }",
        ] {
            assert!(
                evaluate_ui_component_named(
                    "memory://invalid_prefix.ui.hks",
                    source,
                    UiContext::default(),
                    &TextureCatalog::default(),
                    &TermCatalog::default()
                )
                .is_err(),
                "fractional and negative counts must be rejected"
            );
        }
    }

    #[test]
    fn string_interpolation_captures_current_ui_model_values() {
        let screen = evaluate_ui_component_named(
            "memory://live_overlay.ui.hks",
            concat!(
                "import ui.widgets.*\n",
                "canvas { text(\"Player ${playerName}, ${time.elapsedSeconds}s\") }",
            ),
            UiContext::new(BTreeMap::from([(
                "playerName".to_string(),
                StoredValue::String("alice".to_string()),
            )])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("String interpolation should evaluate");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("canvas child should be text");
        };
        // text accepts String: interpolation must not leak a raw template into
        // the renderer. Reactive reevaluation belongs to the UI compiler.
        assert_eq!(text.text, "Player alice, 0s");
        assert!(text.binding.is_none());
    }

    #[test]
    fn typed_models_bind_visibility_buttons_and_progress() {
        let screen = evaluate_ui_component_named(
            "memory://typed_live.ui.hks",
            r#"import ui.widgets.*
canvas {
    text(${hud.label}).visible(${hud.visible})
    button("continue") { text("Continue") }.enabled(${hud.canContinue})
    progress(${player.health}).range(0, 100)
}"#,
            UiContext::new(BTreeMap::from([
                (
                    "hud".to_string(),
                    StoredValue::Map(BTreeMap::from([
                        (
                            "label".to_string(),
                            StoredValue::String("Status".to_string()),
                        ),
                        ("visible".to_string(), StoredValue::Bool(true)),
                        ("canContinue".to_string(), StoredValue::Bool(false)),
                    ])),
                ),
                (
                    "player".to_string(),
                    StoredValue::Map(BTreeMap::from([(
                        "health".to_string(),
                        StoredValue::Float(75.0),
                    )])),
                ),
            ])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("typed live bindings should evaluate");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("first child should be text")
        };
        assert_eq!(text.text, "Status");
        assert!(text.layout.reactive_visibility.is_some());
        let ScreenNode::Button(button) = &screen.children[1] else {
            panic!("second child should be a button")
        };
        assert!(!button.enabled);
        assert!(button.reactive_enabled.is_some());
        let ScreenNode::Bar(progress) = &screen.children[2] else {
            panic!("third child should be progress")
        };
        assert_eq!(progress.value, 75.0);
        assert!(progress.reactive_value.is_some());
        assert_eq!((progress.min, progress.max), (0.0, 100.0));

        let mut models = crate::ui::UiModels::default();
        models.set(
            "player",
            StoredValue::Map(BTreeMap::from([(
                "health".to_string(),
                StoredValue::Float(25.0),
            )])),
        );
        assert_eq!(
            evaluate_ui_reactive_binding(
                progress
                    .reactive_value
                    .as_ref()
                    .expect("progress expression is retained"),
                &models,
            )
            .expect("updated model should evaluate"),
            Value::Number(25.0)
        );
    }

    #[test]
    fn plain_property_expressions_are_live_without_binding_syntax() {
        let screen = evaluate_ui_component_named(
            "memory://properties.ui.hks",
            r#"import ui.widgets.*
global var count = 1
global var shown = true
canvas {
    text(count.toString()).visible(shown)
    button { text("Increment") }.onClick { count += 1 }
}"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("ordinary expressions compile");
        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("expected text");
        };
        assert_eq!(text.text, "1");
        let mut property = text
            .reactive_text
            .clone()
            .expect("compiler extracted property");
        property.globals.insert("count".into(), Value::Int(2));
        assert_eq!(
            evaluate_ui_reactive_binding(&property, &crate::ui::UiModels::default())
                .expect("recompute"),
            Value::String("2".into())
        );
        let mut visible = text
            .layout
            .reactive_visibility
            .clone()
            .expect("live visibility");
        visible.globals.insert("shown".into(), Value::Bool(false));
        assert_eq!(
            evaluate_ui_reactive_binding(&visible, &crate::ui::UiModels::default())
                .expect("recompute"),
            Value::Bool(false)
        );
        let plan = &screen
            .composition
            .as_ref()
            .expect("composition")
            .document
            .plan;
        assert!(
            !plan.structural_globals.contains("count"),
            "callback writes do not trigger rebuilding"
        );
        assert!(!plan.structural_globals.contains("shown"));
        assert!(
            plan.sites
                .iter()
                .any(|site| site.kind == hiraku_ui::RegionKind::Property)
        );
    }

    #[test]
    fn ordinary_script_calls_compute_live_properties() {
        let screen = evaluate_ui_component_named(
            "memory://delegate.ui.hks",
            r#"import ui.widgets.*
fn readHealth() -> Float { player.health }
canvas {
    progress(readHealth()).range(0, 100)
}"#,
            UiContext::new(BTreeMap::from([(
                "player".to_string(),
                StoredValue::Map(BTreeMap::from([(
                    "health".to_string(),
                    StoredValue::Float(75.0),
                )])),
            )])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("script call should become a property computation");

        let ScreenNode::Bar(progress) = &screen.children[0] else {
            panic!("binding consumer should be a progress bar")
        };
        let binding = progress
            .reactive_value
            .as_ref()
            .expect("binding getter should be retained");

        let mut models = crate::ui::UiModels::default();
        models.set(
            "player",
            StoredValue::Map(BTreeMap::from([(
                "health".to_string(),
                StoredValue::Float(30.0),
            )])),
        );
        assert_eq!(
            evaluate_ui_reactive_binding(binding, &models)
                .expect("script getter should evaluate against the latest model"),
            Value::Number(30.0),
        );
    }

    #[test]
    fn rejects_bare_strings_as_ui_nodes() {
        let error = evaluate_ui_component_named(
            "memory://invalid.ui.hks",
            "import ui.widgets.*\nscreen { \"not a node\" }",
            UiContext::new(BTreeMap::new()),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("bare strings must not silently become UI nodes");

        assert!(
            error.to_string().contains("wrap the value with text"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn term_accepts_only_a_user_defined_string_id() {
        let terms = crate::glossary::parse_term_catalog(
            "memory://glossary.hson",
            r#".{ terms: [
                .{ id: "ether", name: "Ether", description: "A fictional substance." },
            ] }"#,
        )
        .expect("glossary should parse");
        let screen = evaluate_ui_component_named(
            "memory://terms.ui.hks",
            r#"import ui.widgets.*
screen { term("ether") }"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &terms,
        )
        .expect("a defined string term should render");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("the default term component should render its display name");
        };
        assert_eq!(text.text, "Ether");
    }

    #[test]
    fn fluent_methods_accept_trailing_closures() {
        let source = r#"
import ui.widgets.*

screen {
    image("alice/background").at(.rel(0, 0)).size(.rel(100, 100))
    image("alice/logo").at(.rel(50, 10)).size(.rel(25, 25))

    button("alice") {
        image("alice/idle")
    }
        .enabled(false)
        .hoveredWhenDisabled(true)
        .hovered {
            image("alice/hovered")
        }

    button("bob") {
        image("bob/idle").at(.rel(10, 70)).size(.rel(20, 20))
    }.hovered {
        image("bob/hovered").at(.rel(10, 69)).size(.rel(20, 21))
    }
}.panel(false)
"#;
        let error = evaluate_ui_component_named(
            "memory://hover.ui.hks",
            source,
            UiContext::new(BTreeMap::new()),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("the fake texture is intentionally absent");

        assert!(
            matches!(error, UiVmError::Invalid(ref message) if message.contains("alice/")),
            "the fluent closure should execute before texture resolution: {error}"
        );
    }
}
#[hiraku_script::hks_module("profile")]
mod profile_api {
    use super::*;
    #[hks(name = "readBool")]
    fn read_bool(_context: &mut UiVmContext, key: String) -> Result<bool, NativeError> {
        crate::storage::profile::read_bool(&key)
            .map_err(|error| NativeError::message(error.to_string()))
    }
    #[hks(name = "writeBool")]
    fn write_bool(_context: &mut UiVmContext, key: String, value: bool) -> Result<(), NativeError> {
        crate::storage::profile::write_bool(&key, value)
            .map_err(|error| NativeError::message(error.to_string()))
    }
}
