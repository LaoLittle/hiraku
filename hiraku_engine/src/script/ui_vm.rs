use std::collections::{BTreeMap, BTreeSet};

use hiraku_script::native::{
    FromHksValue, HksBindable, HksBinding, HksCallable, HksClosure, IntoHksValue, NativeError,
    NativeRegistry,
};
use hiraku_script::{
    BuiltinManifest, LinkedVm, LinkedVmEvent, ModuleId, RenderOptions, ScriptType, SourceMap,
    StatementValue, Value, compile_with_manifest, link_named_modules, parse_program,
    render_diagnostics,
};
use thiserror::Error;

use crate::{
    glossary::{TermCatalog, TermId},
    state::StoredValue,
    texture::TextureCatalog,
    ui::{
        BarNode, ButtonNode, ContainerNode, ScreenImageButtonNode, ScreenImageNode, ScreenLayout,
        ScreenNode, ScreenSpec, ScreenTexture, ScrollableNode, SpacerNode, TextNode, ToggleNode,
        UiEffect, UiPhaseAnimation, UiReactiveBinding,
    },
};

use super::{
    animation::{AnimationPhase, AnimationSpec, register_animation_api},
    navigation::{NavigationHandle, NavigationRequest, NavigationResetValue},
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
}

impl UiSize {
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
    Toggle(bool),
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
    kind: UiDraftKind,
    content: Option<HksClosure>,
    hovered: Option<HksClosure>,
    checked: Option<HksClosure>,
    on_click: Option<HksClosure>,
    layout: ScreenLayout,
    panel: bool,
    enabled: bool,
    enabled_binding: Option<HksBinding<bool>>,
    visible: bool,
    visible_binding: Option<HksBinding<bool>>,
    hovered_when_disabled: bool,
    hover_scale: f32,
    press_scale: f32,
    scroll_speed: f32,
    gap: f32,
    padding: f32,
    surface: Option<[f32; 4]>,
    text_size: Option<f32>,
    text_color: Option<[f32; 4]>,
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
            kind,
            content,
            hovered: None,
            checked: None,
            on_click: None,
            layout: ScreenLayout::default(),
            panel: true,
            enabled: true,
            enabled_binding: None,
            visible: true,
            visible_binding: None,
            hovered_when_disabled: false,
            hover_scale: 1.0,
            press_scale: 1.0,
            scroll_speed: 48.0,
            gap: 12.0,
            padding: 0.0,
            surface: None,
            text_size: None,
            text_color: None,
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

    fn effect(&self, handle: UiEffectHandle) -> Result<&UiEffect, NativeError> {
        self.effects
            .get(&handle.0)
            .ok_or_else(|| NativeError::message(format!("unknown UiEffect handle {}", handle.0)))
    }

    fn insert_navigation(&mut self, navigation: NavigationRequest) -> NavigationHandle {
        self.next_effect += 1;
        self.effects
            .insert(self.next_effect, UiEffect::Navigate(navigation));
        NavigationHandle(self.next_effect)
    }

    fn reset_navigation(
        &mut self,
        NavigationHandle(handle): NavigationHandle,
        reset: NavigationResetValue,
    ) -> Result<NavigationHandle, NativeError> {
        let Some(UiEffect::Navigate(navigation)) = self.effects.get_mut(&handle) else {
            return Err(NativeError::message(format!(
                "unknown Navigation handle {handle}"
            )));
        };
        navigation.reset = reset.into();
        Ok(NavigationHandle(handle))
    }

