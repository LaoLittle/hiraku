use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use bevy::{
    app::AppExit,
    audio::{AudioSink, AudioSinkPlayback, Volume},
    ecs::system::SystemParam,
    log::{info, warn},
    picking::{
        hover::PickingInteraction,
        pointer::{PointerButton, PointerId},
    },
    prelude::*,
};

use crate::{
    audio::{AudioCatalog, PreludeLoopAudio, load_audio_catalog},
    character::{
        CharacterBlendMode, CharacterCatalog, CharacterMaskKind, CharacterPartDefinition,
        load_character_catalog,
    },
    effect::custom::{CustomScreenEffectMaterial, CustomScreenEffectPlayer},
    effect::transition::{RuleTransitionMaterial, RuleTransitionMesh, RuleTransitionPlayer},
    glossary::{TermCatalog, load_term_catalog},
    render::camera::{
        CameraShake, CameraShakeState, CameraState, CameraTweenState, WorldCamera, focus_layer,
        scene_layer, setup_stage_cameras, start_camera_tween, ui_layer,
    },
    render::character_part::{AlphaMaskMaterial, CharacterPartVisual, MultiplyMaterial},
    render::world_sprite::{WorldSprite, WorldSpriteMaterial, world_sprite_render_components},
    script::{
        BatchSubmissionItem, BatchSubmitMode, CharacterEase, ResolvedCharacterKeyframe,
        ScriptBootstrap, ScriptCommand, ScriptRequestId, ScriptResponse, ScriptResponseMessage,
        ScriptRuntimeState, StoryRuntime, StoryRuntimeEvent, UiContext, VoicePlaybackMode,
        compile_story_bytecode, evaluate_ui_component_named, save_runtime_slot,
        script_command_from_effect, start_hks_runtime,
    },
    state::{
        AudioSnapshot, ChoiceOption, DialogueSnapshot, ImageLayerSnapshot, SceneSharedState,
        SceneSnapshot, SpriteSnapshot, StoredValue, TextEffectSnapshot, UiStylePatch,
    },
    storage::{UserSettings, load_save_data, read_user_settings, write_user_settings},
    texture::{TextureAtlasCatalog, TextureCatalog, prepare_texture_atlases},
    ui::{
        BarNode, ButtonNode, ContainerNode, OverlayUiState, ScreenImageButtonNode, ScreenImageNode,
        ScreenLayout, ScreenNode, ScreenSpec, ScreenUiButton, ScreenUiButtonText,
        ScreenUiImageButton, ScreenUiNode, ScreenUiRoot, ScreenUiScrollable, ScreenUiState,
        ScreenUiToggle, SpacerNode, StaleScreenRoot, TextNode, UiAnimationPlayer, UiEnabledBinding,
        UiModels, UiProgressBinding, UiReactiveEnabledBinding, UiReactiveProgressBinding,
        UiReactiveTextBinding, UiReactiveVisibilityBinding, UiTextBinding, UiVisibilityBinding,
    },
    vfs::VfsResource,
};

mod animation_runtime;
mod audio_runtime;
mod character;
mod choice;
mod command_runtime;
mod dialogue;
mod runtime_menu;
mod screen_ui;
mod snapshot;

use animation_runtime::{ActiveScriptBatch, PendingAnimationWait, PendingWait, VisualTween};
pub use animation_runtime::{
    ActiveScriptBatches, AnimationState, PendingAnimationCancels, PendingWaits,
    animate_custom_effects, animate_rule_transitions, animate_visual_tweens,
    apply_animation_cancellations, tick_animation_waits, tick_pending_waits, tick_script_batches,
};
pub(crate) use animation_runtime::{complete_missing_animation, tween_fraction};
use audio_runtime::{
    BgmChannel, BgmFade, BgmPrelude, SfxChannel, VoiceChannel, apply_volume_setting,
    finish_active_voice, finish_all_voices,
};
pub use audio_runtime::{
    animate_bgm_fades, apply_live_audio_settings, poll_voice_playback, prepare_bgm_preludes,
    reconcile_restored_bgm,
};

