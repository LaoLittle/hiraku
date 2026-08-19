use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::mpsc,
};

use bevy::{
    app::AppExit,
    audio::{AudioSink, AudioSinkPlayback, Volume},
    ecs::system::SystemParam,
    log::{info, warn},
    math::Rect,
    prelude::*,
    ui::{ComputedStackIndex, UiGlobalTransform},
    window::PrimaryWindow,
};

use crate::{
    audio::{AudioCatalog, load_audio_catalog},
    character::{CharacterCatalog, CharacterPartDefinition, load_character_catalog},
    effect::custom::{CustomScreenEffectMaterial, CustomScreenEffectPlayer},
    effect::transition::{RuleTransitionMaterial, RuleTransitionMesh, RuleTransitionPlayer},
    render::camera::{
        CameraShake, CameraShakeState, CameraState, CameraTweenState, WorldCamera, focus_layer,
        setup_stage_cameras, start_camera_tween,
    },
    script::{
        BatchSubmissionItem, BatchSubmitMode, CharacterEase, IrCommand, IrEvent, IrRuntime, IrVm,
        IrWaitKind, ResolvedCharacterKeyframe, ScriptBootstrap, ScriptCommand, ScriptResponse,
        UiContext, UiIntent, VoicePlaybackMode, compile_story_program, evaluate_ui_script,
        save_runtime_slot, script_command_from_ir, start_hks_runtime,
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
        StaleScreenRoot, TextNode,
    },
    vfs::VfsResource,
};

mod character;
mod screen_ui;

