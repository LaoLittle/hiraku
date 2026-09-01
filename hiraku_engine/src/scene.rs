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
    animate_custom_effects, animate_rule_transitions, animate_visual_tweens, tick_animation_waits,
    tick_pending_waits, tick_script_batches,
};
pub(crate) use animation_runtime::{complete_missing_animation, tween_fraction};
use audio_runtime::{
    BgmChannel, BgmFade, BgmPrelude, SfxChannel, VoiceChannel, apply_volume_setting,
    finish_active_voice, finish_all_voices, finish_voice,
};
pub use audio_runtime::{
    animate_bgm_fades, apply_live_audio_settings, prepare_bgm_preludes, reconcile_restored_bgm,
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
pub use command_runtime::{PendingScriptCommands, process_script_commands};
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
    PauseMenuRoot, RuntimeMenuButton, RuntimeMenuButtonAction, RuntimeMenuContext,
    RuntimeMenuState, update_runtime_menu_button_visuals,
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
    evaluate_ui_at_with(
        target,
        runtime,
        vfs,
        user_settings,
        textures,
        terms,
        BTreeMap::new(),
    )
}

fn evaluate_ui_at_with(
    target: &str,
    runtime: &ScriptRuntimeState,
    vfs: &VfsResource,
    user_settings: &UserSettings,
    textures: Option<&TextureCatalog>,
    terms: Option<&TermCatalog>,
    extra_values: BTreeMap<String, StoredValue>,
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
    values.insert("dialogue".to_string(), default_dialogue_model());
    values.insert("history".to_string(), default_history_model());
    values.extend(extra_values);
    let source = vfs.0.read_text(target).map_err(|error| error.to_string())?;
    let textures = textures.ok_or_else(|| "texture catalog is unavailable".to_string())?;
    let terms = terms.ok_or_else(|| "term catalog is unavailable".to_string())?;
    evaluate_ui_component_named(target, &source, UiContext::new(values), textures, terms)
        .map_err(|error| error.to_string())
}

fn default_dialogue_model() -> StoredValue {
    StoredValue::Map(BTreeMap::from([
        ("speaker".to_string(), StoredValue::String(String::new())),
        ("text".to_string(), StoredValue::String(String::new())),
        ("visible".to_string(), StoredValue::Bool(false)),
        ("revealedCharacters".to_string(), StoredValue::Int(0)),
        ("canAdvance".to_string(), StoredValue::Bool(false)),
    ]))
}

fn default_history_model() -> StoredValue {
    StoredValue::Map(BTreeMap::from([
        ("entries".to_string(), StoredValue::Array(Vec::new())),
        ("text".to_string(), StoredValue::String(String::new())),
        ("visible".to_string(), StoredValue::Bool(false)),
    ]))
}