pub(crate) use character::apply_character_ease;
pub use character::{
    animate_character_motion_effects, poll_pending_character_shows, reconcile_restored_characters,
};
use character::{
    apply_character_motion, apply_character_timeline, despawn_character_actor,
    queue_character_show, source_rect_from_corners, source_rect_to_corners,
};
pub(crate) use choice::{ChoiceButton, ChoiceUi};
pub use choice::{ChoiceState, handle_choice_action_input, handle_choice_buttons};
use choice::{clear_choice_ui, spawn_choice_ui};
use command_runtime::evaluate_ui_at;
pub use command_runtime::{PendingScriptCommands, bridge_story_events, process_script_commands};
pub use dialogue::{
    DialogueAdvanceSurface, DialogueCharSpan, DialogueHistoryState, DialogueRoot, DialogueState,
    DialogueTextEffect, HintText, LineText, PendingDialogueAdvance, SpeakerText,
    advance_dialogue_on_input, animate_dialogue_text_reveal,
};
use dialogue::{
    advance_dialogue, append_dialogue_line_text, append_dialogue_model_reveal,
    apply_text_effect_spec, clear_dialogue_spans, complete_dialogue_wait,
    dialogue_text_effect_from_snapshot, refresh_dialogue_ui_style, set_dialogue_line_text,
    set_dialogue_model_reveal, text_effect_snapshot,
};
use runtime_menu::parse_ui_action_route;
pub use runtime_menu::{
    PauseMenuRoot, RuntimeMenuButton, RuntimeMenuState, handle_runtime_menu_buttons,
    update_runtime_menu_button_visuals,
};
pub use screen_ui::{
    UiEffectMessage, animate_screen_ui, cleanup_stale_screen_ui, handle_screen_buttons,
    handle_screen_image_buttons, handle_screen_scroll, handle_screen_toggles, process_ui_effects,
    update_builtin_ui_models, update_ui_reactive_bindings, update_ui_text_bindings,
};
use screen_ui::{
    clear_overlay_ui, clear_screen_ui, screen_images_ready,
    should_clear_stale_screen_before_command, spawn_screen_ui,
};
use snapshot::restore_scene_snapshot;
pub use snapshot::sync_scene_snapshot;

const STAGE_Z_BACKGROUND: f32 = 0.0;
const STAGE_Z_SPRITE: f32 = 10.0;
const STAGE_Z_OVERLAY: f32 = 30.0;
const SCREEN_READY_FRAMES: u8 = 0;
const SCREEN_ACTIVE_Z: i32 = 100;
const SCREEN_MODAL_ACTIVE_Z: i32 = 120;
const SCREEN_MODAL_PENDING_Z: i32 = 119;
const SCREEN_MODAL_STALE_Z: i32 = 118;

const BUTTON_NORMAL: Color = Color::srgb(0.13, 0.15, 0.19);
const BUTTON_HOVERED: Color = Color::srgb(0.22, 0.26, 0.32);
const BUTTON_PRESSED: Color = Color::srgb(0.88, 0.74, 0.44);

#[derive(Resource, Clone)]
pub struct UiFonts {
    pub regular: Handle<Font>,
    pub _fonts: Vec<Handle<Font>>,
}