pub(crate) use character::apply_character_ease;
pub use character::{animate_character_motion_effects, poll_pending_character_shows};
use character::{
    apply_character_motion, apply_character_timeline, array_to_rect, despawn_character_actor,
    queue_character_show, rect_to_array,
};
#[cfg(test)]
use screen_ui::window_cursor_to_canvas;
pub use screen_ui::{
    cleanup_stale_screen_ui, handle_screen_buttons, handle_screen_image_buttons,
    update_offscreen_ui_interactions,
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
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

pub struct DialogueRevealState {
    pub spans: Vec<Entity>,
    pub next_index: usize,
    pub accumulator: f32,
    pub interval: f32,
    pub fade_seconds: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    pub waiting: Option<mpsc::Sender<ScriptResponse>>,
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

pub fn bridge_ir_events(
    mut runtime: NonSendMut<IrRuntime>,
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    textures: Option<Res<TextureCatalog>>,
    audio: Option<Res<AudioCatalog>>,
    vfs: Res<VfsResource>,
    user_settings: Res<UserSettings>,
) {
    if let Some(receiver) = runtime.wait_response.as_ref() {
        let response = receiver.try_recv();
        match response {
            Ok(ScriptResponse::Choice(value)) => {
                let value = if let Some(screen) = runtime.pending_ui_screen.take() {
                    UiIntent { screen, value }.value
                } else {
                    value
                };
                if let Some(variable) = runtime.pending_input_variable.take()
                    && let Some(vm) = runtime.vm.as_mut()
                {
                    vm.set_stored_value(variable, value);
                }
                runtime.wait_response = None;
                if let Some(vm) = runtime.vm.as_mut() {
                    vm.resume();
                }
            }
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                runtime.wait_response = None;
                runtime.pending_input_variable = None;
                runtime.pending_ui_screen = None;
                if let Some(vm) = runtime.vm.as_mut() {
                    vm.resume();
                }
            }
            Err(mpsc::TryRecvError::Empty) => return,
        }
    }

    let Some(event) = runtime.events.pop_front() else {
        return;
    };

    match event {
        IrEvent::Command(IrCommand::LoadScript { path }) => {
            let target = vfs.0.resolve_path(runtime.current_script.as_deref(), &path);
            let program = vfs
                .0
                .read_text(&target)
                .map_err(|error| error.to_string())
                .and_then(|source| compile_story_program(&target, &source))
                .and_then(|program| IrVm::new(program).map_err(|error| error.to_string()));
            match program {
                Ok(vm) => {
                    runtime.vm = Some(vm);
                    runtime.current_script = Some(target);
                    runtime.events.clear();
                    runtime.wait_response = None;
                    runtime.pending_input_variable = None;
                    runtime.pending_ui_screen = None;
                    runtime.pending_response = None;
                    runtime.native_tasks.clear();
                }
                Err(error) => warn!("failed to load HKS script `{target}`: {error}"),
            }
        }
        IrEvent::Command(IrCommand::Choose {
            prompt,
            options,
            result,
        }) => {
            let (response_tx, response_rx) = mpsc::channel();
            runtime.pending_input_variable = Some(result);
            runtime.pending_ui_screen = None;
            runtime.pending_response = Some(response_rx);
            pending_script_commands
                .items
                .push_back(ScriptCommand::Choose {
                    prompt,
                    options: options
                        .into_iter()
                        .map(|option| ChoiceOption {
                            text: option.text,
                            value: StoredValue::String(option.value),
                        })
                        .collect(),
                    done: response_tx,
                });
        }
        IrEvent::Command(IrCommand::OpenUi { path, result }) => {
            let target = vfs.0.resolve_path(runtime.current_script.as_deref(), &path);
            let mut story = runtime
                .vm
                .as_ref()
                .map(IrVm::story_values)
                .unwrap_or_default();
            story.insert(
                "bgmVolume".to_string(),
                StoredValue::Float(user_settings.bgm_volume as f64),
            );
            story.insert(
                "voiceVolume".to_string(),
                StoredValue::Float(user_settings.voice_volume as f64),
            );
            story.insert(
                "sfxVolume".to_string(),
                StoredValue::Float(user_settings.sfx_volume as f64),
            );
            let screen = vfs
                .0
                .read_text(&target)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    let textures = textures
                        .as_deref()
                        .ok_or_else(|| "texture catalog is unavailable".to_string())?;
                    evaluate_ui_script(&source, &UiContext::new(story), textures)
                        .map_err(|error| error.to_string())
                });
            let (response_tx, response_rx) = mpsc::channel();
            runtime.pending_input_variable = Some(result);
            runtime.pending_ui_screen = Some(target.clone());
            runtime.pending_response = Some(response_rx);
            match screen {
                Ok(screen) => pending_script_commands
                    .items
                    .push_back(ScriptCommand::ShowScreen {
                        screen,
                        shown: None,
                        done: Some(response_tx),
                    }),
                Err(error) => {
                    warn!("failed to render UI script `{target}`: {error}");
                    runtime.vm = None;
                    runtime.pending_input_variable = None;
                    runtime.pending_ui_screen = None;
                    runtime.pending_response = None;
                }
            }
        }
        IrEvent::Command(IrCommand::PlayBgm {
            path,
            volume,
            fade_in_ms,
        }) => {
            match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_music(&path))
            {
                Some(definition) => {
                    pending_script_commands
                        .items
                        .push_back(ScriptCommand::PlayBgm {
                            path: definition.path.clone(),
                            volume,
                            fade_in: fade_in_ms.map(std::time::Duration::from_millis),
                            animation_id: None,
                            done: None,
                        })
                }
                None => warn!("music `{path}` is not defined"),
            }
        }
        IrEvent::Command(IrCommand::PlayVoice { path, volume }) => {
            match audio
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
                            done: None,
                        })
                }
                None => warn!("voice `{path}` is not defined"),
            }
        }
        IrEvent::Command(IrCommand::WaitHksTask { handle }) => {
            let Some(receiver) = runtime.native_tasks.remove(&handle) else {
                warn!("HKS task handle `{handle}` is not defined");
                return;
            };
            if runtime
                .vm
                .as_mut()
                .is_some_and(|vm| vm.suspend(IrWaitKind::TaskCompletion))
            {
                runtime.wait_response = Some(receiver);
            }
        }
        IrEvent::Command(IrCommand::HksStatement {
            bytecode,
            mut task_result,
        }) => match crate::hks_capabilities::execute(bytecode) {
            Ok(output) => {
                for command in output.commands {
                    if matches!(
                        command,
                        IrCommand::LoadScript { .. }
                            | IrCommand::Choose { .. }
                            | IrCommand::OpenUi { .. }
                            | IrCommand::PlayBgm { .. }
                            | IrCommand::PlayVoice { .. }
                    ) {
                        runtime.events.push_back(IrEvent::Command(command));
                        continue;
                    }
                    match script_command_from_ir(command, textures.as_deref()) {
                        Ok(command) => pending_script_commands.items.push_back(command),
                        Err(error) => warn!("HKS native command rejected: {error}"),
                    }
                }
                for task in output.tasks {
                    let batch_id = runtime.next_native_task_id;
                    runtime.next_native_task_id += 1;
                    let mut items = Vec::new();
                    for (index, command) in task.commands.into_iter().enumerate() {
                        let handle = format!("hks-task-{batch_id}-{index}");
                        let command = match command {
                            IrCommand::PlayVoice { path, volume } => {
                                let Some(definition) = audio
                                    .as_deref()
                                    .and_then(|catalog| catalog.resolve_voice(&path))
                                else {
                                    warn!("voice `{path}` is not defined");
                                    continue;
                                };
                                ScriptCommand::PlayVoice {
                                    path: definition.path.clone(),
                                    volume,
                                    mode: VoicePlaybackMode::Concurrent,
                                    animation_id: Some(handle.clone()),
                                    done: None,
                                }
                            }
                            command @ IrCommand::ShowCharacter { .. } => {
                                match script_command_from_ir(command, textures.as_deref()) {
                                    Ok(ScriptCommand::ShowCharacter {
                                        actor_id,
                                        character_name,
                                        expressions,
                                        position,
                                        scale,
                                        fade,
                                        done,
                                        ..
                                    }) => ScriptCommand::ShowCharacter {
                                        actor_id,
                                        character_name,
                                        expressions,
                                        position,
                                        scale,
                                        fade,
                                        animation_id: Some(handle.clone()),
                                        done,
                                    },
                                    Ok(_) => unreachable!(
                                        "show character IR must stay a show character command"
                                    ),
                                    Err(error) => {
                                        warn!("HKS character task command rejected: {error}");
                                        continue;
                                    }
                                }
                            }
                            _ => {
                                warn!("HKS task command is not yet supported by the story runtime");
                                continue;
                            }
                        };
                        items.push(BatchSubmissionItem {
                            handle: handle.clone(),
                            command: Box::new(command),
                        });
                    }
                    let handles = items
                        .iter()
                        .map(|item| item.handle.clone())
                        .collect::<Vec<_>>();
                    let mode = match task.mode {
                        hiraku_script::hks::vm::TaskMode::Sequence => BatchSubmitMode::Sequence,
                        hiraku_script::hks::vm::TaskMode::Parallel => BatchSubmitMode::Parallel,
                    };
                    if !items.is_empty() {
                        pending_script_commands
                            .items
                            .push_back(ScriptCommand::SubmitBatch { mode, items });
                    }
                    if let Some(handle) = task_result.take() {
                        let (response_tx, response_rx) = mpsc::channel();
                        if handles.is_empty() {
                            let _ = response_tx.send(ScriptResponse::Continue);
                        } else {
                            pending_script_commands.items.push_back(
                                ScriptCommand::WaitAnimations {
                                    ids: handles,
                                    done: response_tx,
                                },
                            );
                        }
                        runtime.native_tasks.insert(handle, response_rx);
                    }
                }
                if let Some(wait) = output.wait
                    && runtime
                        .vm
                        .as_mut()
                        .is_some_and(|vm| vm.suspend(wait.clone()))
                {
                    runtime.events.push_back(IrEvent::Waiting(wait));
                }
            }
            Err(error) => warn!("HKS native statement failed: {error}"),
        },
        IrEvent::Command(command) => match script_command_from_ir(command, textures.as_deref()) {
            Ok(command) => pending_script_commands.items.push_back(command),
            Err(error) => warn!("IR command rejected: {error}"),
        },
        IrEvent::Waiting(wait_kind) => {
            if matches!(wait_kind, IrWaitKind::ScreenChoice | IrWaitKind::UiIntent)
                && let Some(receiver) = runtime.pending_response.take()
            {
                runtime.wait_response = Some(receiver);
                return;
            }
            let (response_tx, response_rx) = mpsc::channel();
            let command = match wait_kind {
                IrWaitKind::DialogueAdvance => {
                    ScriptCommand::AwaitDialogueAdvance { done: response_tx }
                }
                IrWaitKind::ScreenChoice => {
                    ScriptCommand::WaitForScreenChoice { done: response_tx }
                }
                IrWaitKind::UiIntent => ScriptCommand::WaitForScreenChoice { done: response_tx },
                IrWaitKind::TaskCompletion => ScriptCommand::WaitAnimations {
                    ids: Vec::new(),
                    done: response_tx,
                },
                IrWaitKind::DurationMs(milliseconds) => ScriptCommand::Wait {
                    duration: std::time::Duration::from_millis(milliseconds),
                    animation_id: None,
                    done: response_tx,
                },
            };
            runtime.wait_response = Some(response_rx);
            pending_script_commands.items.push_back(command);
        }
        IrEvent::Completed => {}
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
    pub done: mpsc::Sender<ScriptResponse>,
}