    fn navigation_effect(
        &self,
        NavigationHandle(handle): NavigationHandle,
    ) -> Result<&UiEffect, NativeError> {
        self.effects
            .get(&handle)
            .filter(|effect| matches!(effect, UiEffect::Navigate(_)))
            .ok_or_else(|| NativeError::message(format!("unknown Navigation handle {handle}")))
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

    #[hks(name = "__uiToggle")]
    fn ui_toggle(
        context: &mut UiVmContext,
        value: bool,
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
        characters: f64,
    ) -> Result<String, NativeError> {
        if !characters.is_finite() || characters < 0.0 {
            return Err(NativeError::message(
                "String.prefix character count must be finite and non-negative",
            ));
        }
        Ok(value.chars().take(characters.floor() as usize).collect())
    }

    /// Creates a writable binding from ordinary script functions. The getter
    /// takes no arguments; the setter takes the new value. The tuple is a
    /// save-safe pair of script callable IDs/closures, never native pointers.
    #[hks(name = "binding", raw)]
    fn ui_binding(
        _context: &mut UiVmContext,
        call: &hiraku_script::BuiltinCall,
    ) -> Result<Value, NativeError> {
        let [getter, setter] = call.arguments.as_slice() else {
            return Err(NativeError::Arity {
                expected: 2,
                actual: call.arguments.len(),
            });
        };
        hiraku_script::native::HksCallable::from_hks_value(&getter.value).map_err(|_| {
            NativeError::message(format!(
                "binding getter must be a script function, got {:?}",
                getter.value
            ))
        })?;
        hiraku_script::native::HksCallable::from_hks_value(&setter.value).map_err(|_| {
            NativeError::message(format!(
                "binding setter must be a script function, got {:?}",
                setter.value
            ))
        })?;
        Ok(Value::Tuple(vec![
            getter.value.clone(),
            setter.value.clone(),
        ]))
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

    #[hks(name = "fontSize", receiver)]
    fn ui_font_size(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        size: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.text_size = Some(non_negative(size, "UI font size")?);
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

    #[hks(name = "reset", receiver)]
    fn navigation_reset(
        context: &mut UiVmContext,
        navigation: NavigationHandle,
        reset: NavigationResetValue,
    ) -> Result<NavigationHandle, NativeError> {
        context.reset_navigation(navigation, reset)
    }

    #[hks(name = "animation", receiver)]
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
        context.node_mut(node)?.animation = Some(animation);
        Ok(node)
    }

    #[allow(non_snake_case)]
    #[hks(name = "phaseAnimator", receiver)]
    fn ui_phase_animator(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        phases: Vec<AnimationPhase>,
        animation: AnimationSpec,
    ) -> Result<UiNodeHandle, NativeError> {
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

    #[hks]
    fn native_open(context: &mut UiVmContext, role: String) -> Result<UiEffectHandle, NativeError> {
        if role.trim().is_empty() {
            return Err(NativeError::message("UI role must not be empty"));
        }
        Ok(context.insert_effect(UiEffect::OpenUi { role }))
    }

    #[hks]
    fn native_close(context: &mut UiVmContext) -> Result<UiEffectHandle, NativeError> {
        Ok(context.insert_effect(UiEffect::CloseUi))
    }
}

#[hiraku_script::hks_module("storage")]
mod storage_actions {
    use super::*;

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
        path: String,
    ) -> Result<NavigationHandle, NativeError> {
        let navigation =
            NavigationRequest::goto(path)?.with_origin(context.navigation_origin.clone());
        Ok(context.insert_navigation(navigation))
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
    UiPosition::register_hks(&mut registry)
        .expect("UiPosition registration must be internally consistent");
    UiSize::register_hks(&mut registry).expect("UiSize registration must be internally consistent");
    register_animation_api(&mut registry)
        .expect("animation API registration must be internally consistent");
    NavigationResetValue::register_hks(&mut registry)
        .expect("navigation reset API registration must be internally consistent");
    // Register the nominal result type before compiling the HKS standard library.
    let ui_node = registry.define_type("UiNode");
    registry.define_type("UiEffect");
    native_ui::register_hks(&mut registry)
        .expect("UI native primitives must be internally consistent");
    ui_actions::register_hks(&mut registry).expect("UI actions must be internally consistent");
    storage_actions::register_hks(&mut registry)
        .expect("storage actions must be internally consistent");
    story_actions::register_hks(&mut registry)
        .expect("story actions must be internally consistent");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("binding"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::Function, ScriptType::Function],
                variadic: None,
                result: ScriptType::Binding(Box::new(ScriptType::Any)),
            },
        )
        .expect("binding signature must target its raw native implementation");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("button"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![
                    ScriptType::Any,
                    ScriptType::Nullable(Box::new(ScriptType::Function)),
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
                ("elapsedSeconds".to_string(), ScriptType::Number),
                ("unixSeconds".to_string(), ScriptType::Number),
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
        StoredValue::Int(_) | StoredValue::Float(_) => ScriptType::Number,
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
    #[error("failed to compile declarative UI: {0}")]
    Compile(String),
    #[error("failed to link declarative UI: {0}")]
    Link(String),
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
    let manifest = registry.manifest();
    let standard = compile_module(UI_STDLIB_PATH, UI_STDLIB_SOURCE, &manifest)?;
    let parsed = parse_module(path, source)?;
    let entries = parsed
        .statements
        .iter()
        .filter_map(|statement| {
            let hiraku_script::Stmt::Function {
                attributes,
                exported,
                name,
                ..
            } = statement
            else {
                return None;
            };
            attributes
                .iter()
                .any(|attribute| attribute.name == "ui")
                .then_some((*exported, name.clone()))
        })
        .collect::<Vec<_>>();
    if entries.len() > 1 {
        return Err(UiVmError::Invalid(
            "a UI module may declare only one `@ui` entrypoint".into(),
        ));
    }
    if entries.first().is_some_and(|(exported, _)| !exported) {
        return Err(UiVmError::Invalid(
            "the `@ui` entrypoint must be declared with `global fn`".into(),
        ));
    }
    let document = compile_parsed_module(path, source, &parsed, &manifest)?;
    let entry_symbol = entries
        .first()
        .map(|(_, name)| {
            document.symbols.find(name).ok_or_else(|| {
                UiVmError::Invalid(format!("UI entrypoint `{name}` was not interned"))
            })
        })
        .transpose()?;
    let program = link_named_modules(
        vec![(Some("ui.widgets".to_string()), standard), (None, document)],
        &manifest,
    )
    .map_err(|errors| {
        UiVmError::Link(
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    let materialize_program = program.clone();
    let mut context = UiVmContext::new(values, terms.clone()).with_navigation_origin(path);
    let vm = if let Some(symbol) = entry_symbol {
        let callable = Value::Function {
            module: Some(1),
            symbol,
        };
        LinkedVm::from_callable(
            program,
            &callable,
            arguments.iter().map(stored_to_hks).collect(),
        )
    } else if arguments.is_empty() {
        // Temporary migration path for existing UI modules. New UI modules
        // may expose one explicit @ui function when it needs parameters.
        LinkedVm::new(program, ModuleId(1))
    } else {
        return Err(UiVmError::Invalid(
            "parameterized UI modules require an `@ui global fn` entrypoint".into(),
        ));
    }
    .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    let roots = collect_nodes(vm, &registry, &mut context)?;
    if roots.len() != 1 {
        return Err(UiVmError::Invalid(format!(
            "a UI document must produce exactly one root node, got {}",
            roots.len()
        )));
    }
    materialize_screen(
        roots[0],
        &materialize_program,
        &registry,
        &mut context,
        textures,
    )
}

fn parse_module(path: &str, source: &str) -> Result<hiraku_script::Program, UiVmError> {
    let mut sources = SourceMap::new();
    let source_id = sources.insert(path, source);
    parse_program(source).map_err(|errors| {
        UiVmError::Compile(render_diagnostics(
            &errors
                .into_iter()
                .map(|error| error.diagnostic(source_id.clone()))
                .collect::<Vec<_>>(),
            &sources,
            RenderOptions::terminal(),
        ))
    })
}

fn compile_module(
    path: &str,
    source: &str,
    manifest: &BuiltinManifest,
) -> Result<hiraku_script::Bytecode, UiVmError> {
    let program = parse_module(path, source)?;
    compile_parsed_module(path, source, &program, manifest)
}

fn compile_parsed_module(
    path: &str,
    source: &str,
    program: &hiraku_script::Program,
    manifest: &BuiltinManifest,
) -> Result<hiraku_script::Bytecode, UiVmError> {
    let mut sources = SourceMap::new();
    let source_id = sources.insert(path, source);
    compile_with_manifest(program, source_hash(path, source), manifest).map_err(|errors| {
        UiVmError::Compile(render_diagnostics(
            &errors
                .into_iter()
                .map(|error| error.diagnostic(source_id.clone()))
                .collect::<Vec<_>>(),
            &sources,
            RenderOptions::terminal(),
        ))
    })
}

fn source_hash(path: &str, source: &str) -> u64 {
    path.bytes()
        .chain([0])
        .chain(source.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
        })
}

fn collect_nodes(
    mut vm: LinkedVm,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let mut nodes = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let event = match vm.step() {
            Ok(event) => event,
            Err(error) => {
                let snapshot = vm.snapshot();
                let frame = snapshot.frames.last();
                return Err(UiVmError::Runtime(match frame {
                    Some(frame) => format!(
                        "{error:?} in module {} at {:?}:{} with registers {:?}",
                        frame.module.0,
                        frame.vm.location,
                        frame.vm.pc.saturating_sub(1),
                        frame.vm.registers,
                    ),
                    None => format!("{error:?}"),
                }));
            }
        };
        match event {
            Some(LinkedVmEvent::Call(call)) => {
                let value = registry
                    .call(context, &call)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
                vm.resume(value)
                    .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
            }
            Some(LinkedVmEvent::Statement(StatementValue::Value(value))) => {
                let node = UiNodeHandle::from_hks_value(&value).map_err(|_| {
                    UiVmError::Invalid("UI expression statements must produce UiNode".into())
                })?;
                // Script-defined components can return the same handle through
                // several wrapper frames. It still represents one emitted node.
                if seen.insert(node.0) {
                    nodes.push(node);
                }
            }
            Some(LinkedVmEvent::Statement(StatementValue::Commit)) => {}
            Some(LinkedVmEvent::Statement(StatementValue::String(_))) => {
                return Err(UiVmError::Invalid(
                    "bare strings are not UI nodes; wrap the value with text(...)".into(),
                ));
            }
            Some(LinkedVmEvent::Completed(_)) => return Ok(nodes),
            None => {
                return Err(UiVmError::Runtime(
                    "UI VM stopped without completing or requesting a native call".into(),
                ));
            }
        }
    }
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
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
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
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    collect_nodes(vm, registry, context)
}

fn closure_effects(
    closure: Option<HksClosure>,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiEffect>, UiVmError> {
    let Some(closure) = closure else {
        return Ok(Vec::new());
    };
    let callable = closure.into_hks_value();
    let mut vm = LinkedVm::from_callable(program.clone(), &callable, Vec::new())
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    let mut effects = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        match vm
            .step()
            .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?
        {
            Some(LinkedVmEvent::Call(call)) => {
                let value = registry
                    .call(context, &call)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
                vm.resume(value)
                    .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
            }
            Some(LinkedVmEvent::Statement(StatementValue::Value(value))) => {
                let effect = if let Ok(handle) = UiEffectHandle::from_hks_value(&value) {
                    (handle.0, context.effect(handle))
                } else if let Ok(handle) = NavigationHandle::from_hks_value(&value) {
                    (handle.0, context.navigation_effect(handle))
                } else {
                    return Err(UiVmError::Invalid(
                        "onClick statements must produce an effect such as sfx(...) or story.goto(...)"
                            .into(),
                    ));
                };
                if seen.insert(effect.0) {
                    effects.push(
                        effect
                            .1
                            .map_err(|error| UiVmError::Runtime(error.to_string()))?
                            .clone(),
                    );
                }
            }
            Some(LinkedVmEvent::Statement(StatementValue::Commit)) => {}
            Some(LinkedVmEvent::Statement(StatementValue::String(_))) => {
                return Err(UiVmError::Invalid(
                    "bare strings are not valid onClick effects".into(),
                ));
            }
            Some(LinkedVmEvent::Completed(_)) => return Ok(effects),
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
            ("elapsedSeconds".to_string(), Value::Number(0.0)),
            ("unixSeconds".to_string(), Value::Number(0.0)),
        ])),
    );
    globals
}

fn stored_to_hks(value: &StoredValue) -> Value {
    match value {
        StoredValue::Bool(value) => Value::Bool(*value),
        StoredValue::Int(value) => Value::Number(*value as f64),
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
) -> UiReactiveBinding {
    UiReactiveBinding {
        program: program.clone(),
        getter: binding.getter().value().clone(),
        setter: binding.setter().map(|setter| setter.value().clone()),
        globals: context_globals(context),
    }
}

fn evaluate_binding_value(
    binding: &UiReactiveBinding,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Value, UiVmError> {
    evaluate_binding_callable(binding, &binding.getter, Vec::new(), registry, context)
}

fn evaluate_binding_callable(
    binding: &UiReactiveBinding,
    callable: &Value,
    arguments: Vec<Value>,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Value, UiVmError> {
    let mut vm = LinkedVm::from_callable(binding.program.clone(), callable, arguments)
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    vm.set_current_globals(&binding.globals)
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    loop {
        match vm
            .step()
            .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?
        {
            Some(LinkedVmEvent::Call(call)) => {
                let value = registry
                    .call(context, &call)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
                vm.resume(value)
                    .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
            }
            Some(LinkedVmEvent::Completed(value)) => return Ok(value),
            Some(LinkedVmEvent::Statement(_)) => {}
            None => {
                return Err(UiVmError::Runtime(
                    "reactive UI expression stopped without returning a value".into(),
                ));
            }
        }
    }
}

/// Invokes the script-defined setter of a writable UI binding. Controls call
/// this at their commit boundary; pointer motion never mutates script state.
pub(crate) fn evaluate_ui_binding_setter(
    binding: &UiReactiveBinding,
    models: &crate::ui::UiModels,
    value: Value,
) -> Result<Value, UiVmError> {
    let setter = binding
        .setter
        .as_ref()
        .ok_or_else(|| UiVmError::Invalid("the UI binding is read-only".into()))?;
    let mut binding = binding.clone();
    for (name, value) in models.roots() {
        binding
            .globals
            .insert(name.to_string(), stored_to_hks(value));
    }
    let values = UiContext::default();
    let registry = ui_registry(&values);
    let mut context = UiVmContext::new(values, TermCatalog::default());
    evaluate_binding_callable(&binding, setter, vec![value], &registry, &mut context)
}

pub(crate) fn evaluate_ui_reactive_binding(
    binding: &UiReactiveBinding,
    models: &crate::ui::UiModels,
) -> Result<Value, UiVmError> {
    let mut binding = binding.clone();
    for (name, value) in models.roots() {
        binding
            .globals
            .insert(name.to_string(), stored_to_hks(value));
    }
    let values = UiContext::default();
    let registry = ui_registry(&values);
    let mut context = UiVmContext::new(values, TermCatalog::default());
    evaluate_binding_value(&binding, &registry, &mut context)
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
    draft.layout.phase_animation = draft.phase_animation;
    draft.layout.visible_binding = None;
    if let Some(binding) = &draft.visible_binding {
        let reactive = reactive_binding(binding, program, context);
        let value = evaluate_binding_value(&reactive, registry, context)?;
        draft.layout.hidden =
            !bool::from_hks_value(&value).map_err(|error| UiVmError::Invalid(error.to_string()))?;
        draft.layout.reactive_visibility = Some(reactive);
    }
    match draft.kind {
        UiDraftKind::Screen => Err(UiVmError::Invalid(
            "screen nodes may only appear at the document root".into(),
        )),
        UiDraftKind::Image(path) => Ok(ScreenNode::Image(ScreenImageNode {
            texture: resolve_texture(textures, &path)?,
            layout: draft.layout,
        })),
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
            let is_template = text.contains("${");
            let text = context
                .values
                .expand_binding(&text)
                .map_err(|error| UiVmError::Invalid(error.to_string()))?;
            Ok(ScreenNode::Text(TextNode {
                binding: is_template.then(|| text.clone()),
                reactive_text: if is_template { None } else { reactive },
                text,
                size: draft.text_size.unwrap_or(28.0),
                color: draft.text_color,
                align: None,
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
                layout: draft.layout,
            }))
        }
        UiDraftKind::Toggle(value) => {
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
            }))
        }
        UiDraftKind::ChoiceOptions(renderer) => {
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
                let rendered = closure_children_with_args(
                    renderer.clone(),
                    vec![Value::Number(index as f64), Value::String(label)],
                    program,
                    registry,
                    context,
                )?;
                if rendered.len() != 1 {
                    return Err(UiVmError::Invalid(
                        "choice option renderer must return exactly one UiNode".into(),
                    ));
                }
                children.push(materialize_node(
                    rendered[0],
                    program,
                    registry,
                    context,
                    textures,
                )?);
            }
            Ok(ScreenNode::Column(ContainerNode {
                gap: draft.gap,
                padding: draft.padding,
                background: draft.surface,
                border: None,
                justify: Some("center".into()),
                align_items: Some("stretch".into()),
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
                justify: None,
                align_items: None,
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
            let click_effects = closure_effects(draft.on_click, program, registry, context)?;
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
                    "button content must produce exactly one text or image node, got {}",
                    normal_handles.len()
                )));
            }
            let normal = materialize_node(normal_handles[0], program, registry, context, textures)?;
            let value = if matches!(&value, Value::Unit) {
                None
            } else {
                Some(stored_value(value)?)
            };
            match normal {
                ScreenNode::Text(text) => Ok(ScreenNode::Button(ButtonNode {
                    text: text.text,
                    value,
                    click_effects,
                    enabled,
                    enabled_binding: None,
                    reactive_enabled,
                    size: text.size,
                    color: None,
                    hovered_color: None,
                    pressed_color: None,
                    insensitive_color: None,
                    background: None,
                    border: None,
                    hovered_background: None,
                    pressed_background: None,
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
                    align: text.align,
                    padding_x: None,
                    padding_y: None,
                    border_width: None,
                    radius: None,
                    layout: draft.layout,
                })),
                ScreenNode::Image(image) => {
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
                        texture: image.texture,
                        hovered_texture,
                        hovered_layout,
                        hover_scale: draft.hover_scale,
                        press_scale: draft.press_scale,
                        value,
                        click_effects,
                        enabled,
                        enabled_binding: None,
                        reactive_enabled,
                        hovered_when_disabled: draft.hovered_when_disabled,
                        layout: image.layout,
                    }))
                }
                _ => Err(UiVmError::Invalid(
                    "button content must be text(...) or image(...)".into(),
                )),
            }
        }
    }
}