#[derive(Resource, Clone)]
pub struct UiStyle {
    pub dialogue_bg: Color,
    pub dialogue_border: Color,
    pub dialogue_left: f32,
    pub dialogue_right: f32,
    pub dialogue_bottom: f32,
    pub dialogue_min_height: f32,
    pub dialogue_padding_x: f32,
    pub dialogue_padding_y: f32,
    pub dialogue_radius: f32,
    pub speaker_size: f32,
    pub line_size: f32,
    pub hint_size: f32,
    pub hint_visible: bool,
    pub speaker_color: Color,
    pub line_color: Color,
    pub hint_color: Color,
    pub choice_panel_bg: Color,
    pub choice_bottom: f32,
    pub choice_panel_width: f32,
    pub choice_padding: f32,
    pub choice_gap: f32,
    pub choice_prompt_size: f32,
    pub choice_button_size: f32,
    pub choice_center_text: bool,
    pub choice_show_indices: bool,
    pub choice_prompt_color: Color,
    pub choice_button_bg: Color,
    pub choice_button_hovered: Color,
    pub choice_button_pressed: Color,
    pub choice_button_border: Color,
    pub choice_text_color: Color,
    pub quick_menu_bottom: f32,
    pub quick_menu_gap: f32,
    pub quick_button_size: f32,
    pub quick_menu_bg: Color,
    pub quick_button_bg: Color,
    pub quick_button_hovered: Color,
    pub quick_button_pressed: Color,
    pub quick_button_border: Color,
    pub quick_text_color: Color,
}

impl Default for UiStyle {
    fn default() -> Self {
        Self {
            dialogue_bg: Color::BLACK.with_alpha(0.82),
            dialogue_border: Color::WHITE.with_alpha(0.14),
            dialogue_left: 24.0,
            dialogue_right: 24.0,
            dialogue_bottom: 24.0,
            dialogue_min_height: 180.0,
            dialogue_padding_x: 28.0,
            dialogue_padding_y: 22.0,
            dialogue_radius: 18.0,
            speaker_size: 28.0,
            line_size: 34.0,
            hint_size: 18.0,
            hint_visible: true,
            speaker_color: Color::srgb(1.0, 0.9, 0.72),
            line_color: Color::WHITE,
            hint_color: Color::WHITE.with_alpha(0.55),
            choice_panel_bg: Color::BLACK.with_alpha(0.72),
            choice_bottom: 230.0,
            choice_panel_width: 0.0,
            choice_padding: 18.0,
            choice_gap: 12.0,
            choice_prompt_size: 22.0,
            choice_button_size: 26.0,
            choice_center_text: false,
            choice_show_indices: true,
            choice_prompt_color: Color::WHITE.with_alpha(0.82),
            choice_button_bg: BUTTON_NORMAL,
            choice_button_hovered: BUTTON_HOVERED,
            choice_button_pressed: BUTTON_PRESSED,
            choice_button_border: Color::WHITE.with_alpha(0.16),
            choice_text_color: Color::WHITE,
            quick_menu_bottom: 8.0,
            quick_menu_gap: 0.0,
            quick_button_size: 14.0,
            quick_menu_bg: Color::BLACK.with_alpha(0.0),
            quick_button_bg: Color::BLACK.with_alpha(0.0),
            quick_button_hovered: Color::BLACK.with_alpha(0.0),
            quick_button_pressed: BUTTON_PRESSED,
            quick_button_border: Color::BLACK.with_alpha(0.0),
            quick_text_color: Color::WHITE.with_alpha(0.75),
        }
    }
}

#[derive(Resource, Default)]
pub struct FrontendState {
    pub startup_script: String,
    pub notice: Option<String>,
    pub runtime_started: bool,
}