pub struct PendingAnimationWait {
    pub ids: Vec<String>,
    pub done: mpsc::Sender<ScriptResponse>,
}

pub struct ActiveVoice {
    pub entity: Entity,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

pub struct PendingCharacterShow {
    pub actor_id: String,
    pub entity_ids: Vec<String>,
    pub entities: Vec<Entity>,
    pub handles: Vec<Handle<Image>>,
    pub fade: Option<std::time::Duration>,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
pub struct CharacterJumpEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub height: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
pub struct CharacterShakeEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub amplitude: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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

#[derive(Component, Clone)]
pub struct BackgroundLayer {
    pub path: String,
}

#[derive(Component, Clone)]
pub struct BgmChannel {
    pub path: String,
    pub volume: f32,
}

#[derive(Component)]
pub struct BgmFade {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    pub ir_runtime: NonSendMut<'w, IrRuntime>,
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub animations: ResMut<'w, AnimationState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub transition_mesh: Res<'w, RuleTransitionMesh>,
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
    pub ir_runtime: NonSendMut<'w, IrRuntime>,
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
            &'static Interaction,
            &'static mut BackgroundColor,
            &'static RuntimeMenuButton,
        ),
        Changed<Interaction>,
    >,
}

pub fn setup_stage(
    mut commands: Commands,
    ui_fonts: Res<UiFonts>,
    ui_style: Res<UiStyle>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    config: Res<crate::RuntimeLaunchConfig>,
    mut shared_state: ResMut<SceneSharedState>,
) {
    setup_stage_cameras(&mut commands, &mut images, &config);
    commands.insert_resource(RuleTransitionMesh(meshes.add(Rectangle::default())));

    let overlay = commands
        .spawn((
            OverlayMarker,
            Sprite::from_color(Color::BLACK.with_alpha(0.0), Vec2::new(6000.0, 6000.0)),
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
            Some("startup.story.hks not found. Fix settings.hson before starting.".to_string());
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
    mut ir_runtime: NonSendMut<IrRuntime>,
    mut interaction_query: Query<
        (&Interaction, &mut BackgroundColor, &FrontendButton),
        Changed<Interaction>,
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
            Interaction::Pressed => {
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
                            &mut ir_runtime,
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
                            &mut ir_runtime,
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
                                &mut ir_runtime,
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
            Interaction::Hovered => {
                *color = FRONTEND_BUTTON_HOVERED.into();
            }
            Interaction::None => {
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
    let mut ir_runtime = ctx.ir_runtime;
    let mut active_batches = ctx.active_batches;
    let mut dialogue_state = ctx.dialogue_state;
    let mut choice_state = ctx.choice_state;
    let mut screen_state = ctx.screen_state;
    let mut overlay_state = ctx.overlay_state;
    let mut animations = ctx.animations;
    let mut voice_state = ctx.voice_state;
    let mut pending_characters = ctx.pending_characters;
    let transition_mesh = ctx.transition_mesh;
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
            ScriptCommand::StartIr { path, program } => match IrVm::new(program) {
                Ok(vm) => {
                    ir_runtime.vm = Some(vm);
                    ir_runtime.current_script = Some(path);
                    ir_runtime.events.clear();
                    ir_runtime.wait_response = None;
                    ir_runtime.pending_input_variable = None;
                    ir_runtime.pending_ui_screen = None;
                    ir_runtime.pending_response = None;
                }
                Err(error) => warn!("failed to start IR program: {error}"),
            },
            ScriptCommand::Log(message) => info!("[hks] {message}"),
            ScriptCommand::SetBackground {
                path,
                fade,
                animation_id,
                done,
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
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
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
                let mut sprite = Sprite::from_image(image);
                let background = if let Some(duration) = fade {
                    sprite.color = sprite.color.with_alpha(0.0);
                    commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
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
                                done,
                                despawn_on_finish: false,
                            },
                        ))
                        .id()
                } else {
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
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
                            done: None,
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
                done,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }

                let Some(previous_background) = stage.background else {
                    let image = asset_server.load(path.clone());
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            Sprite::from_image(image),
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    shared_state.0.background = Some(ImageLayerSnapshot { path });
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
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
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            Sprite::from_image(image),
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
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
                        Mesh2d(transition_mesh.0.clone()),
                        MeshMaterial2d(material.clone()),
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
                            done,
                        },
                    ))
                    .id();
                stage.transition = Some(transition_entity);
            }
            ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
                done,
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
                        Mesh2d(transition_mesh.0.clone()),
                        MeshMaterial2d(material.clone()),
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
                            done,
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
                done,
            } => {
                let handle = asset_server.load(path.clone());
                let entity = if let Some(entity) = stage.sprites.get(&id).copied() {
                    let mut sprite = Sprite::from_image(handle);
                    sprite.rect = rect.map(array_to_rect);
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
                    let mut sprite = Sprite::from_image(handle);
                    sprite.rect = rect.map(array_to_rect);
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    let entity = commands
                        .spawn((
                            SpriteActor {
                                id: id.clone(),
                                path: path.clone(),
                            },
                            sprite,
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
                        done,
                        despawn_on_finish: false,
                    });
                } else {
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::HideSprite {
                id,
                fade,
                animation_id,
                done,
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
                            done,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(entity).try_despawn();
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::SetOverlay {
                alpha,
                fade,
                animation_id,
                done,
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
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        commands.entity(overlay).insert(Sprite::from_color(
                            Color::BLACK.with_alpha(alpha),
                            Vec2::new(6000.0, 6000.0),
                        ));
                        if let Some(done) = done {
                            let _ = done.send(ScriptResponse::Continue);
                        }
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
                done,
            } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
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
                        None,
                    );
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance { animation_id, done });
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::AwaitDialogueAdvance { done } => {
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id: None,
                    done: Some(done),
                });
            }
            ScriptCommand::SetDialogue {
                speaker,
                text,
                reveal_from,
                animation_id,
                done,
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
                        done,
                    );
                }
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::ClearDialogue => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
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
                scope,
                center,
                duration,
                ease,
                animation_id,
                done,
            } => start_camera_tween(
                &mut camera_state,
                &mut camera_tweens,
                blur_intensity,
                zoom,
                scope,
                center,
                duration,
                ease,
                animation_id,
                done,
                &mut animations,
            ),
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
            ScriptCommand::ShowScreen {
                screen,
                shown,
                done,
            } => {
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
                    if let Some(shown) = shown {
                        let _ = shown.send(ScriptResponse::Continue);
                    }
                } else {
                    commands
                        .entity(root)
                        .insert((Visibility::Hidden, GlobalZIndex(SCREEN_PENDING_Z)));
                    screen_state.pending_root = Some(crate::ui::PendingScreenRoot {
                        entity: root,
                        previous,
                        wait_images: spawned.image_handles,
                        ready_frames_remaining: SCREEN_READY_FRAMES,
                        shown,
                        done,
                    });
                    screen_state.waiting = None;
                }
            }
            ScriptCommand::WaitForScreenChoice { done } => {
                if screen_state.active_root.is_some() && screen_state.pending_root.is_none() {
                    screen_state.waiting = Some(done);
                } else {
                    let _ = done.send(ScriptResponse::Continue);
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
                fade,
                animation_id,
                done,
            } => {
                let Some(character) = characters.characters.get(&character_name).cloned() else {
                    warn!("character `{character_name}` not found in catalog");
                    complete_missing_animation(&mut animations, animation_id, done);
                    continue;
                };
                let parts = match character.parts_for_expressions(&expressions) {
                    Ok(parts) => parts,
                    Err(message) => {
                        warn!("{message}");
                        complete_missing_animation(&mut animations, animation_id, done);
                        continue;
                    }
                };

                despawn_character_actor(
                    &mut commands,
                    &mut stage,
                    &mut pending_characters,
                    &actor_id,
                );
                stage.character_positions.insert(actor_id.clone(), position);
                queue_character_show(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &mut stage,
                    &mut pending_characters,
                    &mut animations,
                    actor_id,
                    parts,
                    position,
                    scale,
                    fade,
                    animation_id,
                    done,
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
                done,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Jump { height },
                    duration,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::ShakeCharacter {
                actor_id,
                amplitude,
                duration,
                animation_id,
                done,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Shake { amplitude },
                    duration,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::AnimateCharacter {
                actor_id,
                keyframes,
                animation_id,
                done,
            } => {
                apply_character_timeline(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    keyframes,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::RestoreSnapshot { snapshot, done } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                // clear_overlay_ui(&mut commands, &mut overlay_state);
                commands.insert_resource(CameraShakeState::default());
                commands.insert_resource(AnimationState::default());
                pending_characters.items.clear();
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
                let _ = done.send(ScriptResponse::Continue);
            }
            ScriptCommand::MoveSprite {
                id,
                position,
                duration,
                animation_id,
                done,
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
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during move");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for move_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::ScaleSprite {
                id,
                scale,
                duration,
                animation_id,
                done,
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
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during scale");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for scale_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::FadeSprite {
                id,
                alpha,
                duration,
                animation_id,
                done,
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
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during fade");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for fade_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
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
                    let _ = done.send(ScriptResponse::Continue);
                } else {
                    animations.waits.push(PendingAnimationWait { ids, done });
                }
            }
            ScriptCommand::Shake {
                duration,
                amplitude,
                animation_id,
                done,
            } => {
                commands.insert_resource(CameraShakeState {
                    active: Some(CameraShake {
                        timer: Timer::new(duration, TimerMode::Once),
                        amplitude,
                        animation_id,
                        done,
                    }),
                });
            }
            ScriptCommand::PlayBgm {
                path,
                volume,
                fade_in,
                animation_id,
                done,
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
                let bgm = commands
                    .spawn((
                        BgmChannel {
                            path: path.clone(),
                            volume,
                        },
                        AudioPlayer::new(asset_server.load(path.clone())),
                        PlaybackSettings::LOOP.with_volume(Volume::Linear(start_volume)),
                    ))
                    .id();
                if let Some(fade_in) = fade_in {
                    commands.entity(bgm).insert(BgmFade {
                        from: start_volume,
                        to: playback_volume,
                        timer: Timer::new(fade_in, TimerMode::Once),
                        animation_id,
                        done,
                    });
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
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
                        done: None,
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
                done,
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
                        done,
                    });
                    if let Some(snapshot) = shared_state.0.bgm.as_mut() {
                        snapshot.volume = volume;
                    }
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
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
                done,
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
                    done,
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
                    let program = vfs
                        .0
                        .read_text(&startup)
                        .map_err(|error| error.to_string())
                        .and_then(|source| compile_story_program(&startup, &source))
                        .and_then(|program| IrVm::new(program).map_err(|error| error.to_string()));
                    match program {
                        Ok(vm) => {
                            ir_runtime.vm = Some(vm);
                            ir_runtime.current_script = Some(startup);
                            ir_runtime.events.clear();
                            ir_runtime.wait_response = None;
                            ir_runtime.pending_input_variable = None;
                            ir_runtime.pending_ui_screen = None;
                            ir_runtime.pending_response = None;
                        }
                        Err(error) => warn!("failed to return to HKS title: {error}"),
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
        fade.timer.tick(time.delta());
        let fraction = tween_fraction(&fade.timer);
        let volume = fade.from + (fade.to - fade.from) * fraction;

        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(volume));
        }

        if fade.timer.is_finished() {
            if let Some(animation_id) = fade.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if let Some(done) = fade.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
            commands.entity(entity).try_remove::<BgmFade>();
        }
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
        (&Interaction, &mut BackgroundColor, &ChoiceButton),
        Changed<Interaction>,
    >,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                *color = ui_style.choice_button_pressed.into();
                resolve_choice(&mut commands, &mut choice_state, &choice_ui, button.index);
            }
            Interaction::Hovered => {
                *color = ui_style.choice_button_hovered.into();
            }
            Interaction::None => {
                *color = ui_style.choice_button_bg.into();
            }
        }
    }
}

