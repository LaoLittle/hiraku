use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use bevy::{
    app::AppExit,
    audio::{AudioSink, AudioSinkPlayback, Volume},
    ecs::system::SystemParam,
    log::{info, warn},
    picking::hover::PickingInteraction,
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
    storage::{
        SaveSlotSummary, StorageError, UserSettings, list_save_slots, load_save_data,
        read_user_settings, write_user_settings,
    },
    texture::{TextureAtlasCatalog, TextureCatalog, prepare_texture_atlases},
    ui::{
        BarNode, ButtonNode, ContainerNode, OverlayUiState, ScreenImageButtonNode, ScreenImageNode,
        ScreenLayout, ScreenNode, ScreenSpec, ScreenUiButton, ScreenUiButtonText,
        ScreenUiImageButton, ScreenUiNode, ScreenUiRoot, ScreenUiState, SpacerNode,
        StaleScreenRoot, TextNode, UiSignals, UiTextBinding,
    },
    vfs::VfsResource,
};

mod character;
mod screen_ui;

pub(crate) use character::apply_character_ease;
pub use character::{animate_character_motion_effects, poll_pending_character_shows};
use character::{
    apply_character_motion, apply_character_timeline, despawn_character_actor,
    queue_character_show, source_rect_from_corners, source_rect_to_corners,
};
pub use screen_ui::{
    cleanup_stale_screen_ui, handle_screen_buttons, handle_screen_image_buttons,
    update_builtin_ui_signals, update_ui_text_bindings,
};
use screen_ui::{
    clear_overlay_ui, clear_screen_ui, screen_images_ready,
    should_clear_stale_screen_before_command, spawn_screen_ui,
};

const STAGE_Z_BACKGROUND: f32 = 0.0;
const STAGE_Z_SPRITE: f32 = 10.0;
const STAGE_Z_OVERLAY: f32 = 30.0;
const SCREEN_READY_FRAMES: u8 = 0;
const SCREEN_PENDING_Z: i32 = 90;
const SCREEN_ACTIVE_Z: i32 = 100;
const SCREEN_STALE_Z: i32 = 80;

const BUTTON_NORMAL: Color = Color::srgb(0.13, 0.15, 0.19);
const BUTTON_HOVERED: Color = Color::srgb(0.22, 0.26, 0.32);
const BUTTON_PRESSED: Color = Color::srgb(0.88, 0.74, 0.44);
const FRONTEND_BG: Color = Color::srgb(0.06, 0.08, 0.11);
const FRONTEND_PANEL: Color = Color::srgb(0.12, 0.09, 0.08);
const FRONTEND_PANEL_ALT: Color = Color::srgb(0.17, 0.13, 0.11);
const FRONTEND_BUTTON: Color = Color::srgb(0.24, 0.18, 0.13);
const FRONTEND_BUTTON_HOVERED: Color = Color::srgb(0.34, 0.24, 0.16);
const FRONTEND_BUTTON_PRESSED: Color = Color::srgb(0.84, 0.61, 0.30);

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrontendScreen {
    #[default]
    Title,
    Load,
    Settings,
    InGame,
}

#[derive(Resource, Default)]
pub struct FrontendState {
    pub startup_script: String,
    pub gallery_script: String,
    pub root: Option<Entity>,
    pub screen: FrontendScreen,
    pub saves: Vec<SaveSlotSummary>,
    pub notice: Option<String>,
    pub runtime_started: bool,
    pub dirty: bool,
}

#[derive(Resource, Default)]
pub struct StageState {
    pub background: Option<Entity>,
    pub overlay: Option<Entity>,
    pub transition: Option<Entity>,
    pub screen_effect: Option<Entity>,
    pub sprites: HashMap<String, Entity>,
    pub character_positions: HashMap<String, Vec2>,
    pub bgm: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct DialogueState {
    pub waiting: Option<PendingDialogueAdvance>,
    pub span_entities: Vec<Entity>,
    pub reveal: Option<DialogueRevealState>,
    pub effect: DialogueTextEffect,
}

pub struct PendingDialogueAdvance {
    pub animation_id: Option<String>,
    pub request: Option<ScriptRequestId>,
}

pub struct DialogueRevealState {
    pub spans: Vec<Entity>,
    pub next_index: usize,
    pub accumulator: f32,
    pub interval: f32,
    pub fade_seconds: f32,
    pub animation_id: Option<String>,
}

#[derive(Clone)]
pub struct DialogueTextEffect {
    pub mode: DialogueTextEffectMode,
    pub cps: f32,
    pub fade_seconds: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogueTextEffectMode {
    Instant,
    TypewriterFade,
}

impl Default for DialogueTextEffect {
    fn default() -> Self {
        Self {
            mode: DialogueTextEffectMode::TypewriterFade,
            cps: 30.0,
            fade_seconds: 0.12,
        }
    }
}

#[derive(Component)]
pub struct DialogueCharSpan {
    pub target_alpha: f32,
    pub age: f32,
    pub revealed: bool,
}

#[derive(Resource, Default)]
pub struct ChoiceState {
    pub waiting: Option<ScriptRequestId>,
    pub options: Vec<ChoiceOption>,
}

#[derive(Resource, Default)]
pub struct PendingWaits {
    pub items: Vec<PendingWait>,
}

#[derive(Resource, Default)]
pub struct AnimationState {
    pub completed: HashSet<String>,
    pub waits: Vec<PendingAnimationWait>,
}

#[derive(Resource, Default)]
pub struct PendingAnimationCancels {
    pub ids: Vec<String>,
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

#[derive(Resource, Default)]
pub struct PendingScriptCommands {
    pub items: VecDeque<ScriptCommand>,
}

fn stored_to_hks(value: StoredValue) -> hiraku_script::Value {
    match value {
        StoredValue::Bool(value) => hiraku_script::Value::Bool(value),
        StoredValue::Int(value) => hiraku_script::Value::Number(value as f64),
        StoredValue::Float(value) => hiraku_script::Value::Number(value),
        StoredValue::String(value) => hiraku_script::Value::String(value),
        StoredValue::Array(values) => {
            hiraku_script::Value::List(values.into_iter().map(stored_to_hks).collect())
        }
        StoredValue::Map(values) => hiraku_script::Value::Map(
            values
                .into_iter()
                .map(|(name, value)| (name, stored_to_hks(value)))
                .collect(),
        ),
    }
}

fn hks_globals_to_stored(
    globals: &BTreeMap<String, hiraku_script::Value>,
) -> BTreeMap<String, StoredValue> {
    globals
        .iter()
        .filter_map(|(name, value)| hks_to_stored(value).map(|value| (name.clone(), value)))
        .collect()
}

fn hks_to_stored(value: &hiraku_script::Value) -> Option<StoredValue> {
    match value {
        hiraku_script::Value::Bool(value) => Some(StoredValue::Bool(*value)),
        hiraku_script::Value::Number(value) => Some(StoredValue::Float(*value)),
        hiraku_script::Value::String(value) | hiraku_script::Value::Symbol(value) => {
            Some(StoredValue::String(value.clone()))
        }
        hiraku_script::Value::List(values) | hiraku_script::Value::Tuple(values) => Some(
            StoredValue::Array(values.iter().filter_map(hks_to_stored).collect()),
        ),
        hiraku_script::Value::Map(values) => Some(StoredValue::Map(
            values
                .iter()
                .filter_map(|(name, value)| hks_to_stored(value).map(|value| (name.clone(), value)))
                .collect(),
        )),
        hiraku_script::Value::Typed { value, .. } => hks_to_stored(value),
        _ => None,
    }
}

fn evaluate_ui_at(
    target: &str,
    runtime: &ScriptRuntimeState,
    vfs: &VfsResource,
    user_settings: &UserSettings,
    textures: Option<&TextureCatalog>,
    terms: Option<&TermCatalog>,
) -> Result<ScreenSpec, String> {
    let mut values = runtime
        .story
        .as_ref()
        .map(|story| hks_globals_to_stored(story.globals()))
        .unwrap_or_default();
    values.insert(
        "bgmVolume".to_string(),
        StoredValue::Float(user_settings.bgm_volume as f64),
    );
    values.insert(
        "voiceVolume".to_string(),
        StoredValue::Float(user_settings.voice_volume as f64),
    );
    values.insert(
        "sfxVolume".to_string(),
        StoredValue::Float(user_settings.sfx_volume as f64),
    );
    let source = vfs.0.read_text(target).map_err(|error| error.to_string())?;
    let textures = textures.ok_or_else(|| "texture catalog is unavailable".to_string())?;
    let terms = terms.ok_or_else(|| "term catalog is unavailable".to_string())?;
    evaluate_ui_component_named(target, &source, UiContext::new(values), textures, terms)
        .map_err(|error| error.to_string())
}

pub fn bridge_story_events(
    mut runtime: ResMut<ScriptRuntimeState>,
    mut response_messages: MessageReader<ScriptResponseMessage>,
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    textures: Option<Res<TextureCatalog>>,
    terms: Option<Res<TermCatalog>>,
    audio: Option<Res<AudioCatalog>>,
    vfs: Res<VfsResource>,
    user_settings: Res<UserSettings>,
) {
    for message in response_messages.read() {
        if let Some(task) = runtime.task_requests.remove(&message.request) {
            if let Some(story) = runtime.story.as_mut()
                && let Err(error) = story.resume_task(task)
            {
                warn!("failed to resume HKS task {task}: {error}");
                runtime.story = None;
            }
        } else {
            runtime.accept_response(message.clone());
        }
    }

    if let Some(request) = runtime.wait_request
        && let Some(response) = runtime.take_response(request)
    {
        let direct_value = match &response {
            ScriptResponse::Choice(value) => stored_to_hks(value.clone()),
            ScriptResponse::Continue => hiraku_script::Value::Null,
        };
        runtime.pending_ui_screen = None;
        runtime.wait_request = None;
        if let Some(story) = runtime.story.as_mut()
            && let Err(error) = story.resume(direct_value)
        {
            warn!("failed to resume script runtime: {error}");
            runtime.story = None;
        }
    }

    if let Some(event) = runtime.story_events.pop_front() {
        match event {
            StoryRuntimeEvent::Effect(
                effect @ (crate::script::capabilities::StoryEffect::GotoScript { .. }
                | crate::script::capabilities::StoryEffect::CallScript { .. }),
            ) => {
                let (path, is_call) = match effect {
                    crate::script::capabilities::StoryEffect::GotoScript { path } => (path, false),
                    crate::script::capabilities::StoryEffect::CallScript { path } => (path, true),
                    _ => unreachable!("matched script transfer effects above"),
                };
                let target = vfs.0.resolve_path(runtime.current_script.as_deref(), &path);
                let inherited_globals = runtime
                    .story
                    .as_ref()
                    .map(|story| story.globals().clone())
                    .unwrap_or_default();
                let result = vfs
                    .0
                    .read_text(&target)
                    .map_err(|error| error.to_string())
                    .and_then(|source| compile_story_bytecode(&target, &source))
                    .and_then(|bytecode| {
                        StoryRuntime::new(bytecode).map_err(|error| error.to_string())
                    });
                match result {
                    Ok(mut story) => {
                        let mut globals = inherited_globals;
                        globals.extend(crate::script::capabilities::engine_globals(&user_settings));
                        story.set_globals(globals);
                        if is_call {
                            if let (Some(script), Some(caller)) =
                                (runtime.current_script.take(), runtime.story.take())
                            {
                                runtime.call_stack.push(crate::script::ScriptCallFrame {
                                    script,
                                    story: caller,
                                });
                            }
                        } else {
                            runtime.call_stack.clear();
                        }
                        runtime.story = Some(story);
                        runtime.current_script = Some(target);
                        runtime.story_events.clear();
                        runtime.task_requests.clear();
                    }
                    Err(error) => crate::script::emit_script_diagnostic(
                        &format!("failed to load HKS script `{target}`:"),
                        &error,
                    ),
                }
            }
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::PlayBgm {
                path,
                volume,
                fade_in_ms,
            }) => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_music(&path))
            {
                Some(definition) => {
                    pending_script_commands
                        .items
                        .push_back(ScriptCommand::PlayBgm {
                            path: definition.path.clone(),
                            prelude: definition.prelude.clone(),
                            volume,
                            fade_in: fade_in_ms.map(std::time::Duration::from_millis),
                            animation_id: None,
                        })
                }
                None => warn!("music `{path}` is not defined"),
            },
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::PlayVoice {
                path,
                volume,
            }) => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_voice(&path))
            {
                Some(definition) => {
                    pending_script_commands
                        .items
                        .push_back(ScriptCommand::PlayVoice {
                            path: definition.path.clone(),
                            volume,
                            mode: VoicePlaybackMode::Exclusive,
                            animation_id: None,
                        })
                }
                None => warn!("voice `{path}` is not defined"),
            },
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::SetUiRole {
                role,
                component,
            }) => {
                let target = vfs
                    .0
                    .resolve_path(runtime.current_script.as_deref(), &component);
                runtime.ui_registry.insert(role, target);
            }
            StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::MountUiOverlay { name, component },
            ) => {
                let target = runtime
                    .ui_registry
                    .get(&component)
                    .cloned()
                    .unwrap_or_else(|| {
                        vfs.0
                            .resolve_path(runtime.current_script.as_deref(), &component)
                    });
                let overlay = evaluate_ui_at(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                );
                match overlay {
                    Ok(screen) => {
                        runtime.mounted_ui_overlays.insert(name.clone(), component);
                        pending_script_commands
                            .items
                            .push_back(ScriptCommand::ShowOverlay { name, screen });
                    }
                    Err(error) => {
                        warn!("failed to mount UI overlay `{name}` from `{target}`: {error}")
                    }
                }
            }
            StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::UnmountUiOverlay { name },
            ) => {
                runtime.mounted_ui_overlays.remove(&name);
                pending_script_commands
                    .items
                    .push_back(ScriptCommand::HideOverlay { name });
            }
            StoryRuntimeEvent::Effect(effect) => {
                match script_command_from_effect(effect, textures.as_deref()) {
                    Ok(command) => pending_script_commands.items.push_back(command),
                    Err(error) => warn!("HKS native command rejected: {error}"),
                }
            }
            StoryRuntimeEvent::Wait(crate::script::capabilities::StoryWait::DialogueAdvance) => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                pending_script_commands
                    .items
                    .push_back(ScriptCommand::AwaitDialogueAdvance { done: request });
            }
            StoryRuntimeEvent::Choice { prompt, options } => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                pending_script_commands
                    .items
                    .push_back(ScriptCommand::Choose {
                        prompt,
                        options: options
                            .into_iter()
                            .enumerate()
                            .map(|(index, text)| ChoiceOption {
                                text,
                                value: StoredValue::Int(index as i64),
                            })
                            .collect(),
                        done: request,
                    });
            }
            StoryRuntimeEvent::OpenUi { path } => {
                let target = runtime.ui_registry.get(&path).cloned().unwrap_or_else(|| {
                    vfs.0.resolve_path(runtime.current_script.as_deref(), &path)
                });
                let screen = evaluate_ui_at(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                );
                let request = runtime.allocate_request();
                runtime.pending_ui_screen = Some(target.clone());
                runtime.wait_request = Some(request);
                match screen {
                    Ok(screen) => {
                        pending_script_commands
                            .items
                            .push_back(ScriptCommand::ShowScreen {
                                screen,
                                done: Some(request),
                            })
                    }
                    Err(error) => {
                        warn!("failed to render UI script `{target}`: {error}");
                        runtime.story = None;
                        runtime.wait_request = None;
                    }
                }
            }
            StoryRuntimeEvent::TaskEffect {
                task,
                effect: crate::script::capabilities::StoryEffect::PlayVoice { path, volume },
            } => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_voice(&path))
            {
                Some(definition) => {
                    let request = runtime.allocate_request();
                    let animation_id = format!("hks-task-voice-{}", request.0);
                    runtime.task_requests.insert(request, task);
                    pending_script_commands
                        .items
                        .push_back(ScriptCommand::PlayVoice {
                            path: definition.path.clone(),
                            volume,
                            mode: VoicePlaybackMode::Concurrent,
                            animation_id: Some(animation_id.clone()),
                        });
                    pending_script_commands
                        .items
                        .push_back(ScriptCommand::WaitAnimations {
                            ids: vec![animation_id],
                            done: request,
                        });
                }
                None => {
                    warn!("voice `{path}` is not defined");
                    if let Some(story) = runtime.story.as_mut()
                        && let Err(error) = story.resume_task(task)
                    {
                        warn!("failed to skip missing HKS task voice: {error}");
                    }
                }
            },
            StoryRuntimeEvent::TaskEffect { task, effect } => {
                warn!("unsupported HKS task effect for task {task}: {effect:?}");
                if let Some(story) = runtime.story.as_mut()
                    && let Err(error) = story.resume_task(task)
                {
                    warn!("failed to resume unsupported HKS task effect: {error}");
                }
            }
            StoryRuntimeEvent::Completed(_) => {
                if let Some(frame) = runtime.call_stack.pop() {
                    let globals = runtime
                        .story
                        .as_ref()
                        .map(|story| story.globals().clone())
                        .unwrap_or_default();
                    let mut caller = frame.story;
                    caller.set_globals(globals);
                    runtime.story = Some(caller);
                    runtime.current_script = Some(frame.script);
                    runtime.story_events.clear();
                    runtime.task_requests.clear();
                }
            }
        }
        return;
    }
}