#[derive(Resource, Default)]
pub struct StageState {
    pub background: Option<Entity>,
    pub overlay: Option<Entity>,
    pub transition: Option<Entity>,
    pub screen_effect: Option<Entity>,
    pub sprites: HashMap<String, Entity>,
    pub character_roots: HashMap<String, Entity>,
    pub character_active_parts: HashMap<String, HashSet<String>>,
    pub character_positions: HashMap<String, Vec2>,
    /// Logical restored parts waiting to be reconciled into render entities.
    /// This keeps mask/blend metadata intact while their assets are loading.
    pub pending_character_restore: Vec<SpriteSnapshot>,
    pub pending_bgm_restore: Option<AudioSnapshot>,
    pub bgm: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct VoiceState {
    pub active: Option<ActiveVoice>,
    pub concurrent: HashMap<Entity, ActiveVoice>,
}

#[derive(Resource, Default)]
pub struct PendingCharacterShows {
    pub items: Vec<PendingCharacterShow>,
}

pub struct ActiveVoice {
    pub entity: Entity,
    pub animation_id: Option<String>,
}

pub struct PendingCharacterShow {
    pub actor_id: String,
    pub entity_ids: Vec<String>,
    pub entities: Vec<Entity>,
    pub handles: Vec<Handle<Image>>,
    /// Tracks whether each pending entity was instantiated by this commit.
    /// Reused hidden children stay cached when a load or animation is cancelled.
    pub newly_spawned: Vec<bool>,
    /// Previously visible parts retained until all replacements are renderable.
    pub outgoing: Vec<(String, Entity)>,
    pub fade: Option<std::time::Duration>,
    pub animation_id: Option<String>,
}

#[derive(Component, Clone, Debug)]
pub struct CharacterRoot {
    pub actor_id: String,
}

#[derive(Component)]
pub struct HideAfterTween;

#[derive(Component)]
pub struct CharacterJumpEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub height: f32,
    pub animation_id: Option<String>,
}

#[derive(Component)]
pub struct CharacterShakeEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub amplitude: f32,
    pub animation_id: Option<String>,
}

#[derive(Component)]
pub struct CharacterTimelineEffect {
    pub origin: Vec3,
    pub actor_id: String,
    pub actor_origin: Vec2,
    pub keyframes: Vec<ResolvedCharacterKeyframe>,
    pub elapsed: f32,
    pub duration: f32,
    pub animation_id: Option<String>,
}

#[derive(Clone, Copy)]
pub enum CharacterMotionKind {
    Jump { height: f32 },
    Shake { amplitude: f32 },
}

#[derive(Component)]
pub struct OverlayMarker;

#[derive(Component, Clone)]
pub struct SpriteActor {
    pub id: String,
    pub path: String,
}

#[derive(Component, Clone, Copy)]
pub struct FocusedActorPart;

#[derive(Component, Clone)]
pub struct BackgroundLayer {
    pub path: String,
}

pub fn setup_stage(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut world_sprite_materials: ResMut<Assets<WorldSpriteMaterial>>,
    config: Res<crate::RuntimeLaunchConfig>,
    mut shared_state: ResMut<SceneSharedState>,
) {
    setup_stage_cameras(&mut commands, &mut images, &config);
    commands.insert_resource(RuleTransitionMesh(meshes.add(Rectangle::default())));

    let overlay_sprite =
        WorldSprite::from_color(Color::BLACK.with_alpha(0.0), Vec2::new(6000.0, 6000.0));
    let overlay_render =
        world_sprite_render_components(&overlay_sprite, &mut meshes, &mut world_sprite_materials);
    let overlay = commands
        .spawn((
            OverlayMarker,
            overlay_sprite,
            overlay_render,
            Transform::from_xyz(0.0, 0.0, STAGE_Z_OVERLAY),
            focus_layer(),
        ))
        .id();

    commands.insert_resource(StageState {
        overlay: Some(overlay),
        ..default()
    });
    commands.insert_resource(DialogueState::default());
    commands.insert_resource(DialogueHistoryState::default());
    commands.insert_resource(ChoiceState::default());
    commands.insert_resource(PendingWaits::default());
    commands.insert_resource(CameraShakeState::default());
    commands.insert_resource(CameraState::default());
    commands.insert_resource(CameraTweenState::default());
    commands.insert_resource(AnimationState::default());
    commands.insert_resource(PendingAnimationCancels::default());
    commands.insert_resource(PendingScriptCommands::default());
    commands.insert_resource(ActiveScriptBatches::default());
    commands.insert_resource(VoiceState::default());
    commands.insert_resource(PendingCharacterShows::default());
    commands.insert_resource(ScreenUiState::default());

    commands.spawn((
        DialogueAdvanceSurface,
        Node {
            position_type: PositionType::Absolute,
            left: px(0.0),
            right: px(0.0),
            top: px(0.0),
            bottom: px(0.0),
            ..default()
        },
        GlobalZIndex(-10_000),
        Pickable::default(),
        ui_layer(),
    ));

    commands.insert_resource(RuntimeMenuState::default());
    commands.insert_resource(OverlayUiState::default());

    let mut snapshot = SceneSnapshot::default();
    snapshot.text_effect = text_effect_snapshot(&DialogueTextEffect::default());
    shared_state.0 = snapshot;
}