/// Maps primary-window pointer input into Hiraku's fixed offscreen canvas.
///
/// Bevy's built-in UI focus system only produces [`Interaction`] for cameras targeting a window.
/// Hiraku renders UI into an image, so the engine performs the equivalent topmost-node hit test in
/// canvas coordinates before the existing semantic button handlers run.
pub fn handle_choice_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    if choice_state.waiting.is_none() {
        return;
    }

    let selected = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::Numpad1,
        KeyCode::Numpad2,
        KeyCode::Numpad3,
        KeyCode::Numpad4,
        KeyCode::Numpad5,
        KeyCode::Numpad6,
        KeyCode::Numpad7,
        KeyCode::Numpad8,
        KeyCode::Numpad9,
    ]
    .into_iter()
    .find_map(|key| {
        if keys.just_pressed(key) {
            Some(match key {
                KeyCode::Digit1 | KeyCode::Numpad1 => 0,
                KeyCode::Digit2 | KeyCode::Numpad2 => 1,
                KeyCode::Digit3 | KeyCode::Numpad3 => 2,
                KeyCode::Digit4 | KeyCode::Numpad4 => 3,
                KeyCode::Digit5 | KeyCode::Numpad5 => 4,
                KeyCode::Digit6 | KeyCode::Numpad6 => 5,
                KeyCode::Digit7 | KeyCode::Numpad7 => 6,
                KeyCode::Digit8 | KeyCode::Numpad8 => 7,
                KeyCode::Digit9 | KeyCode::Numpad9 => 8,
                _ => unreachable!(),
            })
        } else {
            None
        }
    });

    if let Some(index) = selected {
        resolve_choice(&mut commands, &mut choice_state, &choice_ui, index);
    }
}