fn stored_value(value: Value) -> Result<StoredValue, UiVmError> {
    match value {
        Value::Bool(value) => Ok(StoredValue::Bool(value)),
        Value::Number(value) if value.fract() == 0.0 => Ok(StoredValue::Int(value as i64)),
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
            button.click_effects,
            vec![UiEffect::PlaySfx {
                name: "ui/confirm".into(),
                volume: 1.0,
            }]
        );
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
            progress(count).range(0, 10)
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
    fn on_click_builds_typed_state_actions_without_routes() {
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
            button.click_effects,
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
        story.goto("../title.hks").reset(.session)
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
            button.click_effects,
            vec![UiEffect::Navigate(NavigationRequest {
                path: "../title.hks".into(),
                kind: super::super::navigation::NavigationKind::Goto,
                reset: super::super::navigation::NavigationReset::Session,
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
            StoredValue::Map(BTreeMap::from([(
                "options".to_string(),
                StoredValue::Array(vec![
                    StoredValue::String("Route A".to_string()),
                    StoredValue::String("Route B".to_string()),
                ]),
            )])),
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
        assert!(matches!(
            &options.children[0],
            ScreenNode::Button(button)
                if button.text == "Route A"
                    && button.value == Some(StoredValue::Int(0))
        ));
        assert!(matches!(
            &options.children[1],
            ScreenNode::Button(button)
                if button.text == "Route B"
                    && button.value == Some(StoredValue::Int(1))
        ));
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
                "  text(\"Pulse\").animation(.linear(2.0).repeatForever())\n",
                "  text(\"Spinner\").spin(1.5)\n",
                "  text(\"Phases\").phaseAnimator([.rotation(0), .rotation(90)], .easeInOut(0.4).repeatForever())\n",
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
        assert!(passive.click_effects.is_empty());
        let ScreenNode::Button(button) = &screen.children[1] else {
            panic!("second child should be an action button")
        };
        assert_eq!(
            button.click_effects,
            vec![UiEffect::Save {
                slot: "quick".into()
            }]
        );
        assert_eq!(button.value, None);
        assert_eq!(button.hover_scale, 1.08);
        assert_eq!(button.press_scale, 0.94);
    }

    #[test]
    fn live_text_bindings_capture_story_values_and_preserve_models() {
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
        .expect("live UI binding should evaluate");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("canvas child should be text");
        };
        assert_eq!(
            text.binding.as_deref(),
            Some("Player alice, ${time.elapsedSeconds}s")
        );
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
    fn script_functions_form_a_writable_binding_delegate() {
        let screen = evaluate_ui_component_named(
            "memory://delegate.ui.hks",
            r#"import ui.widgets.*
fn readHealth() -> Float { player.health }
fn writeHealth(value: Float) { () }
canvas {
    progress(binding(readHealth, writeHealth)).range(0, 100)
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
        .expect("script getter/setter functions should form a binding");

        let ScreenNode::Bar(progress) = &screen.children[0] else {
            panic!("binding consumer should be a progress bar")
        };
        let binding = progress
            .reactive_value
            .as_ref()
            .expect("binding getter should be retained");
        assert!(
            binding.setter.is_some(),
            "binding setter should be retained"
        );

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
        assert_eq!(
            evaluate_ui_binding_setter(binding, &models, Value::Number(42.0))
                .expect("script setter should be independently invokable"),
            Value::Unit,
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