#[derive(Resource, Default)]
pub struct ActiveScriptBatches {
    pub items: Vec<ActiveScriptBatch>,
}

pub struct ActiveScriptBatch {
    pub remaining: VecDeque<BatchSubmissionItem>,
    pub current_handle: String,
}

pub struct PendingWait {
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: ScriptRequestId,
}

pub struct PendingAnimationWait {
    pub ids: Vec<String>,
    pub done: ScriptRequestId,
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
    /// Previously visible parts retained until all replacements are renderable.
    pub outgoing: Vec<(String, Entity)>,
    pub fade: Option<std::time::Duration>,
    pub animation_id: Option<String>,
}

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

#[derive(Component)]
pub struct SpeakerText;

#[derive(Component)]
pub struct LineText;

#[derive(Component)]
pub struct HintText;

#[derive(Component)]
pub struct DialogueRoot;

/// Lowest UI picking layer. It makes blank canvas clicks available for dialogue advancement
/// without teaching the host application about story state.
#[derive(Component)]
pub struct DialogueAdvanceSurface;

#[derive(Component)]
pub struct ChoiceUi;

#[derive(Component)]
pub struct ChoiceButton {
    pub index: usize,
}

#[derive(Component)]
pub struct PauseMenuRoot;

#[derive(Component)]
pub struct RuntimeMenuButton {
    pub action: RuntimeMenuButtonAction,
}

#[derive(Clone, Copy)]
pub enum RuntimeMenuButtonAction {
    QuickSave,
    QuickLoad,
    OpenPauseMenu,
    Resume,
    ReturnToTitle,
}

#[derive(Resource, Default)]
pub struct RuntimeMenuState {
    pub pause_root: Option<Entity>,
    pub pause_open: bool,
}

#[derive(Component)]
pub struct FrontendRoot;

#[derive(Component)]
pub struct FrontendButton {
    pub action: FrontendAction,
}

#[derive(Clone)]
pub enum FrontendAction {
    StartNewGame,
    StartCharacterGallery,
    OpenLoad,
    OpenSettings,
    BackToTitle,
    LoadSlot(String),
    AdjustBgm(f32),
    AdjustVoice(f32),
    AdjustSfx(f32),
}

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

#[derive(Component, Clone)]
pub struct BgmChannel {
    pub path: String,
    pub volume: f32,
}

/// Holds both file handles until the continuous prelude/loop source can be built.
#[derive(Component)]
pub struct BgmPrelude {
    pub prelude_audio: Handle<AudioSource>,
    pub loop_audio: Handle<AudioSource>,
    pub start_volume: f32,
}

#[derive(Component)]
pub struct BgmFade {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub animation_id: Option<String>,
}

#[derive(Component)]
#[expect(dead_code, reason = "voice metadata is kept for future UI/debug hooks")]
pub struct VoiceChannel {
    pub path: String,
    pub volume: f32,
}

#[derive(Component)]
pub struct SfxChannel {
    pub volume: f32,
}

#[derive(Component)]
pub struct VisualTween {
    pub from_alpha: Option<f32>,
    pub to_alpha: Option<f32>,
    pub from_translation: Option<Vec3>,
    pub to_translation: Option<Vec3>,
    pub from_scale: Option<Vec3>,
    pub to_scale: Option<Vec3>,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub despawn_on_finish: bool,
}

#[derive(SystemParam)]
pub struct SceneCommandContext<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub app_exit: MessageWriter<'w, AppExit>,
    pub asset_server: Res<'w, AssetServer>,
    pub images: Res<'w, Assets<Image>>,
    pub texture_atlases: Res<'w, TextureAtlasCatalog>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: ResMut<'w, SceneSharedState>,
    pub characters: Res<'w, CharacterCatalog>,
    pub user_settings: ResMut<'w, UserSettings>,
    pub ui_fonts: Res<'w, UiFonts>,
    pub ui_style: ResMut<'w, UiStyle>,
    pub frontend: ResMut<'w, FrontendState>,
    pub stage: ResMut<'w, StageState>,
    pub waits: ResMut<'w, PendingWaits>,
    pub pending_cancels: ResMut<'w, PendingAnimationCancels>,
    pub camera_state: ResMut<'w, CameraState>,
    pub camera_tweens: ResMut<'w, CameraTweenState>,
    pub pending_script_commands: ResMut<'w, PendingScriptCommands>,
    pub script_runtime: ResMut<'w, ScriptRuntimeState>,
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub animations: ResMut<'w, AnimationState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub transition_mesh: Res<'w, RuleTransitionMesh>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub alpha_mask_materials: ResMut<'w, Assets<AlphaMaskMaterial>>,
    pub multiply_materials: ResMut<'w, Assets<MultiplyMaterial>>,
    pub world_sprite_materials: ResMut<'w, Assets<WorldSpriteMaterial>>,
    pub custom_effect_materials: ResMut<'w, Assets<CustomScreenEffectMaterial>>,
    pub rule_materials: ResMut<'w, Assets<RuleTransitionMaterial>>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root:
        Query<'w, 's, &'static mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    pub dialogue_root_node: Query<'w, 's, &'static mut Node, With<DialogueRoot>>,
    pub dialogue_background: Query<'w, 's, &'static mut BackgroundColor, With<DialogueRoot>>,
    pub dialogue_border: Query<'w, 's, &'static mut BorderColor, With<DialogueRoot>>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub line_text_entity: Query<'w, 's, Entity, (With<LineText>, Without<SpeakerText>)>,
    pub speaker_font: Query<'w, 's, &'static mut TextFont, (With<SpeakerText>, Without<LineText>)>,
    pub line_font: Query<'w, 's, &'static mut TextFont, (With<LineText>, Without<SpeakerText>)>,
    pub hint_font: Query<
        'w,
        's,
        &'static mut TextFont,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
    pub hint_visibility:
        Query<'w, 's, &'static mut Visibility, (With<HintText>, Without<DialogueRoot>)>,
    pub speaker_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<SpeakerText>, Without<LineText>, Without<HintText>),
    >,
    pub line_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<LineText>, Without<SpeakerText>, Without<HintText>),
    >,
    pub hint_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
}

#[derive(SystemParam)]
pub struct RuntimeMenuContext<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub asset_server: Res<'w, AssetServer>,
    pub ui_fonts: Res<'w, UiFonts>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: ResMut<'w, SceneSharedState>,
    pub script_runtime: ResMut<'w, ScriptRuntimeState>,
    pub frontend: ResMut<'w, FrontendState>,
    pub user_settings: Res<'w, UserSettings>,
    pub ui_style: Res<'w, UiStyle>,
    pub runtime_menu: ResMut<'w, RuntimeMenuState>,
    pub stage: ResMut<'w, StageState>,
    pub waits: ResMut<'w, PendingWaits>,
    pub pending_script_commands: ResMut<'w, PendingScriptCommands>,
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub animations: ResMut<'w, AnimationState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root:
        Query<'w, 's, &'static mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub interaction_query: Query<
        'w,
        's,
        (
            &'static PickingInteraction,
            &'static mut BackgroundColor,
            &'static RuntimeMenuButton,
        ),
        Changed<PickingInteraction>,
    >,
}

pub fn setup_stage(
    mut commands: Commands,
    ui_fonts: Res<UiFonts>,
    ui_style: Res<UiStyle>,
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

    let dialogue_root = commands
        .spawn((
            DialogueRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(ui_style.dialogue_left),
                right: px(ui_style.dialogue_right),
                bottom: px(ui_style.dialogue_bottom),
                min_height: px(ui_style.dialogue_min_height),
                border: UiRect::all(px(1.0)),
                padding: UiRect::axes(
                    px(ui_style.dialogue_padding_x),
                    px(ui_style.dialogue_padding_y),
                ),
                border_radius: BorderRadius::all(px(ui_style.dialogue_radius)),
                flex_direction: FlexDirection::Column,
                row_gap: px(12.0),
                ..default()
            },
            BackgroundColor(ui_style.dialogue_bg),
            BorderColor::all(ui_style.dialogue_border),
            Visibility::Hidden,
        ))
        .id();

    commands.entity(dialogue_root).with_children(|parent| {
        parent.spawn((
            SpeakerText,
            Text::new(""),
            ui_text_font(&ui_fonts, ui_style.speaker_size),
            TextColor(ui_style.speaker_color),
        ));
        parent.spawn((
            LineText,
            Text::new(""),
            ui_text_font(&ui_fonts, ui_style.line_size),
            TextColor(ui_style.line_color),
        ));
        parent.spawn((
            HintText,
            Text::new("click / enter / space"),
            ui_text_font(&ui_fonts, ui_style.hint_size),
            TextColor(ui_style.hint_color),
            if ui_style.hint_visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ));
    });

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
        gallery_script: vfs
            .0
            .resolve_path(Some(vfs.0.settings_path()), "scripts/gallery.story.hks"),
        screen: FrontendScreen::InGame,
        runtime_started: true,
        dirty: false,
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