pub fn advance_dialogue_on_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<&mut DialogueCharSpan>,
    choice_state: Res<ChoiceState>,
    runtime_menu: Res<RuntimeMenuState>,
    ui_interactions: Query<
        &Interaction,
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

    let advance = keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Space)
        || mouse.just_pressed(MouseButton::Left);

    if !advance {
        return;
    }

    if mouse.just_pressed(MouseButton::Left)
        && ui_interactions
            .iter()
            .any(|interaction| !matches!(*interaction, Interaction::None))
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
        if let Some(done) = waiting.done {
            let _ = done.send(ScriptResponse::Continue);
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
        if let Some(done) = reveal.done.take() {
            let _ = done.send(ScriptResponse::Continue);
        }
        dialogue_state.reveal = None;
    }
}

pub fn tick_pending_waits(
    time: Res<Time>,
    mut waits: ResMut<PendingWaits>,
    mut animations: ResMut<AnimationState>,
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
        let _ = done.send(ScriptResponse::Continue);
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
            let _ = wait.done.send(ScriptResponse::Continue);
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
            complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
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
        complete_missing_animation(
            &mut animations,
            shake.animation_id.take(),
            shake.done.take(),
        );
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
            complete_missing_animation(&mut animations, item.animation_id.take(), item.done.take());
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
            complete_missing_animation(
                &mut animations,
                tween.animation_id.take(),
                tween.done.take(),
            );
            commands.entity(entity).try_remove::<VisualTween>();
        }
    }

    for (entity, mut fade) in &mut bgm_fades {
        if fade
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, fade.animation_id.take(), fade.done.take());
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
            complete_missing_animation(
                &mut animations,
                effect.animation_id.take(),
                effect.done.take(),
            );
            commands.entity(entity).try_remove::<CharacterJumpEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = shake
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(
                &mut animations,
                effect.animation_id.take(),
                effect.done.take(),
            );
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
            complete_missing_animation(
                &mut animations,
                effect.animation_id.take(),
                effect.done.take(),
            );
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
            complete_missing_animation(
                &mut animations,
                transition.animation_id.take(),
                transition.done.take(),
            );
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
            complete_missing_animation(
                &mut animations,
                effect.animation_id.take(),
                effect.done.take(),
            );
            commands.entity(entity).try_despawn();
        }
    }
}