/// UI components under the conventional `ui/` directory are package-rooted.
/// Other relative paths remain relative to the declaring script. Persisted
/// canonical paths pass through unchanged, which also repairs older saves that
/// retained `ui/...` and were restored from a script subdirectory.
fn resolve_ui_component_path(
    vfs: &VfsResource,
    current_script: Option<&str>,
    component: &str,
) -> String {
    if component.starts_with("ui/") {
        if let Some((archive, _)) = current_script
            .and_then(|path| path.strip_prefix("hdp://"))
            .and_then(|path| path.split_once('/'))
        {
            return vfs
                .0
                .resolve_path(None, &format!("hdp://{archive}/{component}"));
        }
        return vfs.0.resolve_path(None, component);
    }
    vfs.0.resolve_path(current_script, component)
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
            ScriptResponse::Continue => hiraku_script::Value::Unit,
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
                    pending_script_commands.enqueue(ScriptCommand::PlayBgm {
                        path: definition.path.clone(),
                        prelude: definition.prelude.clone(),
                        volume,
                        fade_in: fade_in_ms.map(std::time::Duration::from_millis),
                        animation_id: None,
                    });
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
                    pending_script_commands.enqueue(ScriptCommand::PlayVoice {
                        path: definition.path.clone(),
                        volume,
                        mode: VoicePlaybackMode::Exclusive,
                        animation_id: None,
                    });
                }
                None => warn!("voice `{path}` is not defined"),
            },
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::SetUiRole {
                role,
                component,
            }) => {
                let target =
                    resolve_ui_component_path(&vfs, runtime.current_script.as_deref(), &component);
                runtime.ui_registry.insert(role.clone(), target.clone());
                if role == "dialogue" {
                    match evaluate_ui_at(
                        &target,
                        &runtime,
                        &vfs,
                        &user_settings,
                        textures.as_deref(),
                        terms.as_deref(),
                    ) {
                        Ok(screen) => {
                            runtime
                                .mounted_ui_overlays
                                .insert("__role.dialogue".into(), target);
                            pending_script_commands.enqueue(ScriptCommand::ShowOverlay {
                                name: "__role.dialogue".into(),
                                screen,
                            });
                        }
                        Err(error) => {
                            warn!("failed to enable dialogue UI `{component}`: {error}");
                            runtime.story = None;
                        }
                    }
                }
            }
            StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::MountUiOverlay { name, component },
            ) => {
                let target = runtime
                    .ui_registry
                    .get(&component)
                    .cloned()
                    .unwrap_or_else(|| {
                        resolve_ui_component_path(
                            &vfs,
                            runtime.current_script.as_deref(),
                            &component,
                        )
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
                        runtime
                            .mounted_ui_overlays
                            .insert(name.clone(), target.clone());
                        pending_script_commands
                            .enqueue(ScriptCommand::ShowOverlay { name, screen });
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
                pending_script_commands.enqueue(ScriptCommand::HideOverlay { name });
            }
            StoryRuntimeEvent::Effect(
                effect @ (crate::script::capabilities::StoryEffect::Say { .. }
                | crate::script::capabilities::StoryEffect::ContinueDialogue { .. }),
            ) => {
                if !runtime.ui_registry.contains_key("dialogue") {
                    warn!(
                        "dialogue UI is not configured; call ui.set(\"dialogue\", \"path/to/dialogue.ui.hks\") before executing dialogue"
                    );
                    runtime.story = None;
                    runtime.story_events.clear();
                } else {
                    match script_command_from_effect(effect, textures.as_deref()) {
                        Ok(command) => {
                            pending_script_commands.enqueue(command);
                        }
                        Err(error) => warn!("HKS dialogue command rejected: {error}"),
                    }
                }
            }
            StoryRuntimeEvent::Effect(effect) => {
                match script_command_from_effect(effect, textures.as_deref()) {
                    Ok(command) => {
                        pending_script_commands.enqueue(command);
                    }
                    Err(error) => warn!("HKS native command rejected: {error}"),
                }
            }
            StoryRuntimeEvent::Wait(crate::script::capabilities::StoryWait::DialogueAdvance) => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                pending_script_commands
                    .enqueue(ScriptCommand::AwaitDialogueAdvance { done: request });
            }
            StoryRuntimeEvent::Choice { prompt, options } => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                let Some(target) = runtime.ui_registry.get("choice").cloned() else {
                    warn!(
                        "choice UI is not configured; call ui.set(\"choice\", \"path/to/choice.ui.hks\") before executing choice"
                    );
                    runtime.story = None;
                    runtime.story_events.clear();
                    return;
                };
                let choice_model = StoredValue::Map(BTreeMap::from([
                    ("prompt".into(), StoredValue::String(prompt)),
                    (
                        "options".into(),
                        StoredValue::Array(options.into_iter().map(StoredValue::String).collect()),
                    ),
                ]));
                match evaluate_ui_at_with(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                    BTreeMap::from([("choice".into(), choice_model)]),
                ) {
                    Ok(screen) => {
                        pending_script_commands.enqueue(ScriptCommand::ShowScreen {
                            screen,
                            done: Some(request),
                        });
                    }
                    Err(error) => {
                        warn!("failed to render choice UI `{target}`: {error}");
                        runtime.story = None;
                        runtime.story_events.clear();
                    }
                }
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
                        pending_script_commands.enqueue(ScriptCommand::ShowScreen {
                            screen,
                            done: Some(request),
                        });
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
                    pending_script_commands.enqueue(ScriptCommand::PlayVoice {
                        path: definition.path.clone(),
                        volume,
                        mode: VoicePlaybackMode::Concurrent,
                        animation_id: Some(animation_id.clone()),
                    });
                    pending_script_commands.enqueue(ScriptCommand::WaitAnimations {
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
            for ((id, entity), newly_spawned) in item
                .entity_ids
                .drain(..)
                .zip(item.entities.drain(..))
                .zip(item.newly_spawned.drain(..))
            {
                if newly_spawned {
                    if stage.sprites.get(&id) == Some(&entity) {
                        stage.sprites.remove(&id);
                    }
                    commands.entity(entity).try_despawn();
                } else {
                    commands.entity(entity).try_insert(Visibility::Hidden);
                }
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

    if let Err(error) = start_hks_runtime(vfs, script_runtime, bootstrap, user_settings) {
        frontend.notice = Some(format!("Failed to start HKS runtime: {error}"));
        frontend.runtime_started = false;
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

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(mut ctx: RuntimeMenuContext) {
    for click in ctx.clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let Some(button_entity) =
            find_component_ancestor(click.entity, &ctx.action_query, &ctx.parents)
        else {
            continue;
        };
        let Ok((button, screen_button, image_button)) = ctx.action_query.get(button_entity) else {
            continue;
        };
        if let Some(root) = button.screen_root
            && Some(root) != ctx.screen_state.active_root
            && !ctx
                .overlay_state
                .roots
                .values()
                .any(|overlay| *overlay == root)
        {
            continue;
        }
        if screen_button.is_some_and(|button| !button.enabled) {
            continue;
        }
        *ctx.runtime_menu
            .consumed_pointer_clicks
            .entry(click.pointer_id)
            .or_default() += 1;
        // The action may replace or cover this node before picking emits a
        // later interaction transition. Restore its release visual now.
        if let Some(image_button) = image_button {
            let mut image = ImageNode::new(image_button.normal_texture.clone());
            image.texture_atlas = image_button.normal_atlas.clone();
            image.rect = image_button.normal_rect;
            ctx.commands.entity(button_entity).insert((
                BackgroundColor(Color::NONE),
                UiTransform::IDENTITY,
                image,
                image_button.normal_node.clone(),
            ));
        } else if let Some(screen_button) = screen_button {
            ctx.commands.entity(button_entity).insert((
                BackgroundColor(screen_button.normal_background),
                UiTransform::IDENTITY,
            ));
            ctx.commands
                .entity(screen_button.text_entity)
                .insert(TextColor(screen_button.normal_text_color));
            if let Some(texture) = screen_button.normal_texture.as_ref() {
                let mut image = ImageNode::new(texture.clone());
                image.texture_atlas = screen_button.normal_atlas.clone();
                image.rect = screen_button.normal_rect;
                ctx.commands.entity(button_entity).insert(image);
            }
        } else {
            ctx.commands
                .entity(button_entity)
                .insert(BackgroundColor(ctx.ui_style.choice_button_bg));
        }
        let action = button.action.clone();
        match &action {
            RuntimeMenuButtonAction::Save(slot) => {
                if let Err(error) = save_runtime_slot(slot, &ctx.script_runtime, &ctx.shared_state)
                {
                    warn!("failed to save slot `{slot}`: {error}");
                }
            }
            RuntimeMenuButtonAction::Load(slot) => {
                let save_data = match load_save_data(slot) {
                    Ok(save_data) => save_data,
                    Err(error) => {
                        warn!("failed to load slot `{slot}`: {error}");
                        ctx.frontend.notice = Some(format!("Failed to load slot {slot}: {error}"));
                        continue;
                    }
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
                ctx.dialogue_history.entries.clear();
                ctx.dialogue_history.visible = false;
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
                if let Some(error) = ctx.frontend.notice.as_deref() {
                    warn!("failed to restore slot `{slot}`: {error}");
                } else {
                    info!("loaded save slot `{slot}`");
                }
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
            RuntimeMenuButtonAction::OpenUi(role) => {
                let Some(target) = ctx.script_runtime.ui_registry.get(role).cloned() else {
                    warn!("UI action route references unregistered role `{role}`");
                    continue;
                };
                match evaluate_ui_at(
                    &target,
                    &ctx.script_runtime,
                    &ctx.vfs,
                    &ctx.user_settings,
                    Some(&ctx.textures),
                    Some(&ctx.terms),
                ) {
                    Ok(screen) => {
                        ctx.pending_script_commands
                            .enqueue(ScriptCommand::ShowScreen { screen, done: None });
                    }
                    Err(error) => warn!("failed to open UI role `{role}`: {error}"),
                }
            }
            RuntimeMenuButtonAction::CloseUi => {
                clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
            }
            RuntimeMenuButtonAction::SetHistoryVisible(visible) => {
                ctx.dialogue_history.visible = *visible;
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
                ctx.dialogue_history.entries.clear();
                ctx.dialogue_history.visible = false;
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
            RuntimeMenuButtonAction::AdvanceDialogue => {
                advance_dialogue(
                    &mut ctx.dialogue_state,
                    &mut ctx.animations,
                    &mut ctx.dialogue_chars,
                    &mut ctx.responses,
                );
            }
        }
    }
}

fn spawn_pause_menu(commands: &mut Commands, ui_fonts: &UiFonts, ui_style: &UiStyle) -> Entity {
    let root = commands
        .spawn((
            PauseMenuRoot,
            GlobalZIndex(SCREEN_MODAL_ACTIVE_Z + 10),
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
                    width: percent(72.0),
                    max_width: px(640.0),
                    padding: UiRect::all(px(32.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18.0),
                    border: UiRect::all(px(2.0)),
                    border_radius: BorderRadius::all(px(24.0)),
                    ..default()
                },
                BackgroundColor(ui_style.choice_panel_bg),
                BorderColor::all(ui_style.choice_button_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Game Menu"),
                    ui_text_font(ui_fonts, 42.0),
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
                    RuntimeMenuButtonAction::Save("quick".into()),
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Quick Load",
                    RuntimeMenuButtonAction::Load("quick".into()),
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
            RuntimeMenuButton {
                action,
                screen_root: None,
            },
            Button,
            Node {
                width: percent(100.0),
                min_height: px(68.0),
                border: UiRect::all(px(2.0)),
                padding: UiRect::axes(px(24.0), px(14.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(px(14.0)),
                ..default()
            },
            BackgroundColor(ui_style.choice_button_bg),
            BorderColor::all(ui_style.choice_button_border),
        ))
        .with_children(|button| {
            button.spawn((
                Pickable::IGNORE,
                Text::new(text),
                ui_text_font(ui_fonts, ui_style.quick_button_size.max(30.0)),
                TextColor(ui_style.choice_text_color),
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
    pending_script_commands.clear();
    active_batches.items.clear();
    pending_characters.items.clear();
    animations.waits.clear();
    finish_all_voices(commands, animations, voice_state);
}