pub fn setup_frontend(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vfs: Res<VfsResource>,
) {
    prepare_texture_atlases(&mut commands, &asset_server, &vfs.0);
    match load_audio_catalog(&vfs.0) {
        Ok(catalog) => commands.insert_resource(catalog),
        Err(error) => {
            warn!("failed to load audio catalog: {error}");
            commands.insert_resource(AudioCatalog::default());
        }
    }
    match load_term_catalog(&vfs.0) {
        Ok(catalog) => commands.insert_resource(catalog),
        Err(error) => {
            warn!("failed to load glossary: {error}");
            commands.insert_resource(TermCatalog::default());
        }
    }
    let startup_script = match vfs.0.load_startup_script_path() {
        Ok(startup_script) => startup_script,
        Err(err) => {
            warn!("failed to resolve startup script: {err}");
            String::new()
        }
    };

    let user_settings = match read_user_settings() {
        Ok(settings) => settings,
        Err(err) => {
            warn!("failed to read user settings: {err}");
            UserSettings::default()
        }
    };

    let font_paths = match vfs.0.load_font_paths() {
        Ok(paths) => paths,
        Err(err) => {
            warn!("failed to enumerate fonts directory: {err}");
            Vec::new()
        }
    };

    let character_catalog = match load_character_catalog(&vfs.0) {
        Ok(catalog) => catalog,
        Err(err) => {
            warn!("failed to load character catalog: {err}");
            CharacterCatalog::default()
        }
    };

    let mut frontend = FrontendState {
        startup_script,
        runtime_started: true,
        ..default()
    };

    if frontend.startup_script.is_empty() {
        frontend.notice =
            Some("startup.hks not found. Fix settings.hson before starting.".to_string());
    }

    commands.insert_resource(UiFonts {
        regular: font_paths
            .first()
            .map(|path| asset_server.load(path.clone()))
            .unwrap_or_default(),
        _fonts: font_paths
            .into_iter()
            .map(|path| asset_server.load(path))
            .collect(),
    });
    commands.insert_resource(UiStyle::default());
    commands.insert_resource(character_catalog);
    commands.insert_resource(user_settings);
    commands.insert_resource(frontend);
}

/// Picking targets the deepest visible UI entity. Resolve through the hierarchy
/// so labels and decorative children retain the interaction semantics of their
/// owning button or modal root.
fn find_component_ancestor<T: bevy::ecs::query::QueryData, F: bevy::ecs::query::QueryFilter>(
    mut entity: Entity,
    components: &Query<T, F>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    loop {
        if components.contains(entity) {
            return Some(entity);
        }
        entity = parents.get(entity).ok()?.parent();
    }
}

fn adjusted_volume(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(0.0, 1.0)
}