pub fn rebuild_frontend_ui(
    mut commands: Commands,
    mut frontend: ResMut<FrontendState>,
    ui_fonts: Res<UiFonts>,
    user_settings: Res<UserSettings>,
) {
    if !frontend.dirty {
        return;
    }

    if let Some(root) = frontend.root.take() {
        commands.entity(root).try_despawn();
    }

    frontend.dirty = false;
    if frontend.screen == FrontendScreen::InGame {
        return;
    }

    let root = commands
        .spawn((
            FrontendRoot,
            Node {
                width: percent(100.0),
                height: percent(100.0),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Stretch,
                padding: UiRect::axes(px(36.0), px(30.0)),
                column_gap: px(28.0),
                ..default()
            },
            BackgroundColor(FRONTEND_BG.with_alpha(0.96)),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Node {
                    width: percent(42.0),
                    height: percent(100.0),
                    padding: UiRect::all(px(28.0)),
                    border_radius: BorderRadius::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                },
                BackgroundColor(FRONTEND_PANEL.with_alpha(0.92)),
                BorderColor::all(Color::WHITE.with_alpha(0.08)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("hiraku"),
                    ui_text_font(&ui_fonts, 72.0),
                    TextColor(Color::srgb(0.95, 0.82, 0.67)),
                ));
                panel.spawn((
                    Text::new("Bevy + HKS visual novel runtime"),
                    ui_text_font(&ui_fonts, 24.0),
                    TextColor(Color::WHITE.with_alpha(0.72)),
                ));
                panel.spawn((
                    Text::new("Title menu now owns startup, settings, and save selection so scripts can stay focused on scene flow."),
                    ui_text_font(&ui_fonts, 22.0),
                    TextColor(Color::WHITE.with_alpha(0.56)),
                ));
            });

        parent
            .spawn((
                Node {
                    width: percent(58.0),
                    height: percent(100.0),
                    padding: UiRect::all(px(28.0)),
                    border_radius: BorderRadius::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18.0),
                    ..default()
                },
                BackgroundColor(FRONTEND_PANEL_ALT.with_alpha(0.96)),
                BorderColor::all(Color::WHITE.with_alpha(0.10)),
            ))
            .with_children(|panel| {
                let title = match frontend.screen {
                    FrontendScreen::Title => "Start",
                    FrontendScreen::Load => "Load",
                    FrontendScreen::Settings => "Settings",
                    FrontendScreen::InGame => unreachable!(),
                };

                panel.spawn((
                    Text::new(title),
                    ui_text_font(&ui_fonts, 46.0),
                    TextColor(Color::WHITE),
                ));

                if let Some(notice) = frontend.notice.as_ref() {
                    panel.spawn((
                        Text::new(notice.clone()),
                        ui_text_font(&ui_fonts, 20.0),
                        TextColor(Color::srgb(0.98, 0.78, 0.58)),
                    ));
                }

                match frontend.screen {
                    FrontendScreen::Title => {
                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::StartNewGame,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("New Game"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::OpenLoad,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new(format!("Load Game ({})", frontend.saves.len())),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::StartCharacterGallery,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Character Gallery"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::OpenSettings,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Settings"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::Load => {
                        if frontend.saves.is_empty() {
                            panel.spawn((
                                Text::new("No save files found in saves/*.sav"),
                                ui_text_font(&ui_fonts, 24.0),
                                TextColor(Color::WHITE.with_alpha(0.62)),
                            ));
                        }

                        for slot in &frontend.saves {
                            let route = slot.route.clone().unwrap_or_else(|| "unknown route".to_string());
                            let background = slot
                                .background
                                .as_ref()
                                .map(|path| shorten_path(path))
                                .unwrap_or_else(|| "no background snapshot".to_string());
                            panel
                                .spawn((
                                    FrontendButton {
                                        action: FrontendAction::LoadSlot(slot.slot.clone()),
                                    },
                                    Button,
                                    Node {
                                        width: percent(100.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(16.0)),
                                        justify_content: JustifyContent::FlexStart,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(16.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new(format!(
                                            "Slot {}\n{}\n{}\n{}",
                                            slot.slot,
                                            route,
                                            shorten_path(&slot.resume_script),
                                            background
                                        )),
                                        ui_text_font(&ui_fonts, 22.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                        }

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::BackToTitle,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Back"),
                                    ui_text_font(&ui_fonts, 26.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::Settings => {
                        panel.spawn((
                            Text::new(format!("BGM {:.0}%", user_settings.bgm_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustBgm(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("BGM -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustBgm(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("BGM +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel.spawn((
                            Text::new(format!("Voice {:.0}%", user_settings.voice_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustVoice(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("Voice -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustVoice(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("Voice +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel.spawn((
                            Text::new(format!("SFX {:.0}%", user_settings.sfx_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustSfx(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("SFX -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustSfx(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("SFX +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::BackToTitle,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    margin: UiRect::top(px(10.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Back"),
                                    ui_text_font(&ui_fonts, 26.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::InGame => unreachable!(),
                }
            });
    });

    frontend.root = Some(root);
}

#[allow(clippy::too_many_arguments)]
pub fn handle_frontend_buttons(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vfs: Res<VfsResource>,
    mut shared_state: ResMut<SceneSharedState>,
    mut frontend: ResMut<FrontendState>,
    mut user_settings: ResMut<UserSettings>,
    mut stage: ResMut<StageState>,
    mut dialogue_state: ResMut<DialogueState>,
    mut choice_state: ResMut<ChoiceState>,
    mut script_runtime: ResMut<ScriptRuntimeState>,
    mut interaction_query: Query<
        (&PickingInteraction, &mut BackgroundColor, &FrontendButton),
        Changed<PickingInteraction>,
    >,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    mut dialogue_root: Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    mut speaker_text: Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    mut line_text: Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
) {
    if frontend.screen == FrontendScreen::InGame {
        return;
    }

    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            PickingInteraction::Pressed => {
                *color = FRONTEND_BUTTON_PRESSED.into();
                let action = button.action.clone();
                match action {
                    FrontendAction::StartNewGame => {
                        if frontend.runtime_started {
                            continue;
                        }
                        if frontend.startup_script.is_empty() {
                            frontend.notice = Some(
                                "No startup script is configured. Check hdp://main.hdp/settings.hson."
                                    .to_string(),
                            );
                            frontend.dirty = true;
                            continue;
                        }

                        let bootstrap = ScriptBootstrap::new(frontend.startup_script.clone());

                        start_frontend_session(
                            &mut commands,
                            &asset_server,
                            &vfs,
                            &mut shared_state,
                            &mut stage,
                            &mut dialogue_state,
                            &mut choice_state,
                            &choice_ui,
                            &mut dialogue_root,
                            &mut speaker_text,
                            &mut line_text,
                            &user_settings,
                            &mut frontend,
                            &mut script_runtime,
                            bootstrap,
                            SceneSnapshot::default(),
                        );
                    }
                    FrontendAction::StartCharacterGallery => {
                        if frontend.runtime_started {
                            continue;
                        }
                        if !vfs.0.exists(&frontend.gallery_script) {
                            frontend.notice = Some(
                                "Character gallery script is missing from main.hdp.".to_string(),
                            );
                            frontend.dirty = true;
                            continue;
                        }
                        let mut bootstrap = ScriptBootstrap::new(frontend.gallery_script.clone());
                        bootstrap.values.insert(
                            "route".to_string(),
                            StoredValue::String("gallery".to_string()),
                        );

                        start_frontend_session(
                            &mut commands,
                            &asset_server,
                            &vfs,
                            &mut shared_state,
                            &mut stage,
                            &mut dialogue_state,
                            &mut choice_state,
                            &choice_ui,
                            &mut dialogue_root,
                            &mut speaker_text,
                            &mut line_text,
                            &user_settings,
                            &mut frontend,
                            &mut script_runtime,
                            bootstrap,
                            SceneSnapshot::default(),
                        );
                    }
                    FrontendAction::OpenLoad => {
                        frontend.screen = FrontendScreen::Load;
                        frontend.notice = None;
                        match list_save_slots() {
                            Ok(saves) => frontend.saves = saves,
                            Err(err) => {
                                frontend.notice = Some(format!("Failed to read save list: {err}"))
                            }
                        }
                        frontend.dirty = true;
                    }
                    FrontendAction::OpenSettings => {
                        frontend.screen = FrontendScreen::Settings;
                        frontend.notice = None;
                        frontend.dirty = true;
                    }
                    FrontendAction::BackToTitle => {
                        frontend.screen = FrontendScreen::Title;
                        frontend.notice = None;
                        frontend.dirty = true;
                    }
                    FrontendAction::LoadSlot(slot) => match load_save_data(&slot) {
                        Ok(save_data) => {
                            if frontend.runtime_started {
                                continue;
                            }
                            let snapshot = save_data.scene.clone();
                            let bootstrap = ScriptBootstrap::from_save(&save_data);
                            start_frontend_session(
                                &mut commands,
                                &asset_server,
                                &vfs,
                                &mut shared_state,
                                &mut stage,
                                &mut dialogue_state,
                                &mut choice_state,
                                &choice_ui,
                                &mut dialogue_root,
                                &mut speaker_text,
                                &mut line_text,
                                &user_settings,
                                &mut frontend,
                                &mut script_runtime,
                                bootstrap,
                                snapshot,
                            );
                        }
                        Err(err) => {
                            frontend.notice = Some(format!("Failed to load slot {slot}: {err}"));
                            frontend.dirty = true;
                        }
                    },
                    FrontendAction::AdjustBgm(delta) => {
                        user_settings.bgm_volume = adjusted_volume(user_settings.bgm_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                    FrontendAction::AdjustVoice(delta) => {
                        user_settings.voice_volume =
                            adjusted_volume(user_settings.voice_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                    FrontendAction::AdjustSfx(delta) => {
                        user_settings.sfx_volume = adjusted_volume(user_settings.sfx_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                }
            }
            PickingInteraction::Hovered => {
                *color = FRONTEND_BUTTON_HOVERED.into();
            }
            PickingInteraction::None => {
                *color = FRONTEND_BUTTON.into();
            }
        }
    }
}

pub fn process_script_commands(ctx: SceneCommandContext) {
    let mut commands = ctx.commands;
    let mut app_exit = ctx.app_exit;
    let asset_server = ctx.asset_server;
    let images = ctx.images;
    let texture_atlases = ctx.texture_atlases;
    let vfs = ctx.vfs;
    let mut shared_state = ctx.shared_state;
    let characters = ctx.characters;
    let mut user_settings = ctx.user_settings;
    let ui_fonts = ctx.ui_fonts;
    let mut ui_style = ctx.ui_style;
    let mut frontend = ctx.frontend;
    let mut stage = ctx.stage;
    let mut waits = ctx.waits;
    let mut pending_cancels = ctx.pending_cancels;
    let mut camera_state = ctx.camera_state;
    let mut camera_tweens = ctx.camera_tweens;
    let mut pending_script_commands = ctx.pending_script_commands;
    let mut script_runtime = ctx.script_runtime;
    let mut active_batches = ctx.active_batches;
    let mut dialogue_state = ctx.dialogue_state;
    let mut choice_state = ctx.choice_state;
    let mut screen_state = ctx.screen_state;
    let mut overlay_state = ctx.overlay_state;
    let mut animations = ctx.animations;
    let mut voice_state = ctx.voice_state;
    let mut pending_characters = ctx.pending_characters;
    let transition_mesh = ctx.transition_mesh;
    let mut meshes = ctx.meshes;
    let mut alpha_mask_materials = ctx.alpha_mask_materials;
    let mut multiply_materials = ctx.multiply_materials;
    let mut world_sprite_materials = ctx.world_sprite_materials;
    let mut custom_effect_materials = ctx.custom_effect_materials;
    let mut rule_materials = ctx.rule_materials;
    let choice_ui_roots = ctx.choice_ui_roots;
    let mut dialogue_root = ctx.dialogue_root;
    let mut dialogue_root_node = ctx.dialogue_root_node;
    let mut dialogue_background = ctx.dialogue_background;
    let mut dialogue_border = ctx.dialogue_border;
    let mut speaker_text = ctx.speaker_text;
    let mut line_text = ctx.line_text;
    let line_text_entity = ctx.line_text_entity;
    let mut speaker_font = ctx.speaker_font;
    let mut line_font = ctx.line_font;
    let mut hint_font = ctx.hint_font;
    let mut hint_visibility = ctx.hint_visibility;
    let mut speaker_color = ctx.speaker_color;
    let mut line_color = ctx.line_color;
    let mut hint_color = ctx.hint_color;

    while let Some(command) = pending_script_commands.items.pop_front() {
        if screen_state.active_root.is_some()
            && screen_state.waiting.is_none()
            && should_clear_stale_screen_before_command(&command)
        {
            clear_screen_ui(&mut commands, &mut screen_state);
        }

        match command {
            ScriptCommand::Log(message) => info!("[hks] {message}"),
            ScriptCommand::SetBackground {
                path,
                fade,
                animation_id,
            } => {
                let current_background = shared_state
                    .0
                    .background
                    .as_ref()
                    .map(|background| background.path.clone());
                if fade.is_none() && current_background.as_deref() == Some(path.as_str()) {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                }

                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }
                let image = asset_server.load(path.clone());
                let mut sprite = WorldSprite::from_image(image);
                let background = if let Some(duration) = fade {
                    sprite.color = sprite.color.with_alpha(0.0);
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                            VisualTween {
                                from_alpha: Some(0.0),
                                to_alpha: Some(1.0),
                                from_translation: None,
                                to_translation: None,
                                from_scale: None,
                                to_scale: None,
                                timer: Timer::new(duration, TimerMode::Once),
                                animation_id,
                                despawn_on_finish: false,
                            },
                        ))
                        .id()
                } else {
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    entity
                };

                if let Some(previous) = stage.background.replace(background) {
                    if let Some(duration) = fade {
                        commands.entity(previous).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id: None,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(previous).try_despawn();
                    }
                }

                shared_state.0.background = Some(ImageLayerSnapshot { path });
            }
            ScriptCommand::RuleTransitionBg {
                path,
                rule_path,
                duration,
                vague,
                animation_id,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }

                let Some(previous_background) = stage.background else {
                    let image = asset_server.load(path.clone());
                    let sprite = WorldSprite::from_image(image);
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    shared_state.0.background = Some(ImageLayerSnapshot { path });
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let Some(previous_path) = shared_state
                    .0
                    .background
                    .as_ref()
                    .map(|background| background.path.clone())
                else {
                    let image = asset_server.load(path.clone());
                    let sprite = WorldSprite::from_image(image);
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let target_image = asset_server.load(path.clone());
                let previous_image = asset_server.load(previous_path);
                let rule_image = asset_server.load(rule_path);
                let material = rule_materials.add(RuleTransitionMaterial {
                    from_texture: previous_image,
                    to_texture: target_image.clone(),
                    rule_texture: rule_image,
                    progress: 0.0,
                    vague,
                });
                let transition_entity = commands
                    .spawn((
                        Mesh3d(transition_mesh.0.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_BACKGROUND + 1.0),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        RuleTransitionPlayer {
                            material,
                            target_path: path,
                            target_image,
                            previous_background,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                        },
                    ))
                    .id();
                stage.transition = Some(transition_entity);
            }
            ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }

                let source_image = asset_server.load(options.from_path.clone());
                let target_image = asset_server.load(options.to_path.clone());
                let rule_image = asset_server.load(options.rule_path.clone());
                let aux0_image = asset_server.load(options.aux0_path.clone());
                let aux1_image = asset_server.load(options.aux1_path.clone());

                let previous_background = if options.commit_to_bg {
                    stage.background.take()
                } else {
                    stage.background
                };

                let material = custom_effect_materials.add(CustomScreenEffectMaterial {
                    source_texture: source_image,
                    target_texture: target_image.clone(),
                    rule_texture: rule_image,
                    aux0_texture: aux0_image,
                    aux1_texture: aux1_image,
                    progress: 0.0,
                    duration: options.duration.as_secs_f32(),
                    time: 0.0,
                    mode: options.mode,
                    p0: options.p0,
                    p1: options.p1,
                    p2: options.p2,
                    p3: options.p3,
                });

                let effect_entity = commands
                    .spawn((
                        Mesh3d(transition_mesh.0.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_OVERLAY - 0.5),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        CustomScreenEffectPlayer {
                            material,
                            timer: Timer::new(options.duration, TimerMode::Once),
                            target_path: options.commit_to_bg.then_some(options.to_path),
                            target_image: options.commit_to_bg.then_some(target_image),
                            previous_background,
                            animation_id,
                        },
                    ))
                    .id();
                stage.screen_effect = Some(effect_entity);
            }
            ScriptCommand::ShowSprite {
                id,
                path,
                rect,
                position,
                layer,
                scale,
                fade,
                animation_id,
            } => {
                let handle = asset_server.load(path.clone());
                let entity = if let Some(entity) = stage.sprites.get(&id).copied() {
                    let mut sprite = WorldSprite::from_image(handle)
                        .with_rect(rect.map(source_rect_from_corners));
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    commands.entity(entity).insert((
                        SpriteActor {
                            id: id.clone(),
                            path: path.clone(),
                        },
                        sprite,
                        Transform {
                            translation: Vec3::new(position.x, position.y, STAGE_Z_SPRITE + layer),
                            scale: Vec3::splat(scale),
                            ..default()
                        },
                    ));
                    entity
                } else {
                    let mut sprite = WorldSprite::from_image(handle)
                        .with_rect(rect.map(source_rect_from_corners));
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            SpriteActor {
                                id: id.clone(),
                                path: path.clone(),
                            },
                            sprite,
                            render,
                            Transform {
                                translation: Vec3::new(
                                    position.x,
                                    position.y,
                                    STAGE_Z_SPRITE + layer,
                                ),
                                scale: Vec3::splat(scale),
                                ..default()
                            },
                        ))
                        .id();
                    stage.sprites.insert(id.clone(), entity);
                    entity
                };

                if let Some(duration) = fade {
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                        despawn_on_finish: false,
                    });
                } else {
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::HideSprite {
                id,
                fade,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.remove(&id) {
                    if let Some(duration) = fade {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(entity).try_despawn();
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::SetOverlay {
                alpha,
                fade,
                animation_id,
            } => {
                if let Some(overlay) = stage.overlay {
                    if let Some(duration) = fade {
                        let current_alpha = shared_state.0.overlay_alpha;
                        commands.entity(overlay).insert(VisualTween {
                            from_alpha: Some(current_alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        commands.entity(overlay).insert(WorldSprite::from_color(
                            Color::BLACK.with_alpha(alpha),
                            Vec2::new(6000.0, 6000.0),
                        ));
                        if let Some(animation_id) = animation_id {
                            animations.completed.insert(animation_id);
                        }
                    }
                    shared_state.0.overlay_alpha = alpha;
                }
            }
            ScriptCommand::Say {
                speaker,
                text,
                animation_id,
            } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_fonts,
                        &ui_style,
                        &text,
                        0,
                        None,
                    );
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id,
                    request: None,
                });
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::ContinueDialogue { text, animation_id } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                if shared_state.0.dialogue.is_none() {
                    warn!("dialogue continuation has no UI buffer; treating it as narration");
                    if let Ok(mut visibility) = dialogue_root.single_mut() {
                        *visibility = Visibility::Visible;
                    }
                    if let Ok(mut speaker_node) = speaker_text.single_mut() {
                        **speaker_node = String::new();
                    }
                    if let Ok(line_root) = line_text_entity.single() {
                        set_dialogue_line_text(
                            &mut commands,
                            &mut dialogue_state,
                            line_root,
                            &mut line_text,
                            &ui_fonts,
                            &ui_style,
                            &text,
                            0,
                            None,
                        );
                    }
                    shared_state.0.dialogue = Some(DialogueSnapshot {
                        speaker: String::new(),
                        text,
                    });
                } else {
                    if let Ok(line_root) = line_text_entity.single() {
                        append_dialogue_line_text(
                            &mut commands,
                            &mut dialogue_state,
                            line_root,
                            &ui_fonts,
                            &ui_style,
                            &text,
                            None,
                        );
                    }
                    if let Some(dialogue) = shared_state.0.dialogue.as_mut() {
                        dialogue.text.push_str(&text);
                    }
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id,
                    request: None,
                });
            }
            ScriptCommand::AwaitDialogueAdvance { done } => {
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id: None,
                    request: Some(done),
                });
            }
            ScriptCommand::SetDialogue {
                speaker,
                text,
                reveal_from,
                animation_id,
            } => {
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_fonts,
                        &ui_style,
                        &text,
                        reveal_from.unwrap_or_else(|| text.chars().count()),
                        animation_id,
                    );
                }
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::ClearDialogue => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                clear_dialogue_spans(&mut commands, &mut dialogue_state);
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Hidden;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = String::new();
                }
                if let Ok(mut line_node) = line_text.single_mut() {
                    **line_node = String::new();
                }
                shared_state.0.dialogue = None;
            }
            ScriptCommand::SetTextEffect(effect) => {
                apply_text_effect_spec(&mut dialogue_state.effect, effect);
                shared_state.0.text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::ResetTextEffect => {
                dialogue_state.effect = DialogueTextEffect::default();
                shared_state.0.text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::SetCamera {
                blur_intensity,
                zoom,
                offset,
                rotation,
                projection,
                scope,
                duration,
                ease,
                animation_id,
            } => {
                if let Some(blur) = blur_intensity {
                    shared_state.0.camera.blur = blur;
                }
                if let Some(zoom) = zoom {
                    shared_state.0.camera.zoom = zoom;
                }
                if let Some(offset) = offset {
                    shared_state.0.camera.offset = offset.to_array();
                }
                if let Some(rotation) = rotation {
                    shared_state.0.camera.rotation = rotation.to_array();
                }
                if let Some(projection) = projection {
                    shared_state.0.camera.projection = match projection {
                        crate::script::CameraProjectionMode::Orthographic => "orthographic",
                        crate::script::CameraProjectionMode::Perspective => "perspective",
                    }
                    .to_string();
                }
                shared_state.0.camera.scope = match scope {
                    crate::script::CameraEffectScope::World => "world",
                    crate::script::CameraEffectScope::Canvas => "canvas",
                }
                .to_string();
                start_camera_tween(
                    &mut camera_state,
                    &mut camera_tweens,
                    blur_intensity,
                    zoom,
                    offset,
                    rotation,
                    projection,
                    scope,
                    duration,
                    ease,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::ApplyUserSettings(settings) => *user_settings = settings,
            ScriptCommand::AdjustUserSetting { name, delta } => {
                let volume = match name.as_str() {
                    "bgmVolume" => &mut user_settings.bgm_volume,
                    "voiceVolume" => &mut user_settings.voice_volume,
                    "sfxVolume" => &mut user_settings.sfx_volume,
                    _ => {
                        warn!("unsupported user setting `{name}`");
                        continue;
                    }
                };
                *volume = adjusted_volume(*volume, delta);
                if let Err(error) = write_user_settings(user_settings.as_ref()) {
                    warn!("failed to write user settings: {error}");
                }
            }
            ScriptCommand::ApplyUiStyle(style_patch) => {
                apply_ui_style_patch(&mut ui_style, style_patch);
                refresh_dialogue_ui_style(
                    &ui_fonts,
                    &ui_style,
                    &mut dialogue_root_node,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_font,
                    &mut line_font,
                    &mut hint_font,
                    &mut hint_visibility,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ResetUiStyle => {
                *ui_style = UiStyle::default();
                refresh_dialogue_ui_style(
                    &ui_fonts,
                    &ui_style,
                    &mut dialogue_root_node,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_font,
                    &mut line_font,
                    &mut hint_font,
                    &mut hint_visibility,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ShowScreen { screen, done } => {
                let spawned = spawn_screen_ui(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &ui_fonts,
                    &ui_style,
                    &screen,
                );
                let root = spawned.root;
                let previous = screen_state.active_root.take();
                let images_ready = screen_images_ready(&images, &spawned.image_handles);
                if previous.is_none() && images_ready {
                    commands
                        .entity(root)
                        .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z)));
                    screen_state.active_root = Some(root);
                    screen_state.waiting = done;
                } else {
                    commands
                        .entity(root)
                        .insert((Visibility::Hidden, GlobalZIndex(SCREEN_PENDING_Z)));
                    screen_state.pending_root = Some(crate::ui::PendingScreenRoot {
                        entity: root,
                        previous,
                        wait_images: spawned.image_handles,
                        ready_frames_remaining: SCREEN_READY_FRAMES,
                        done,
                    });
                    screen_state.waiting = None;
                }
            }
            ScriptCommand::WaitForScreenChoice { done } => {
                if screen_state.active_root.is_some() && screen_state.pending_root.is_none() {
                    screen_state.waiting = Some(done);
                } else {
                    commands.write_message(ScriptResponseMessage {
                        request: done,
                        response: ScriptResponse::Continue,
                    });
                }
            }
            ScriptCommand::ShowOverlay { name, screen } => {
                if let Some(root) = overlay_state.roots.remove(&name) {
                    commands.entity(root).try_despawn();
                }
                let spawned = spawn_screen_ui(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &ui_fonts,
                    &ui_style,
                    &screen,
                );
                commands
                    .entity(spawned.root)
                    .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z + 10)));
                overlay_state.roots.insert(name, spawned.root);
            }
            ScriptCommand::HideOverlay { name } => {
                if let Some(root) = overlay_state.roots.remove(&name) {
                    commands.entity(root).try_despawn();
                }
            }
            ScriptCommand::Choose {
                prompt,
                options,
                done,
            } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                spawn_choice_ui(&mut commands, &ui_fonts, &ui_style, &prompt, &options);
                choice_state.waiting = Some(done);
                choice_state.options = options;
            }
            ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                expressions,
                position,
                scale,
                focused,
                fade,
                animation_id,
            } => {
                let Some(character) = characters.characters.get(&character_name).cloned() else {
                    warn!("character `{character_name}` not found in catalog");
                    complete_missing_animation(&mut animations, animation_id);
                    continue;
                };
                let parts = match character.parts_for_expressions(&expressions) {
                    Ok(parts) => parts,
                    Err(message) => {
                        warn!("{message}");
                        complete_missing_animation(&mut animations, animation_id);
                        continue;
                    }
                };

                stage.character_positions.insert(actor_id.clone(), position);
                queue_character_show(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &mut meshes,
                    &mut alpha_mask_materials,
                    &mut multiply_materials,
                    &mut stage,
                    &mut pending_characters,
                    &mut animations,
                    actor_id,
                    parts,
                    position,
                    scale,
                    focused,
                    fade,
                    animation_id,
                );
            }
            ScriptCommand::HideCharacter { actor_id } => {
                despawn_character_actor(
                    &mut commands,
                    &mut stage,
                    &mut pending_characters,
                    &actor_id,
                );
                stage.character_positions.remove(&actor_id);
            }
            ScriptCommand::JumpCharacter {
                actor_id,
                height,
                duration,
                animation_id,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Jump { height },
                    duration,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::ShakeCharacter {
                actor_id,
                amplitude,
                duration,
                animation_id,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Shake { amplitude },
                    duration,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::AnimateCharacter {
                actor_id,
                keyframes,
                animation_id,
            } => {
                apply_character_timeline(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    keyframes,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::RestoreSnapshot { snapshot } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                // clear_overlay_ui(&mut commands, &mut overlay_state);
                commands.insert_resource(CameraShakeState::default());
                commands.insert_resource(AnimationState::default());
                pending_characters.items.clear();
                camera_state.blur_intensity = snapshot.camera.blur;
                camera_state.zoom = snapshot.camera.zoom.max(0.01);
                camera_state.offset = Vec3::from_array(snapshot.camera.offset);
                camera_state.rotation = Vec3::from_array(snapshot.camera.rotation);
                camera_state.projection = if snapshot.camera.projection == "perspective" {
                    crate::script::CameraProjectionMode::Perspective
                } else {
                    crate::script::CameraProjectionMode::Orthographic
                };
                camera_state.effect_scope = if snapshot.camera.scope == "canvas" {
                    crate::script::CameraEffectScope::Canvas
                } else {
                    crate::script::CameraEffectScope::World
                };
                *camera_tweens = CameraTweenState::default();
                restore_scene_snapshot(
                    &mut commands,
                    &asset_server,
                    &mut stage,
                    &mut dialogue_state,
                    &mut choice_state,
                    &mut dialogue_root,
                    &mut speaker_text,
                    &mut line_text,
                    &user_settings,
                    snapshot.clone(),
                );
                shared_state.0 = snapshot;
            }
            ScriptCommand::MoveSprite {
                id,
                position,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: Some(from),
                            to_translation: Some(Vec3::new(
                                position.x,
                                position.y,
                                STAGE_Z_SPRITE + sprite.layer,
                            )),
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during move");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for move_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::ScaleSprite {
                id,
                scale,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::splat(sprite.scale);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: None,
                            to_translation: None,
                            from_scale: Some(from),
                            to_scale: Some(Vec3::splat(scale)),
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during scale");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for scale_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::FadeSprite {
                id,
                alpha,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(sprite.alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during fade");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for fade_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::Wait {
                duration,
                animation_id,
                done,
            } => {
                waits.items.push(PendingWait {
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id,
                    done,
                });
            }
            ScriptCommand::WaitAnimations { ids, done } => {
                if ids.iter().all(|id| animations.completed.contains(id)) {
                    commands.write_message(ScriptResponseMessage {
                        request: done,
                        response: ScriptResponse::Continue,
                    });
                } else {
                    animations.waits.push(PendingAnimationWait { ids, done });
                }
            }
            ScriptCommand::Shake {
                duration,
                amplitude,
                animation_id,
            } => {
                commands.insert_resource(CameraShakeState {
                    active: Some(CameraShake {
                        timer: Timer::new(duration, TimerMode::Once),
                        amplitude,
                        animation_id,
                    }),
                });
            }
            ScriptCommand::PlayBgm {
                path,
                prelude,
                volume,
                fade_in,
                animation_id,
            } => {
                let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
                if let Some(previous) = stage.bgm.take() {
                    commands.entity(previous).try_despawn();
                }
                let start_volume = if fade_in.is_some() {
                    0.0
                } else {
                    playback_volume
                };
                let loop_audio = asset_server.load(path.clone());
                let bgm = if let Some(prelude) = prelude {
                    commands
                        .spawn((
                            BgmChannel {
                                path: path.clone(),
                                volume,
                            },
                            BgmPrelude {
                                prelude_audio: asset_server.load(prelude),
                                loop_audio,
                                start_volume,
                            },
                        ))
                        .id()
                } else {
                    commands
                        .spawn((
                            BgmChannel {
                                path: path.clone(),
                                volume,
                            },
                            AudioPlayer::new(loop_audio),
                            PlaybackSettings::LOOP.with_volume(Volume::Linear(start_volume)),
                        ))
                        .id()
                };
                if let Some(fade_in) = fade_in {
                    commands.entity(bgm).insert(BgmFade {
                        from: start_volume,
                        to: playback_volume,
                        timer: Timer::new(fade_in, TimerMode::Once),
                        animation_id,
                    });
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                }
                stage.bgm = Some(bgm);
                shared_state.0.bgm = Some(AudioSnapshot { path, volume });
            }
            ScriptCommand::SetBgmVolume { volume } => {
                let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
                if let Some(bgm) = stage.bgm {
                    if let Some(snapshot) = shared_state.0.bgm.as_ref() {
                        commands.entity(bgm).insert(BgmChannel {
                            path: snapshot.path.clone(),
                            volume,
                        });
                    }
                    commands.entity(bgm).insert(BgmFade {
                        from: playback_volume,
                        to: playback_volume,
                        timer: Timer::new(std::time::Duration::ZERO, TimerMode::Once),
                        animation_id: None,
                    });
                }
                if let Some(snapshot) = shared_state.0.bgm.as_mut() {
                    snapshot.volume = volume;
                }
            }
            ScriptCommand::FadeBgm {
                volume,
                duration,
                animation_id,
            } => {
                let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
                let from = shared_state
                    .0
                    .bgm
                    .as_ref()
                    .map(|bgm| bgm.volume)
                    .map(|volume| apply_volume_setting(volume, user_settings.bgm_volume))
                    .unwrap_or(playback_volume);
                if let Some(bgm) = stage.bgm {
                    if let Some(snapshot) = shared_state.0.bgm.as_ref() {
                        commands.entity(bgm).insert(BgmChannel {
                            path: snapshot.path.clone(),
                            volume,
                        });
                    }
                    commands.entity(bgm).insert(BgmFade {
                        from,
                        to: playback_volume,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                    });
                    if let Some(snapshot) = shared_state.0.bgm.as_mut() {
                        snapshot.volume = volume;
                    }
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                }
            }
            ScriptCommand::StopBgm => {
                if let Some(previous) = stage.bgm.take() {
                    commands.entity(previous).try_despawn();
                }
                shared_state.0.bgm = None;
            }
            ScriptCommand::PlayVoice {
                path,
                volume,
                mode,
                animation_id,
            } => {
                let playback_volume = apply_volume_setting(volume, user_settings.voice_volume);
                if mode == VoicePlaybackMode::Exclusive {
                    finish_active_voice(&mut commands, &mut animations, &mut voice_state);
                }
                let voice = commands
                    .spawn((
                        VoiceChannel {
                            path: path.clone(),
                            volume,
                        },
                        AudioPlayer::new(asset_server.load(path.clone())),
                        PlaybackSettings::ONCE.with_volume(Volume::Linear(playback_volume)),
                    ))
                    .id();
                let active = ActiveVoice {
                    entity: voice,
                    animation_id,
                };
                match mode {
                    VoicePlaybackMode::Exclusive => voice_state.active = Some(active),
                    VoicePlaybackMode::Concurrent => {
                        voice_state.concurrent.insert(voice, active);
                    }
                }
            }
            ScriptCommand::StopVoice => {
                finish_all_voices(&mut commands, &mut animations, &mut voice_state);
            }
            ScriptCommand::PlaySfx { path, volume } => {
                let playback_volume = apply_volume_setting(volume, user_settings.sfx_volume);
                commands.spawn((
                    SfxChannel { volume },
                    AudioPlayer::new(asset_server.load(path)),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(playback_volume)),
                ));
            }
            ScriptCommand::SubmitBatch { mode, items } => match mode {
                BatchSubmitMode::Parallel => {
                    pending_script_commands
                        .items
                        .extend(items.into_iter().map(|item| *item.command));
                }
                BatchSubmitMode::Sequence => {
                    let mut remaining = items.into_iter().collect::<VecDeque<_>>();
                    let Some(first) = remaining.pop_front() else {
                        continue;
                    };
                    let current_handle = first.handle.clone();
                    pending_script_commands.items.push_back(*first.command);
                    active_batches.items.push(ActiveScriptBatch {
                        remaining,
                        current_handle,
                    });
                }
            },
            ScriptCommand::CancelAnimations { ids } => {
                pending_cancels.ids.extend(ids);
            }
            ScriptCommand::Exit => {
                app_exit.write(AppExit::Success);
            }
            ScriptCommand::ReturnToTitle => {
                finish_all_voices(&mut commands, &mut animations, &mut voice_state);
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                clear_overlay_ui(&mut commands, &mut overlay_state);
                script_runtime.mounted_ui_overlays.clear();
                script_runtime.ui_registry.clear();
                pending_characters.items.clear();
                shared_state.0 = SceneSnapshot::default();
                restore_scene_snapshot(
                    &mut commands,
                    &asset_server,
                    &mut stage,
                    &mut dialogue_state,
                    &mut choice_state,
                    &mut dialogue_root,
                    &mut speaker_text,
                    &mut line_text,
                    &user_settings,
                    SceneSnapshot::default(),
                );
                frontend.runtime_started = true;
                frontend.screen = FrontendScreen::InGame;
                frontend.notice = None;
                frontend.dirty = false;
                if !frontend.startup_script.is_empty() {
                    let startup = frontend.startup_script.clone();
                    let story = vfs
                        .0
                        .read_text(&startup)
                        .map_err(|error| error.to_string())
                        .and_then(|source| compile_story_bytecode(&startup, &source))
                        .and_then(|bytecode| {
                            StoryRuntime::new(bytecode).map_err(|error| error.to_string())
                        });
                    match story {
                        Ok(mut story) => {
                            story.set_globals(crate::script::capabilities::engine_globals(
                                &user_settings,
                            ));
                            script_runtime.story = Some(story);
                            script_runtime.current_script = Some(startup);
                            script_runtime.story_events.clear();
                            script_runtime.pending_ui_screen = None;
                            script_runtime.wait_request = None;
                            script_runtime.response_inbox.clear();
                            script_runtime.task_requests.clear();
                        }
                        Err(error) => crate::script::emit_script_diagnostic(
                            "failed to return to HKS title:",
                            &error,
                        ),
                    }
                }
            }
        }
    }
}

pub fn animate_bgm_fades(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut bgms: Query<(Entity, Option<&mut AudioSink>, &mut BgmFade)>,
) {
    for (entity, sink, mut fade) in &mut bgms {
        let Some(mut sink) = sink else {
            // Asset loading is asynchronous. The fade starts with audible playback, not while
            // the source is still waiting to be decoded.
            continue;
        };
        fade.timer.tick(time.delta());
        let fraction = tween_fraction(&fade.timer);
        let volume = fade.from + (fade.to - fade.from) * fraction;
        sink.set_volume(Volume::Linear(volume));

        if fade.timer.is_finished() {
            if let Some(animation_id) = fade.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            commands.entity(entity).try_remove::<BgmFade>();
        }
    }
}

/// Waits until both files are cached, then starts one decoder with no ECS boundary switch.
pub fn prepare_bgm_preludes(
    mut commands: Commands,
    file_audio: Res<Assets<AudioSource>>,
    mut prelude_loop_audio: ResMut<Assets<PreludeLoopAudio>>,
    preludes: Query<(Entity, &BgmPrelude)>,
) {
    for (entity, prelude) in &preludes {
        let Some(prelude_source) = file_audio.get(&prelude.prelude_audio) else {
            continue;
        };
        let Some(loop_source) = file_audio.get(&prelude.loop_audio) else {
            continue;
        };

        let audio = prelude_loop_audio.add(PreludeLoopAudio::new(
            prelude_source.clone(),
            loop_source.clone(),
        ));
        commands
            .entity(entity)
            .insert((
                AudioPlayer(audio),
                PlaybackSettings::ONCE.with_volume(Volume::Linear(prelude.start_volume)),
            ))
            .try_remove::<BgmPrelude>();
    }
}

pub fn apply_live_audio_settings(
    user_settings: Res<UserSettings>,
    mut bgms: Query<(&mut AudioSink, &BgmChannel), (Without<VoiceChannel>, Without<SfxChannel>)>,
    mut voices: Query<(&mut AudioSink, &VoiceChannel), (Without<BgmChannel>, Without<SfxChannel>)>,
    mut sfx: Query<(&mut AudioSink, &SfxChannel), (Without<BgmChannel>, Without<VoiceChannel>)>,
) {
    if !user_settings.is_changed() {
        return;
    }

    for (mut sink, channel) in &mut bgms {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.bgm_volume,
        )));
    }
    for (mut sink, channel) in &mut voices {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.voice_volume,
        )));
    }
    for (mut sink, channel) in &mut sfx {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.sfx_volume,
        )));
    }
}

pub fn handle_choice_buttons(
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    ui_style: Res<UiStyle>,
    mut interaction_query: Query<
        (&PickingInteraction, &mut BackgroundColor, &ChoiceButton),
        Changed<PickingInteraction>,
    >,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            PickingInteraction::Pressed => {
                *color = ui_style.choice_button_pressed.into();
                resolve_choice(&mut commands, &mut choice_state, &choice_ui, button.index);
            }
            PickingInteraction::Hovered => {
                *color = ui_style.choice_button_hovered.into();
            }
            PickingInteraction::None => {
                *color = ui_style.choice_button_bg.into();
            }
        }
    }
}

pub fn handle_choice_action_input(
    mut actions: MessageReader<crate::input::HirakuActionInput>,
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    if choice_state.waiting.is_none() {
        return;
    }

    let selected = actions.read().find_map(|action| match action.0 {
        crate::input::HirakuAction::Choice(index) => Some(index),
        _ => None,
    });

    if let Some(index) = selected {
        resolve_choice(&mut commands, &mut choice_state, &choice_ui, index);
    }
}

pub fn advance_dialogue_on_input(
    mut actions: MessageReader<crate::input::HirakuActionInput>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<&mut DialogueCharSpan>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    choice_state: Res<ChoiceState>,
    runtime_menu: Res<RuntimeMenuState>,
    ui_interactions: Query<
        &PickingInteraction,
        Or<(
            With<ScreenUiButton>,
            With<ScreenUiImageButton>,
            With<RuntimeMenuButton>,
            With<ChoiceButton>,
        )>,
    >,
) {
    if runtime_menu.pause_open {
        return;
    }

    if choice_state.waiting.is_some() {
        return;
    }

    let action_advance = actions
        .read()
        .any(|action| action.0 == crate::input::HirakuAction::NextDialogue);
    let pointer_advance = clicks
        .read()
        .any(|click| click.pointer_id.is_custom() && !ui_interactions.contains(click.entity));
    let advance = action_advance || pointer_advance;

    if !advance {
        return;
    }

    if action_advance
        && ui_interactions
            .iter()
            .any(|interaction| !matches!(*interaction, PickingInteraction::None))
    {
        return;
    }

    if dialogue_reveal_has_hidden_chars(&dialogue_state) {
        reveal_all_dialogue_chars(&mut dialogue_state, &mut dialogue_chars);
        return;
    }

    if let Some(waiting) = dialogue_state.waiting.take() {
        if let Some(animation_id) = waiting.animation_id {
            animations.completed.insert(animation_id);
        }
        if let Some(request) = waiting.request {
            responses.write(ScriptResponseMessage {
                request,
                response: ScriptResponse::Continue,
            });
        }
    }
}

pub fn animate_dialogue_text_reveal(
    time: Res<Time>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<(&mut TextColor, &mut DialogueCharSpan)>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    reveal.accumulator += time.delta_secs();
    while reveal.next_index < reveal.spans.len() && reveal.accumulator >= reveal.interval {
        reveal.accumulator -= reveal.interval;
        if let Some(entity) = reveal.spans.get(reveal.next_index).copied()
            && let Ok((_, mut span)) = dialogue_chars.get_mut(entity)
        {
            span.revealed = true;
            span.age = 0.0;
        }
        reveal.next_index += 1;
    }

    let mut fully_visible = reveal.next_index >= reveal.spans.len();
    for &entity in &reveal.spans {
        if let Ok((mut color, mut span)) = dialogue_chars.get_mut(entity) {
            if span.revealed {
                span.age = (span.age + time.delta_secs()).min(reveal.fade_seconds);
                let alpha = if reveal.fade_seconds <= f32::EPSILON {
                    span.target_alpha
                } else {
                    span.target_alpha * (span.age / reveal.fade_seconds).clamp(0.0, 1.0)
                };
                color.0.set_alpha(alpha);
                if span.age + f32::EPSILON < reveal.fade_seconds {
                    fully_visible = false;
                }
            } else {
                color.0.set_alpha(0.0);
                fully_visible = false;
            }
        }
    }

    if fully_visible {
        if let Some(animation_id) = reveal.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        dialogue_state.reveal = None;
    }
}

pub fn tick_pending_waits(
    time: Res<Time>,
    mut waits: ResMut<PendingWaits>,
    mut animations: ResMut<AnimationState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    for wait in waits.items.iter_mut() {
        wait.timer.tick(time.delta());
    }

    let mut completed = Vec::new();
    waits.items.retain(|wait| {
        if wait.timer.is_finished() {
            completed.push((wait.animation_id.clone(), wait.done.clone()));
            false
        } else {
            true
        }
    });

    for (animation_id, done) in completed {
        if let Some(animation_id) = animation_id {
            animations.completed.insert(animation_id);
        }
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Continue,
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_animation_cancellations(
    mut commands: Commands,
    mut stage: ResMut<StageState>,
    mut waits: ResMut<PendingWaits>,
    mut dialogue_state: ResMut<DialogueState>,
    mut pending_cancels: ResMut<PendingAnimationCancels>,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    mut voice_state: ResMut<VoiceState>,
    mut pending_characters: ResMut<PendingCharacterShows>,
    mut tweens: Query<(Entity, Option<&SpriteActor>, &mut VisualTween)>,
    mut bgm_fades: Query<(Entity, &mut BgmFade)>,
    mut motion_queries: ParamSet<(
        Query<'_, '_, &'static mut Transform, With<WorldCamera>>,
        Query<
            '_,
            '_,
            (
                Entity,
                &'static mut Transform,
                Option<&'static mut CharacterJumpEffect>,
                Option<&'static mut CharacterShakeEffect>,
                Option<&'static mut CharacterTimelineEffect>,
            ),
            Without<WorldCamera>,
        >,
    )>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    if pending_cancels.ids.is_empty() {
        return;
    }

    let cancelled = pending_cancels.ids.drain(..).collect::<HashSet<_>>();
    for id in &cancelled {
        animations.completed.insert(id.clone());
    }

    waits.items.retain(|wait| {
        if wait
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            commands.write_message(ScriptResponseMessage {
                request: wait.done,
                response: ScriptResponse::Continue,
            });
            false
        } else {
            true
        }
    });

    if dialogue_state
        .waiting
        .as_ref()
        .and_then(|waiting| waiting.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        if let Some(waiting) = dialogue_state.waiting.take() {
            complete_dialogue_wait(&mut commands, &mut animations, waiting);
        }
    }

    if let Some(shake) = shake_state.active.as_mut()
        && shake
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        for mut camera in &mut motion_queries.p0() {
            camera.translation.x = 0.0;
            camera.translation.y = 0.0;
        }
        complete_missing_animation(&mut animations, shake.animation_id.take());
        shake_state.active = None;
    }

    if voice_state
        .active
        .as_ref()
        .and_then(|voice| voice.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }

    pending_characters.items.retain_mut(|item| {
        if item
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            for entity in item.entities.drain(..) {
                commands.entity(entity).try_despawn();
            }
            for id in item.entity_ids.drain(..) {
                stage.sprites.remove(&id);
            }
            complete_missing_animation(&mut animations, item.animation_id.take());
            false
        } else {
            true
        }
    });

    for (entity, actor, mut tween) in &mut tweens {
        if tween
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if tween.despawn_on_finish
                && let Some(actor) = actor
            {
                stage.sprites.insert(actor.id.clone(), entity);
            }
            complete_missing_animation(&mut animations, tween.animation_id.take());
            commands.entity(entity).try_remove::<VisualTween>();
        }
    }

    for (entity, mut fade) in &mut bgm_fades {
        if fade
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, fade.animation_id.take());
            commands.entity(entity).try_remove::<BgmFade>();
        }
    }

    for (entity, mut transform, jump, shake, timeline) in &mut motion_queries.p1() {
        let mut reset_translation = false;
        let origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin));

        if let Some(mut effect) = jump
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_remove::<CharacterJumpEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = shake
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_remove::<CharacterShakeEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = timeline
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if let Some(final_keyframe) = effect.keyframes.last() {
                stage
                    .character_positions
                    .insert(effect.actor_id.clone(), final_keyframe.position);
            }
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands
                .entity(entity)
                .try_remove::<CharacterTimelineEffect>();
            reset_translation = true;
        }

        if reset_translation {
            if let Some(origin) = origin {
                transform.translation = origin;
            }
        }
    }

    for (entity, mut transition) in &mut transitions {
        if transition
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.transition == Some(entity) {
                stage.transition = None;
            }
            complete_missing_animation(&mut animations, transition.animation_id.take());
            commands.entity(entity).try_despawn();
        }
    }

    for (entity, mut effect) in &mut effects {
        if effect
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_despawn();
        }
    }
}

pub fn animate_visual_tweens(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut alpha_mask_materials: ResMut<Assets<AlphaMaskMaterial>>,
    mut multiply_materials: ResMut<Assets<MultiplyMaterial>>,
    mut visuals: Query<(
        Entity,
        Option<&mut WorldSprite>,
        Option<&MeshMaterial3d<AlphaMaskMaterial>>,
        Option<&MeshMaterial3d<MultiplyMaterial>>,
        Option<&CharacterPartVisual>,
        &mut Transform,
        &mut VisualTween,
    )>,
) {
    for (entity, mut sprite, alpha_mask, multiply, part_visual, mut transform, mut tween) in
        &mut visuals
    {
        tween.timer.tick(time.delta());
        let fraction = tween_fraction(&tween.timer);

        if let (Some(from), Some(to)) = (tween.from_alpha, tween.to_alpha) {
            set_visual_alpha(
                sprite.as_deref_mut(),
                alpha_mask,
                multiply,
                part_visual,
                &mut alpha_mask_materials,
                &mut multiply_materials,
                from + (to - from) * fraction,
            );
        }
        if let (Some(from), Some(to)) = (tween.from_translation, tween.to_translation) {
            transform.translation = from.lerp(to, fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_scale, tween.to_scale) {
            transform.scale = from.lerp(to, fraction);
        }

        if tween.timer.is_finished() {
            if let Some(to) = tween.to_alpha {
                set_visual_alpha(
                    sprite.as_deref_mut(),
                    alpha_mask,
                    multiply,
                    part_visual,
                    &mut alpha_mask_materials,
                    &mut multiply_materials,
                    to,
                );
            }
            if let Some(to) = tween.to_translation {
                transform.translation = to;
            }
            if let Some(to) = tween.to_scale {
                transform.scale = to;
            }
            if let Some(animation_id) = tween.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if tween.despawn_on_finish {
                commands.entity(entity).try_despawn();
            } else {
                commands.entity(entity).try_remove::<VisualTween>();
            }
        }
    }
}

fn set_visual_alpha(
    sprite: Option<&mut WorldSprite>,
    alpha_mask: Option<&MeshMaterial3d<AlphaMaskMaterial>>,
    multiply: Option<&MeshMaterial3d<MultiplyMaterial>>,
    part_visual: Option<&CharacterPartVisual>,
    alpha_mask_materials: &mut Assets<AlphaMaskMaterial>,
    multiply_materials: &mut Assets<MultiplyMaterial>,
    alpha: f32,
) {
    if let Some(sprite) = sprite {
        sprite.color.set_alpha(
            part_visual
                .map(|visual| visual.base_alpha * alpha)
                .unwrap_or(alpha),
        );
    }
    if let Some(material) = alpha_mask
        && let Some(mut material) = alpha_mask_materials.get_mut(&material.0)
    {
        material.opacity = alpha;
    }
    if let Some(material) = multiply
        && let Some(mut material) = multiply_materials.get_mut(&material.0)
    {
        material.opacity = alpha;
    }
}

pub fn animate_rule_transitions(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut rule_materials: ResMut<Assets<RuleTransitionMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut world_sprite_materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
) {
    for (entity, mut transition) in &mut transitions {
        transition.timer.tick(time.delta());
        let progress = tween_fraction(&transition.timer);

        if let Some(mut material) = rule_materials.get_mut(&transition.material) {
            material.progress = progress;
        }

        if transition.timer.is_finished() {
            let sprite = WorldSprite::from_image(transition.target_image.clone());
            let render =
                world_sprite_render_components(&sprite, &mut meshes, &mut world_sprite_materials);
            let new_background = commands
                .spawn((
                    BackgroundLayer {
                        path: transition.target_path.clone(),
                    },
                    sprite,
                    render,
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id();

            stage.background = Some(new_background);
            if stage.transition == Some(entity) {
                stage.transition = None;
            }

            commands
                .entity(transition.previous_background)
                .try_despawn();
            commands.entity(entity).try_despawn();

            if let Some(animation_id) = transition.animation_id.take() {
                animations.completed.insert(animation_id);
            }
        }
    }
}

pub fn animate_custom_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut materials: ResMut<Assets<CustomScreenEffectMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut world_sprite_materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    for (entity, mut effect) in &mut effects {
        effect.timer.tick(time.delta());
        let progress = tween_fraction(&effect.timer);

        if let Some(mut material) = materials.get_mut(&effect.material) {
            material.progress = progress;
            material.time = effect.timer.elapsed_secs();
        }

        if effect.timer.is_finished() {
            if let Some(target_path) = effect.target_path.take()
                && let Some(target_image) = effect.target_image.take()
            {
                let sprite = WorldSprite::from_image(target_image);
                let render = world_sprite_render_components(
                    &sprite,
                    &mut meshes,
                    &mut world_sprite_materials,
                );
                let new_background = commands
                    .spawn((
                        BackgroundLayer { path: target_path },
                        sprite,
                        render,
                        Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                    ))
                    .id();
                stage.background = Some(new_background);
            }

            if let Some(previous_background) = effect.previous_background.take()
                && stage.background != Some(previous_background)
            {
                commands.entity(previous_background).try_despawn();
            }

            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            commands.entity(entity).try_despawn();

            if let Some(animation_id) = effect.animation_id.take() {
                animations.completed.insert(animation_id);
            }
        }
    }
}

pub fn tick_animation_waits(
    mut animations: ResMut<AnimationState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    let completed = animations.completed.clone();
    let mut resolved = Vec::new();
    animations.waits.retain(|wait| {
        if wait.ids.iter().all(|id| completed.contains(id)) {
            resolved.push(wait.done.clone());
            false
        } else {
            true
        }
    });

    for done in resolved {
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Continue,
        });
    }
}

pub fn tick_script_batches(
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    mut active_batches: ResMut<ActiveScriptBatches>,
    animations: Res<AnimationState>,
) {
    let completed = animations.completed.clone();
    active_batches.items.retain_mut(|batch| {
        if !completed.contains(&batch.current_handle) {
            return true;
        }

        while let Some(next) = batch.remaining.pop_front() {
            if completed.contains(&next.handle) {
                continue;
            }

            batch.current_handle = next.handle;
            pending_script_commands.items.push_back(*next.command);
            return true;
        }

        false
    });
}

pub fn poll_voice_playback(
    mut commands: Commands,
    mut animations: ResMut<AnimationState>,
    mut voice_state: ResMut<VoiceState>,
    sinks: Query<&AudioSink>,
) {
    let exclusive_finished = voice_state
        .active
        .as_ref()
        .is_some_and(|active| sinks.get(active.entity).is_ok_and(|sink| sink.empty()));
    if exclusive_finished {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }

    let completed = voice_state
        .concurrent
        .keys()
        .copied()
        .filter(|entity| sinks.get(*entity).is_ok_and(|sink| sink.empty()))
        .collect::<Vec<_>>();
    for entity in completed {
        if let Some(active) = voice_state.concurrent.remove(&entity) {
            finish_voice(&mut commands, &mut animations, active);
        }
    }
}

pub fn sync_scene_snapshot(
    mut shared_state: ResMut<SceneSharedState>,
    stage: Res<StageState>,
    alpha_mask_materials: Res<Assets<AlphaMaskMaterial>>,
    multiply_materials: Res<Assets<MultiplyMaterial>>,
    dialogue_state: Res<DialogueState>,
    background_layers: Query<&BackgroundLayer>,
    bgms: Query<&BgmChannel>,
    overlay: Query<&WorldSprite, With<OverlayMarker>>,
    sprites: Query<(
        &SpriteActor,
        Option<&WorldSprite>,
        Option<&MeshMaterial3d<AlphaMaskMaterial>>,
        Option<&MeshMaterial3d<MultiplyMaterial>>,
        Option<&CharacterPartVisual>,
        Option<&FocusedActorPart>,
        &Transform,
    )>,
) {
    let snapshot = &mut shared_state.0;

    snapshot.background = stage
        .background
        .and_then(|entity| background_layers.get(entity).ok())
        .map(|layer| ImageLayerSnapshot {
            path: layer.path.clone(),
        });

    let mut sprite_snapshots = sprites
        .iter()
        .map(
            |(actor, sprite, alpha_mask, multiply, visual, focused, transform)| SpriteSnapshot {
                id: actor.id.clone(),
                path: actor.path.clone(),
                x: transform.translation.x,
                y: transform.translation.y,
                layer: transform.translation.z - STAGE_Z_SPRITE,
                scale: transform.scale.x,
                alpha: sprite
                    .map(|sprite| sprite.color.alpha())
                    .unwrap_or_else(|| {
                        alpha_mask
                            .and_then(|material| alpha_mask_materials.get(&material.0))
                            .map(|material| material.tint.w * material.opacity)
                            .or_else(|| {
                                multiply
                                    .and_then(|material| multiply_materials.get(&material.0))
                                    .map(|material| material.tint.w * material.opacity)
                            })
                            .unwrap_or(1.0)
                    }),
                rect: sprite
                    .and_then(|sprite| sprite.rect.map(source_rect_to_corners))
                    .or_else(|| visual.and_then(|visual| visual.rect)),
                focused: focused.is_some(),
            },
        )
        .collect::<Vec<_>>();
    sprite_snapshots.sort_by(|left, right| left.id.cmp(&right.id));
    snapshot.sprites = sprite_snapshots;
    snapshot.character_positions = stage
        .character_positions
        .iter()
        .map(|(actor_id, position)| (actor_id.clone(), [position.x, position.y]))
        .collect();

    snapshot.bgm = stage.bgm.and_then(|entity| {
        bgms.get(entity).ok().map(|bgm| AudioSnapshot {
            path: bgm.path.clone(),
            volume: bgm.volume,
        })
    });

    if let Ok(overlay_sprite) = overlay.single() {
        snapshot.overlay_alpha = overlay_sprite.color.alpha();
    }

    snapshot.text_effect = text_effect_snapshot(&dialogue_state.effect);
}

fn start_frontend_session(
    commands: &mut Commands,
    asset_server: &AssetServer,
    vfs: &VfsResource,
    shared_state: &mut SceneSharedState,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    dialogue_root: &mut Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    user_settings: &UserSettings,
    frontend: &mut FrontendState,
    script_runtime: &mut ScriptRuntimeState,
    bootstrap: ScriptBootstrap,
    snapshot: SceneSnapshot,
) {
    if let Some(root) = frontend.root.take() {
        commands.entity(root).try_despawn();
    }

    clear_choice_ui(commands, choice_ui);
    shared_state.0 = snapshot.clone();
    restore_scene_snapshot(
        commands,
        asset_server,
        stage,
        dialogue_state,
        choice_state,
        dialogue_root,
        speaker_text,
        line_text,
        user_settings,
        snapshot,
    );

    frontend.notice = None;
    frontend.runtime_started = true;
    frontend.screen = FrontendScreen::InGame;
    frontend.dirty = false;

    if let Err(error) = start_hks_runtime(vfs, script_runtime, bootstrap, user_settings) {
        frontend.notice = Some(format!("Failed to start HKS runtime: {error}"));
        frontend.runtime_started = false;
        frontend.dirty = true;
    }
}

fn shorten_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn adjusted_volume(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(0.0, 1.0)
}

fn apply_volume_setting(volume: f32, setting: f32) -> f32 {
    (volume * setting).clamp(0.0, 1.0)
}

fn format_storage_error(error: StorageError) -> String {
    format!("Failed to write settings: {error}")
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

fn clear_dialogue_spans(commands: &mut Commands, dialogue_state: &mut DialogueState) {
    for entity in dialogue_state.span_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    dialogue_state.reveal = None;
}

fn set_dialogue_line_text(
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    line_root: Entity,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    visible_prefix_chars: usize,
    animation_id: Option<String>,
) {
    clear_dialogue_spans(commands, dialogue_state);

    if let Ok(mut line_node) = line_text.single_mut() {
        **line_node = String::new();
    }

    let chars = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    let target_alpha = ui_style.line_color.alpha();
    let reveal_enabled = dialogue_state.effect.mode != DialogueTextEffectMode::Instant;
    let visible_prefix_chars = visible_prefix_chars.min(chars.len());

    for (index, ch) in chars.iter().enumerate() {
        let revealed = !reveal_enabled || index < visible_prefix_chars;
        let initial_alpha = if revealed { target_alpha } else { 0.0 };
        let entity = commands
            .spawn((
                TextSpan::new(ch.clone()),
                ui_text_font(ui_fonts, ui_style.line_size),
                TextColor(ui_style.line_color.with_alpha(initial_alpha)),
                DialogueCharSpan {
                    target_alpha,
                    age: if revealed {
                        dialogue_state.effect.fade_seconds
                    } else {
                        0.0
                    },
                    revealed,
                },
            ))
            .id();
        commands.entity(line_root).add_child(entity);
        dialogue_state.span_entities.push(entity);
    }

    if reveal_enabled && visible_prefix_chars < chars.len() {
        dialogue_state.reveal = Some(DialogueRevealState {
            spans: dialogue_state.span_entities.clone(),
            next_index: visible_prefix_chars,
            accumulator: 0.0,
            interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
            fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
            animation_id,
        });
    } else {
        dialogue_state.reveal = None;
        let _ = animation_id;
    }
}

fn append_dialogue_line_text(
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    line_root: Entity,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    animation_id: Option<String>,
) {
    let start_index = dialogue_state.span_entities.len();
    let target_alpha = ui_style.line_color.alpha();
    let reveal_enabled = dialogue_state.effect.mode != DialogueTextEffectMode::Instant;

    for ch in text.chars() {
        let entity = commands
            .spawn((
                TextSpan::new(ch.to_string()),
                ui_text_font(ui_fonts, ui_style.line_size),
                TextColor(ui_style.line_color.with_alpha(if reveal_enabled {
                    0.0
                } else {
                    target_alpha
                })),
                DialogueCharSpan {
                    target_alpha,
                    age: if reveal_enabled {
                        0.0
                    } else {
                        dialogue_state.effect.fade_seconds
                    },
                    revealed: !reveal_enabled,
                },
            ))
            .id();
        commands.entity(line_root).add_child(entity);
        dialogue_state.span_entities.push(entity);
    }

    if reveal_enabled && start_index < dialogue_state.span_entities.len() {
        if let Some(reveal) = dialogue_state.reveal.as_mut() {
            reveal.spans = dialogue_state.span_entities.clone();
            if reveal.animation_id.is_none() {
                reveal.animation_id = animation_id;
            }
        } else {
            dialogue_state.reveal = Some(DialogueRevealState {
                spans: dialogue_state.span_entities.clone(),
                next_index: start_index,
                accumulator: 0.0,
                interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
                fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
                animation_id,
            });
        }
    }
}

fn dialogue_reveal_has_hidden_chars(dialogue_state: &DialogueState) -> bool {
    dialogue_state
        .reveal
        .as_ref()
        .is_some_and(|reveal| reveal.next_index < reveal.spans.len())
}

fn reveal_all_dialogue_chars(
    dialogue_state: &mut DialogueState,
    dialogue_chars: &mut Query<&mut DialogueCharSpan>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    for &entity in &reveal.spans {
        if let Ok(mut span) = dialogue_chars.get_mut(entity) {
            span.revealed = true;
            span.age = reveal.fade_seconds;
        }
    }
    reveal.next_index = reveal.spans.len();
    reveal.accumulator = 0.0;
}

fn apply_text_effect_spec(
    effect: &mut DialogueTextEffect,
    spec: crate::script::DialogueTextEffectSpec,
) {
    if let Some(mode) = spec.mode.as_deref() {
        effect.mode = match mode {
            "instant" => DialogueTextEffectMode::Instant,
            _ => DialogueTextEffectMode::TypewriterFade,
        };
    }
    if let Some(cps) = spec.cps {
        effect.cps = cps.max(1.0);
    }
    if let Some(fade_seconds) = spec.fade_seconds {
        effect.fade_seconds = fade_seconds.max(0.0);
    }
    if let Some(fade_ms) = spec.fade_ms {
        effect.fade_seconds = (fade_ms / 1000.0).max(0.0);
    }
}

fn text_effect_snapshot(effect: &DialogueTextEffect) -> TextEffectSnapshot {
    TextEffectSnapshot {
        mode: match effect.mode {
            DialogueTextEffectMode::Instant => "instant".to_string(),
            DialogueTextEffectMode::TypewriterFade => "typewriter_fade".to_string(),
        },
        cps: effect.cps,
        fade_seconds: effect.fade_seconds,
    }
}

fn dialogue_text_effect_from_snapshot(snapshot: &TextEffectSnapshot) -> DialogueTextEffect {
    let mut effect = DialogueTextEffect::default();
    if snapshot.mode == "instant" {
        effect.mode = DialogueTextEffectMode::Instant;
    }
    if snapshot.cps > 0.0 {
        effect.cps = snapshot.cps;
    }
    if snapshot.fade_seconds >= 0.0 {
        effect.fade_seconds = snapshot.fade_seconds;
    }
    effect
}

fn refresh_dialogue_ui_style(
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    dialogue_root_node: &mut Query<&mut Node, With<DialogueRoot>>,
    dialogue_background: &mut Query<&mut BackgroundColor, With<DialogueRoot>>,
    dialogue_border: &mut Query<&mut BorderColor, With<DialogueRoot>>,
    speaker_font: &mut Query<&mut TextFont, (With<SpeakerText>, Without<LineText>)>,
    line_font: &mut Query<&mut TextFont, (With<LineText>, Without<SpeakerText>)>,
    hint_font: &mut Query<&mut TextFont, (With<HintText>, Without<SpeakerText>, Without<LineText>)>,
    hint_visibility: &mut Query<&mut Visibility, (With<HintText>, Without<DialogueRoot>)>,
    speaker_color: &mut Query<
        &mut TextColor,
        (With<SpeakerText>, Without<LineText>, Without<HintText>),
    >,
    line_color: &mut Query<
        &mut TextColor,
        (With<LineText>, Without<SpeakerText>, Without<HintText>),
    >,
    hint_color: &mut Query<
        &mut TextColor,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
) {
    if let Ok(mut node) = dialogue_root_node.single_mut() {
        node.left = px(ui_style.dialogue_left);
        node.right = px(ui_style.dialogue_right);
        node.bottom = px(ui_style.dialogue_bottom);
        node.min_height = px(ui_style.dialogue_min_height);
        node.padding = UiRect::axes(
            px(ui_style.dialogue_padding_x),
            px(ui_style.dialogue_padding_y),
        );
        node.border_radius = BorderRadius::all(px(ui_style.dialogue_radius));
    }
    if let Ok(mut color) = dialogue_background.single_mut() {
        *color = ui_style.dialogue_bg.into();
    }
    if let Ok(mut color) = dialogue_border.single_mut() {
        *color = BorderColor::all(ui_style.dialogue_border);
    }
    if let Ok(mut font) = speaker_font.single_mut() {
        *font = ui_text_font(ui_fonts, ui_style.speaker_size);
    }
    if let Ok(mut font) = line_font.single_mut() {
        *font = ui_text_font(ui_fonts, ui_style.line_size);
    }
    if let Ok(mut font) = hint_font.single_mut() {
        *font = ui_text_font(ui_fonts, ui_style.hint_size);
    }
    if let Ok(mut visibility) = hint_visibility.single_mut() {
        *visibility = if ui_style.hint_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if let Ok(mut color) = speaker_color.single_mut() {
        *color = ui_style.speaker_color.into();
    }
    if let Ok(mut color) = line_color.single_mut() {
        *color = ui_style.line_color.into();
    }
    if let Ok(mut color) = hint_color.single_mut() {
        *color = ui_style.hint_color.into();
    }
}

fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn runtime_menu_action_from_str(value: &str) -> Option<RuntimeMenuButtonAction> {
    match value {
        "quick_save" | "quick-save" => Some(RuntimeMenuButtonAction::QuickSave),
        "quick_load" | "quick-load" => Some(RuntimeMenuButtonAction::QuickLoad),
        "menu" | "open_menu" | "open-menu" => Some(RuntimeMenuButtonAction::OpenPauseMenu),
        "return" | "resume" => Some(RuntimeMenuButtonAction::Resume),
        "main_menu" | "main-menu" => Some(RuntimeMenuButtonAction::ReturnToTitle),
        other => {
            warn!("unknown screen button action `{other}`");
            None
        }
    }
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

fn spawn_choice_ui(
    commands: &mut Commands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    prompt: &str,
    options: &[ChoiceOption],
) {
    let root = commands
        .spawn((
            ChoiceUi,
            Node {
                position_type: PositionType::Absolute,
                left: if ui_style.choice_panel_width > 0.0 {
                    Val::Auto
                } else {
                    px(24.0)
                },
                right: if ui_style.choice_panel_width > 0.0 {
                    Val::Auto
                } else {
                    px(24.0)
                },
                bottom: px(ui_style.choice_bottom),
                width: if ui_style.choice_panel_width > 0.0 {
                    px(ui_style.choice_panel_width)
                } else {
                    percent(100.0)
                },
                max_width: percent(92.0),
                padding: UiRect::all(px(ui_style.choice_padding)),
                flex_direction: FlexDirection::Column,
                row_gap: px(ui_style.choice_gap),
                justify_self: JustifySelf::Center,
                align_self: AlignSelf::Center,
                ..default()
            },
            BackgroundColor(ui_style.choice_panel_bg),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        if !prompt.is_empty() {
            parent.spawn((
                ChoiceUi,
                Text::new(prompt),
                ui_text_font(ui_fonts, ui_style.choice_prompt_size),
                TextColor(ui_style.choice_prompt_color),
            ));
        }

        for (index, option) in options.iter().enumerate() {
            parent
                .spawn((
                    ChoiceUi,
                    ChoiceButton { index },
                    Button,
                    Node {
                        width: percent(100.0),
                        border: UiRect::all(px(1.0)),
                        padding: UiRect::axes(px(18.0), px(14.0)),
                        justify_content: if ui_style.choice_center_text {
                            JustifyContent::Center
                        } else {
                            JustifyContent::FlexStart
                        },
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(12.0)),
                        ..default()
                    },
                    BackgroundColor(ui_style.choice_button_bg),
                    BorderColor::all(ui_style.choice_button_border),
                ))
                .with_children(|button| {
                    let label = if ui_style.choice_show_indices {
                        format!("{}. {}", index + 1, option.text)
                    } else {
                        option.text.clone()
                    };
                    button.spawn((
                        ChoiceUi,
                        Text::new(label),
                        ui_text_font(ui_fonts, ui_style.choice_button_size),
                        TextColor(ui_style.choice_text_color),
                    ));
                });
        }
    });
}

fn clear_choice_ui(
    commands: &mut Commands,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    let entities = choice_ui.iter().collect::<Vec<_>>();
    for entity in entities {
        commands.entity(entity).try_despawn();
    }
}

fn resolve_choice(
    commands: &mut Commands,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    index: usize,
) {
    let Some(selected) = choice_state.options.get(index).cloned() else {
        return;
    };
    let Some(done) = choice_state.waiting.take() else {
        return;
    };

    clear_choice_ui(commands, choice_ui);
    choice_state.options.clear();
    commands.write_message(ScriptResponseMessage {
        request: done,
        response: ScriptResponse::Choice(selected.value),
    });
}

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(mut ctx: RuntimeMenuContext) {
    for (interaction, mut color, button) in &mut ctx.interaction_query {
        match *interaction {
            PickingInteraction::Pressed => {
                *color = ctx.ui_style.quick_button_pressed.into();
                match button.action {
                    RuntimeMenuButtonAction::QuickSave => {
                        let _ = save_runtime_slot("quick", &ctx.script_runtime, &ctx.shared_state);
                    }
                    RuntimeMenuButtonAction::QuickLoad => {
                        let Ok(save_data) = load_save_data("quick") else {
                            continue;
                        };
                        abort_runtime_waiters(
                            &mut ctx.commands,
                            &mut ctx.waits,
                            &mut ctx.dialogue_state,
                            &mut ctx.choice_state,
                            &mut ctx.screen_state,
                            &mut ctx.pending_script_commands,
                            &mut ctx.active_batches,
                            &mut ctx.pending_characters,
                            &mut ctx.animations,
                            &mut ctx.voice_state,
                            &ctx.choice_ui_roots,
                        );
                        close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
                        clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
                        start_frontend_session(
                            &mut ctx.commands,
                            &ctx.asset_server,
                            &ctx.vfs,
                            &mut ctx.shared_state,
                            &mut ctx.stage,
                            &mut ctx.dialogue_state,
                            &mut ctx.choice_state,
                            &ctx.choice_ui_roots,
                            &mut ctx.dialogue_root,
                            &mut ctx.speaker_text,
                            &mut ctx.line_text,
                            &ctx.user_settings,
                            &mut ctx.frontend,
                            &mut ctx.script_runtime,
                            ScriptBootstrap::from_save(&save_data),
                            save_data.scene.clone(),
                        );
                    }
                    RuntimeMenuButtonAction::OpenPauseMenu => {
                        if ctx.runtime_menu.pause_root.is_none() {
                            ctx.runtime_menu.pause_root = Some(spawn_pause_menu(
                                &mut ctx.commands,
                                &ctx.ui_fonts,
                                &ctx.ui_style,
                            ));
                        }
                        ctx.runtime_menu.pause_open = true;
                    }
                    RuntimeMenuButtonAction::Resume => {
                        close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
                    }
                    RuntimeMenuButtonAction::ReturnToTitle => {
                        let startup_script = ctx.frontend.startup_script.clone();
                        abort_runtime_waiters(
                            &mut ctx.commands,
                            &mut ctx.waits,
                            &mut ctx.dialogue_state,
                            &mut ctx.choice_state,
                            &mut ctx.screen_state,
                            &mut ctx.pending_script_commands,
                            &mut ctx.active_batches,
                            &mut ctx.pending_characters,
                            &mut ctx.animations,
                            &mut ctx.voice_state,
                            &ctx.choice_ui_roots,
                        );
                        close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
                        clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
                        clear_overlay_ui(&mut ctx.commands, &mut ctx.overlay_state);
                        start_frontend_session(
                            &mut ctx.commands,
                            &ctx.asset_server,
                            &ctx.vfs,
                            &mut ctx.shared_state,
                            &mut ctx.stage,
                            &mut ctx.dialogue_state,
                            &mut ctx.choice_state,
                            &ctx.choice_ui_roots,
                            &mut ctx.dialogue_root,
                            &mut ctx.speaker_text,
                            &mut ctx.line_text,
                            &ctx.user_settings,
                            &mut ctx.frontend,
                            &mut ctx.script_runtime,
                            ScriptBootstrap::new(startup_script),
                            SceneSnapshot::default(),
                        );
                    }
                }
            }
            PickingInteraction::Hovered => {
                *color = ctx.ui_style.quick_button_hovered.into();
            }
            PickingInteraction::None => {
                *color = ctx.ui_style.quick_button_bg.into();
            }
        }
    }
}

fn spawn_pause_menu(commands: &mut Commands, ui_fonts: &UiFonts, ui_style: &UiStyle) -> Entity {
    let root = commands
        .spawn((
            PauseMenuRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(0.0),
                right: px(0.0),
                top: px(0.0),
                bottom: px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.35)),
            Visibility::Inherited,
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Node {
                    width: px(360.0),
                    padding: UiRect::all(px(20.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10.0),
                    border: UiRect::all(px(1.0)),
                    border_radius: BorderRadius::all(px(18.0)),
                    ..default()
                },
                BackgroundColor(ui_style.choice_panel_bg),
                BorderColor::all(ui_style.choice_button_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Game Menu"),
                    ui_text_font(ui_fonts, 30.0),
                    TextColor(ui_style.speaker_color),
                ));
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Return",
                    RuntimeMenuButtonAction::Resume,
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Quick Save",
                    RuntimeMenuButtonAction::QuickSave,
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Quick Load",
                    RuntimeMenuButtonAction::QuickLoad,
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Main Menu",
                    RuntimeMenuButtonAction::ReturnToTitle,
                );
            });
    });

    root
}

fn spawn_runtime_menu_button(
    parent: &mut bevy::ecs::hierarchy::ChildSpawnerCommands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    action: RuntimeMenuButtonAction,
) {
    parent
        .spawn((
            RuntimeMenuButton { action },
            Button,
            Node {
                width: Val::Auto,
                border: UiRect::all(px(1.0)),
                padding: UiRect::axes(px(10.0), px(4.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(px(8.0)),
                ..default()
            },
            BackgroundColor(ui_style.quick_button_bg),
            BorderColor::all(ui_style.quick_button_border),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(text),
                ui_text_font(ui_fonts, ui_style.quick_button_size),
                TextColor(ui_style.quick_text_color),
            ));
        });
}

fn close_pause_menu(commands: &mut Commands, runtime_menu: &mut RuntimeMenuState) {
    runtime_menu.pause_open = false;
    if let Some(root) = runtime_menu.pause_root.take() {
        commands.entity(root).try_despawn();
    }
}

#[allow(clippy::too_many_arguments)]
fn abort_runtime_waiters(
    commands: &mut Commands,
    waits: &mut PendingWaits,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    screen_state: &mut ScreenUiState,
    pending_script_commands: &mut PendingScriptCommands,
    active_batches: &mut ActiveScriptBatches,
    pending_characters: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
    choice_ui_roots: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    clear_choice_ui(commands, choice_ui_roots);
    choice_state.options.clear();
    choice_state.waiting.take();
    screen_state.waiting.take();
    dialogue_state.waiting.take();
    waits.items.clear();
    pending_script_commands.items.clear();
    active_batches.items.clear();
    pending_characters.items.clear();
    animations.waits.clear();
    finish_all_voices(commands, animations, voice_state);
}

#[allow(clippy::too_many_arguments)]
fn restore_scene_snapshot(
    commands: &mut Commands,
    asset_server: &AssetServer,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    dialogue_root: &mut Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    user_settings: &UserSettings,
    snapshot: SceneSnapshot,
) {
    let text_effect = snapshot.text_effect.clone();

    if let Some(background) = stage.background.take() {
        commands.entity(background).try_despawn();
    }
    if let Some(effect) = stage.screen_effect.take() {
        commands.entity(effect).try_despawn();
    }
    if let Some(transition) = stage.transition.take() {
        commands.entity(transition).try_despawn();
    }
    for (_, entity) in stage.sprites.drain() {
        commands.entity(entity).try_despawn();
    }
    stage.character_positions.clear();
    if let Some(bgm) = stage.bgm.take() {
        commands.entity(bgm).try_despawn();
    }

    if let Some(background) = snapshot.background.as_ref() {
        stage.background = Some(
            commands
                .spawn((
                    BackgroundLayer {
                        path: background.path.clone(),
                    },
                    WorldSprite::from_image(asset_server.load(background.path.clone())),
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id(),
        );
    }

    for sprite in &snapshot.sprites {
        let mut entity_sprite = WorldSprite::from_image(asset_server.load(sprite.path.clone()));
        entity_sprite.color.set_alpha(sprite.alpha);
        entity_sprite.rect = sprite.rect.map(source_rect_from_corners);
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite.id.clone(),
                    path: sprite.path.clone(),
                },
                entity_sprite,
                Transform {
                    translation: Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer),
                    scale: Vec3::splat(sprite.scale),
                    ..default()
                },
            ))
            .id();
        if sprite.focused {
            commands
                .entity(entity)
                .try_insert((FocusedActorPart, focus_layer()));
        } else {
            commands.entity(entity).try_insert(scene_layer());
        }
        stage.sprites.insert(sprite.id.clone(), entity);
    }

    stage.character_positions = snapshot
        .character_positions
        .iter()
        .map(|(actor_id, position)| (actor_id.clone(), Vec2::new(position[0], position[1])))
        .collect();

    if let Some(bgm) = snapshot.bgm.as_ref() {
        let playback_volume = apply_volume_setting(bgm.volume, user_settings.bgm_volume);
        stage.bgm = Some(
            commands
                .spawn((
                    BgmChannel {
                        path: bgm.path.clone(),
                        volume: bgm.volume,
                    },
                    AudioPlayer::new(asset_server.load(bgm.path.clone())),
                    PlaybackSettings::LOOP.with_volume(Volume::Linear(playback_volume)),
                ))
                .id(),
        );
    }

    if let Some(overlay) = stage.overlay {
        commands.entity(overlay).insert(WorldSprite::from_color(
            Color::BLACK.with_alpha(snapshot.overlay_alpha),
            Vec2::new(6000.0, 6000.0),
        ));
    }

    match snapshot.dialogue {
        Some(dialogue) => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Visible;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = dialogue.speaker;
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = dialogue.text;
            }
        }
        None => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Hidden;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = String::new();
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = String::new();
            }
        }
    }

    dialogue_state.effect = dialogue_text_effect_from_snapshot(&text_effect);
    dialogue_state.waiting = None;
    choice_state.waiting = None;
    choice_state.options.clear();
}