pub fn animate_visual_tweens(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut sprites: Query<(Entity, &mut Sprite, &mut Transform, &mut VisualTween)>,
) {
    for (entity, mut sprite, mut transform, mut tween) in &mut sprites {
        tween.timer.tick(time.delta());
        let fraction = tween_fraction(&tween.timer);

        if let (Some(from), Some(to)) = (tween.from_alpha, tween.to_alpha) {
            sprite.color.set_alpha(from + (to - from) * fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_translation, tween.to_translation) {
            transform.translation = from.lerp(to, fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_scale, tween.to_scale) {
            transform.scale = from.lerp(to, fraction);
        }

        if tween.timer.is_finished() {
            if let Some(to) = tween.to_alpha {
                sprite.color.set_alpha(to);
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
            if let Some(done) = tween.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
            if tween.despawn_on_finish {
                commands.entity(entity).try_despawn();
            } else {
                commands.entity(entity).try_remove::<VisualTween>();
            }
        }
    }
}

pub fn animate_rule_transitions(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut rule_materials: ResMut<Assets<RuleTransitionMaterial>>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
) {
    for (entity, mut transition) in &mut transitions {
        transition.timer.tick(time.delta());
        let progress = tween_fraction(&transition.timer);

        if let Some(mut material) = rule_materials.get_mut(&transition.material) {
            material.progress = progress;
        }

        if transition.timer.is_finished() {
            let new_background = commands
                .spawn((
                    BackgroundLayer {
                        path: transition.target_path.clone(),
                    },
                    Sprite::from_image(transition.target_image.clone()),
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
            if let Some(done) = transition.done.take() {
                let _ = done.send(ScriptResponse::Continue);
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
                let new_background = commands
                    .spawn((
                        BackgroundLayer { path: target_path },
                        Sprite::from_image(target_image),
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
            if let Some(done) = effect.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
        }
    }
}

pub fn tick_animation_waits(mut animations: ResMut<AnimationState>) {
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
        let _ = done.send(ScriptResponse::Continue);
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
    dialogue_state: Res<DialogueState>,
    background_layers: Query<&BackgroundLayer>,
    bgms: Query<&BgmChannel>,
    overlay: Query<&Sprite, With<OverlayMarker>>,
    sprites: Query<(&SpriteActor, &Sprite, &Transform)>,
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
        .map(|(actor, sprite, transform)| SpriteSnapshot {
            id: actor.id.clone(),
            path: actor.path.clone(),
            x: transform.translation.x,
            y: transform.translation.y,
            layer: transform.translation.z - STAGE_Z_SPRITE,
            scale: transform.scale.x,
            alpha: sprite.color.alpha(),
            rect: sprite.rect.map(rect_to_array),
        })
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
    ir_runtime: &mut IrRuntime,
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

    if let Err(error) = start_hks_runtime(vfs, ir_runtime, bootstrap) {
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
    done: Option<mpsc::Sender<ScriptResponse>>,
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
            done,
        });
    } else {
        dialogue_state.reveal = None;
        if let Some(animation_id) = animation_id {
            if let Some(done) = done {
                let _ = done.send(ScriptResponse::Continue);
            }
            let _ = animation_id;
        } else if let Some(done) = done {
            let _ = done.send(ScriptResponse::Continue);
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
    let _ = done.send(ScriptResponse::Choice(selected.value));
}

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(mut ctx: RuntimeMenuContext) {
    for (interaction, mut color, button) in &mut ctx.interaction_query {
        match *interaction {
            Interaction::Pressed => {
                *color = ctx.ui_style.quick_button_pressed.into();
                match button.action {
                    RuntimeMenuButtonAction::QuickSave => {
                        let _ = save_runtime_slot("quick", &ctx.ir_runtime, &ctx.shared_state);
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
                            &mut ctx.ir_runtime,
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
                            &mut ctx.ir_runtime,
                            ScriptBootstrap::new(startup_script),
                            SceneSnapshot::default(),
                        );
                    }
                }
            }
            Interaction::Hovered => {
                *color = ctx.ui_style.quick_button_hovered.into();
            }
            Interaction::None => {
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
                    Sprite::from_image(asset_server.load(background.path.clone())),
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id(),
        );
    }

    for sprite in &snapshot.sprites {
        let mut entity_sprite = Sprite::from_image(asset_server.load(sprite.path.clone()));
        entity_sprite.color.set_alpha(sprite.alpha);
        entity_sprite.rect = sprite.rect.map(array_to_rect);
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
        commands.entity(overlay).insert(Sprite::from_color(
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
    if let Some(done) = active.done.take() {
        let _ = done.send(ScriptResponse::Continue);
    }
}

pub(crate) fn complete_missing_animation(
    animations: &mut AnimationState,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
) {
    if let Some(animation_id) = animation_id {
        animations.completed.insert(animation_id);
    }
    if let Some(done) = done {
        let _ = done.send(ScriptResponse::Continue);
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

#[cfg(test)]
mod tests {
    use super::window_cursor_to_canvas;
    use bevy::prelude::Vec2;

    #[test]
    fn maps_pointer_through_letterboxing() {
        let canvas = Vec2::new(1280.0, 720.0);
        let mapped =
            window_cursor_to_canvas(Vec2::new(1000.0, 1000.0), Vec2::new(500.0, 500.0), canvas);
        assert_eq!(mapped, Some(Vec2::new(640.0, 360.0)));
        assert_eq!(
            window_cursor_to_canvas(Vec2::new(1000.0, 1000.0), Vec2::new(500.0, 100.0), canvas,),
            None
        );
    }

    #[test]
    fn maps_pointer_through_pillarboxing() {
        let mapped = window_cursor_to_canvas(
            Vec2::new(2000.0, 720.0),
            Vec2::new(1000.0, 360.0),
            Vec2::new(1280.0, 720.0),
        );
        assert_eq!(mapped, Some(Vec2::new(640.0, 360.0)));
    }
}