fn apply_ui_style_patch(ui_style: &mut UiStyle, patch: UiStylePatch) {
    if let Some(color) = patch.dialogue_bg {
        ui_style.dialogue_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.dialogue_border {
        ui_style.dialogue_border = color_from_rgba(color);
    }
    if let Some(value) = patch.dialogue_left {
        ui_style.dialogue_left = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_right {
        ui_style.dialogue_right = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_bottom {
        ui_style.dialogue_bottom = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_min_height {
        ui_style.dialogue_min_height = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_padding_x {
        ui_style.dialogue_padding_x = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_padding_y {
        ui_style.dialogue_padding_y = value.max(0.0);
    }
    if let Some(value) = patch.dialogue_radius {
        ui_style.dialogue_radius = value.max(0.0);
    }
    if let Some(value) = patch.speaker_size {
        ui_style.speaker_size = value.max(1.0);
    }
    if let Some(value) = patch.line_size {
        ui_style.line_size = value.max(1.0);
    }
    if let Some(value) = patch.hint_size {
        ui_style.hint_size = value.max(1.0);
    }
    if let Some(value) = patch.hint_visible {
        ui_style.hint_visible = value;
    }
    if let Some(color) = patch.speaker_color {
        ui_style.speaker_color = color_from_rgba(color);
    }
    if let Some(color) = patch.line_color {
        ui_style.line_color = color_from_rgba(color);
    }
    if let Some(color) = patch.hint_color {
        ui_style.hint_color = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_panel_bg {
        ui_style.choice_panel_bg = color_from_rgba(color);
    }
    if let Some(value) = patch.choice_bottom {
        ui_style.choice_bottom = value.max(0.0);
    }
    if let Some(value) = patch.choice_panel_width {
        ui_style.choice_panel_width = value.max(0.0);
    }
    if let Some(value) = patch.choice_padding {
        ui_style.choice_padding = value.max(0.0);
    }
    if let Some(value) = patch.choice_gap {
        ui_style.choice_gap = value.max(0.0);
    }
    if let Some(value) = patch.choice_prompt_size {
        ui_style.choice_prompt_size = value.max(1.0);
    }
    if let Some(value) = patch.choice_button_size {
        ui_style.choice_button_size = value.max(1.0);
    }
    if let Some(value) = patch.choice_center_text {
        ui_style.choice_center_text = value;
    }
    if let Some(value) = patch.choice_show_indices {
        ui_style.choice_show_indices = value;
    }
    if let Some(color) = patch.choice_prompt_color {
        ui_style.choice_prompt_color = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_bg {
        ui_style.choice_button_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_hovered {
        ui_style.choice_button_hovered = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_pressed {
        ui_style.choice_button_pressed = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_border {
        ui_style.choice_button_border = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_text_color {
        ui_style.choice_text_color = color_from_rgba(color);
    }
    if let Some(value) = patch.quick_menu_bottom {
        ui_style.quick_menu_bottom = value.max(0.0);
    }
    if let Some(value) = patch.quick_menu_gap {
        ui_style.quick_menu_gap = value.max(0.0);
    }
    if let Some(value) = patch.quick_button_size {
        ui_style.quick_button_size = value.max(1.0);
    }
    if let Some(color) = patch.quick_menu_bg {
        ui_style.quick_menu_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.quick_button_bg {
        ui_style.quick_button_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.quick_button_hovered {
        ui_style.quick_button_hovered = color_from_rgba(color);
    }
    if let Some(color) = patch.quick_button_pressed {
        ui_style.quick_button_pressed = color_from_rgba(color);
    }
    if let Some(color) = patch.quick_button_border {
        ui_style.quick_button_border = color_from_rgba(color);
    }
    if let Some(color) = patch.quick_text_color {
        ui_style.quick_text_color = color_from_rgba(color);
    }
}

fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn align_items_from_align(value: f32) -> AlignItems {
    if value <= 0.25 {
        AlignItems::FlexStart
    } else if value >= 0.75 {
        AlignItems::FlexEnd
    } else {
        AlignItems::Center
    }
}

fn ui_text_font(ui_fonts: &UiFonts, font_size: f32) -> TextFont {
    TextFont::from_font_size(font_size).with_font(ui_fonts.regular.clone())
}