fn finish_active_voice(
    commands: &mut Commands,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    let Some(active) = voice_state.active.take() else {
        return;
    };

    finish_voice(commands, animations, active);
}

fn finish_all_voices(
    commands: &mut Commands,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    finish_active_voice(commands, animations, voice_state);
    let concurrent = std::mem::take(&mut voice_state.concurrent);
    for active in concurrent.into_values() {
        finish_voice(commands, animations, active);
    }
}

fn finish_voice(commands: &mut Commands, animations: &mut AnimationState, mut active: ActiveVoice) {
    commands.entity(active.entity).try_despawn();
    if let Some(animation_id) = active.animation_id.take() {
        animations.completed.insert(animation_id);
    }
}

pub(crate) fn complete_missing_animation(
    animations: &mut AnimationState,
    animation_id: Option<String>,
) {
    if let Some(animation_id) = animation_id {
        animations.completed.insert(animation_id);
    }
}

fn complete_dialogue_wait(
    commands: &mut Commands,
    animations: &mut AnimationState,
    waiting: PendingDialogueAdvance,
) {
    complete_missing_animation(animations, waiting.animation_id);
    if let Some(request) = waiting.request {
        commands.write_message(ScriptResponseMessage {
            request,
            response: ScriptResponse::Continue,
        });
    }
}

pub(crate) fn tween_fraction(timer: &Timer) -> f32 {
    let duration = timer.duration().as_secs_f32();
    if duration <= f32::EPSILON {
        1.0
    } else {
        (timer.elapsed_secs() / duration).clamp(0.0, 1.0)
    }
}
