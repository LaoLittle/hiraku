use std::{collections::BTreeMap, time::Duration};

use bevy::math::{Vec2, Vec3};
use serde::Deserialize;

use crate::{
    effect::custom::CustomEffectOptions,
    state::{
        ChoiceOption, SaveGameData, SceneSharedState, SceneSnapshot, StoredValue, UiStylePatch,
    },
    storage::{StorageError, save_root_path, write_save_data_to_root},
    texture::TextureCatalog,
    ui::ScreenSpec,
    vfs::VfsResource,
};

mod animation;
pub(crate) mod capabilities;
mod hks_runtime;
mod runtime;
mod task_runtime;
pub mod ui_runtime;
mod ui_vm;

pub use animation::{AnimationPhase, AnimationSpec};
pub(crate) use hks_runtime::{StoryRuntime, StoryRuntimeEvent, StoryRuntimeSnapshot};
pub(crate) use runtime::{
    CameraEffectScope, CameraProjectionMode, ScriptCallFrame, ScriptRuntimeState,
    tick_script_runtime,
};

pub(crate) fn compile_story_bytecode(
    path: &str,
    source: &str,
) -> Result<hiraku_script::Bytecode, String> {
    if !path.ends_with(".hks") {
        return Err(format!(
            "executable scripts must use the `.hks` extension: `{path}`"
        ));
    }
    capabilities::compile_story_bytecode_with_options(
        path,
        source,
        hiraku_script::RenderOptions::terminal(),
    )
}

pub(crate) fn emit_script_diagnostic(context: &str, diagnostic: &str) {
    if let Err(error) = hiraku_script::emit_rendered_diagnostic(context, diagnostic) {
        bevy::log::error!("failed to write script diagnostic to stderr: {error}");
    }
}

pub use ui_runtime::{UiContext, UiIntent};
pub use ui_vm::evaluate_ui_component_named;
pub(crate) use ui_vm::evaluate_ui_reactive_binding;

#[derive(Debug)]
pub enum ScriptCommand {
    Log(String),
    SetBackground {
        path: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
    ShowSprite {
        id: String,
        path: String,
        rect: Option<[f32; 4]>,
        position: Vec2,
        layer: f32,
        scale: f32,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
    HideSprite {
        id: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
    SetOverlay {
        alpha: f32,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
    Say {
        speaker: String,
        text: String,
        animation_id: Option<String>,
    },
    ContinueDialogue {
        text: String,
        animation_id: Option<String>,
    },
    AwaitDialogueAdvance {
        done: ScriptRequestId,
    },
    SetDialogue {
        speaker: String,
        text: String,
        reveal_from: Option<usize>,
        animation_id: Option<String>,
    },
    ClearDialogue,
    SetTextEffect(DialogueTextEffectSpec),
    ResetTextEffect,
    SetCamera {
        blur_intensity: Option<f32>,
        zoom: Option<f32>,
        offset: Option<Vec3>,
        rotation: Option<Vec3>,
        projection: Option<CameraProjectionMode>,
        scope: CameraEffectScope,
        duration: Duration,
        ease: CharacterEase,
        animation_id: Option<String>,
    },
    ApplyUserSettings(crate::storage::UserSettings),
    AdjustUserSetting {
        name: String,
        delta: f32,
    },
    ApplyUiStyle(UiStylePatch),
    ResetUiStyle,
    ShowScreen {
        screen: ScreenSpec,
        done: Option<ScriptRequestId>,
    },
    WaitForScreenChoice {
        done: ScriptRequestId,
    },
    ShowOverlay {
        name: String,
        screen: ScreenSpec,
    },
    HideOverlay {
        name: String,
    },
    Choose {
        prompt: String,
        options: Vec<ChoiceOption>,
        done: ScriptRequestId,
    },
    ShowCharacter {
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: Vec2,
        scale: f32,
        focused: bool,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
    HideCharacter {
        actor_id: String,
    },
    JumpCharacter {
        actor_id: String,
        height: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    ShakeCharacter {
        actor_id: String,
        amplitude: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    AnimateCharacter {
        actor_id: String,
        keyframes: Vec<ResolvedCharacterKeyframe>,
        animation_id: Option<String>,
    },
    RestoreSnapshot {
        snapshot: SceneSnapshot,
    },
    PlayCustomEffect {
        options: CustomEffectOptions,
        animation_id: Option<String>,
    },
    RuleTransitionBg {
        path: String,
        rule_path: String,
        duration: Duration,
        vague: f32,
        animation_id: Option<String>,
    },
    MoveSprite {
        id: String,
        position: Vec2,
        duration: Duration,
        animation_id: Option<String>,
    },
    ScaleSprite {
        id: String,
        scale: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    FadeSprite {
        id: String,
        alpha: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    Wait {
        duration: Duration,
        animation_id: Option<String>,
        done: ScriptRequestId,
    },
    WaitAnimations {
        ids: Vec<String>,
        done: ScriptRequestId,
    },
    Shake {
        duration: Duration,
        amplitude: f32,
        animation_id: Option<String>,
    },
    PlayBgm {
        path: String,
        prelude: Option<String>,
        volume: f32,
        fade_in: Option<Duration>,
        animation_id: Option<String>,
    },
    SetBgmVolume {
        volume: f32,
    },
    FadeBgm {
        volume: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    StopBgm,
    PlayVoice {
        path: String,
        volume: f32,
        mode: VoicePlaybackMode,
        animation_id: Option<String>,
    },
    StopVoice,
    PlaySfx {
        path: String,
        volume: f32,
    },
    SubmitBatch {
        mode: BatchSubmitMode,
        items: Vec<BatchSubmissionItem>,
    },
    CancelAnimations {
        ids: Vec<String>,
    },
    ReturnToTitle,
    Exit,
}

pub(crate) fn script_command_from_effect(
    effect: capabilities::StoryEffect,
    textures: Option<&TextureCatalog>,
) -> Result<ScriptCommand, String> {
    use capabilities::StoryEffect;

    Ok(match effect {
        StoryEffect::Log(message) => ScriptCommand::Log(message),
        StoryEffect::ClearDialogue => ScriptCommand::ClearDialogue,
        StoryEffect::StopBgm => ScriptCommand::StopBgm,
        StoryEffect::Exit => ScriptCommand::Exit,
        StoryEffect::ReturnToTitle => ScriptCommand::ReturnToTitle,
        StoryEffect::AdjustSetting { name, delta } => {
            ScriptCommand::AdjustUserSetting { name, delta }
        }
        StoryEffect::Say { speaker, text } => ScriptCommand::Say {
            speaker,
            text,
            animation_id: None,
        },
        StoryEffect::ContinueDialogue { text } => ScriptCommand::ContinueDialogue {
            text,
            animation_id: None,
        },
        StoryEffect::SetCamera {
            blur,
            zoom,
            offset,
            rotation,
            projection,
            scope,
            duration_ms,
            ease,
        } => ScriptCommand::SetCamera {
            blur_intensity: blur,
            zoom,
            offset: offset.map(Vec3::from_array),
            rotation: rotation.map(Vec3::from_array),
            projection,
            scope,
            duration: Duration::from_millis(duration_ms),
            ease: parse_camera_ease(&ease)?,
            animation_id: None,
        },
        StoryEffect::SetBackground { texture } => {
            let definition = textures
                .and_then(|catalog| catalog.resolve(&texture))
                .ok_or_else(|| format!("texture `{texture}` is not defined"))?;
            ScriptCommand::SetBackground {
                path: definition.path.clone(),
                fade: None,
                animation_id: None,
            }
        }
        StoryEffect::ShowCharacter {
            actor_id,
            character_name,
            expressions,
            position,
            scale,
            focused,
        } => ScriptCommand::ShowCharacter {
            actor_id,
            character_name,
            expressions,
            position: Vec2::new(position[0], position[1]),
            scale,
            focused,
            fade: None,
            animation_id: None,
        },
        StoryEffect::GotoScript { .. }
        | StoryEffect::CallScript { .. }
        | StoryEffect::SetUiRole { .. }
        | StoryEffect::MountUiOverlay { .. }
        | StoryEffect::UnmountUiOverlay { .. }
        | StoryEffect::PlayBgm { .. }
        | StoryEffect::PlayVoice { .. } => {
            return Err("effect requires script runtime asset resolution".to_string());
        }
    })
}

fn parse_camera_ease(name: &str) -> Result<CharacterEase, String> {
    match name {
        "" | "linear" => Ok(CharacterEase::Linear),
        "ease" => Ok(CharacterEase::Ease),
        "easeIn" | "ease_in" => Ok(CharacterEase::EaseIn),
        "easeOut" | "ease_out" => Ok(CharacterEase::EaseOut),
        "easeInOut" | "ease_in_out" => Ok(CharacterEase::EaseInOut),
        "bounce" => Ok(CharacterEase::Bounce),
        _ => Err(format!("unsupported camera easing `{name}`")),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BatchSubmitMode {
    Sequence,
    Parallel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePlaybackMode {
    Exclusive,
    Concurrent,
}

#[derive(Debug)]
pub struct BatchSubmissionItem {
    pub handle: String,
    pub command: Box<ScriptCommand>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DialogueTextEffectSpec {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub cps: Option<f32>,
    #[serde(default)]
    pub fade_seconds: Option<f32>,
    #[serde(default)]
    pub fade_ms: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ResolvedCharacterKeyframe {
    pub time: f32,
    pub position: Vec2,
    pub ease: CharacterEase,
}

#[derive(Debug, Clone, Copy)]
pub enum CharacterEase {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
}

#[derive(Debug, Clone)]
pub enum ScriptResponse {
    Continue,
    Choice(StoredValue),
}

/// Stable identifier joining an ECS-owned script wait with its eventual response.
///
/// Request identifiers, unlike Bevy entities or message sequence numbers, can be
/// retained in a script snapshot and restored deterministically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptRequestId(pub u64);

/// Transient response emitted by UI/input systems. Durable waiting state lives
/// in the script runtime; this message only reports that an external fact occurred.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct ScriptResponseMessage {
    pub request: ScriptRequestId,
    pub response: ScriptResponse,
}

#[derive(Clone, Debug)]
pub struct ScriptBootstrap {
    pub startup_script: String,
    pub values: BTreeMap<String, StoredValue>,
    pub snapshot: Option<StoryRuntimeSnapshot>,
    pub call_stack: Vec<crate::state::ScriptCallFrameSnapshot>,
    pub pending_ui_screen: Option<String>,
    pub ui_registry: BTreeMap<String, String>,
    pub mounted_ui_overlays: BTreeMap<String, String>,
}

impl ScriptBootstrap {
    pub fn new(startup_script: String) -> Self {
        Self {
            startup_script,
            values: BTreeMap::new(),
            snapshot: None,
            call_stack: Vec::new(),
            pending_ui_screen: None,
            ui_registry: BTreeMap::new(),
            mounted_ui_overlays: BTreeMap::new(),
        }
    }

    pub fn from_save(data: &SaveGameData) -> Self {
        let mut values = data.globals.clone();
        values.extend(data.scope.clone());
        Self {
            startup_script: data.resume_script.clone(),
            values,
            snapshot: data.vm_snapshot.clone(),
            call_stack: data.script_call_stack.clone(),
            pending_ui_screen: data.pending_ui_screen.clone(),
            ui_registry: data.ui_registry.clone(),
            mounted_ui_overlays: data.mounted_ui_overlays.clone(),
        }
    }
}

pub fn start_hks_runtime(
    vfs: &VfsResource,
    runtime: &mut ScriptRuntimeState,
    bootstrap: ScriptBootstrap,
    user_settings: &crate::storage::UserSettings,
) -> Result<(), String> {
    let ScriptBootstrap {
        startup_script,
        values,
        snapshot,
        call_stack,
        pending_ui_screen,
        ui_registry,
        mounted_ui_overlays,
    } = bootstrap;
    let source = vfs
        .0
        .read_text(&startup_script)
        .map_err(|error| error.to_string())?;
    let bytecode = compile_story_bytecode(&startup_script, &source)?;
    let story = if let Some(snapshot) = snapshot {
        StoryRuntime::restore(bytecode, snapshot).map_err(|error| error.to_string())?
    } else {
        let mut story = StoryRuntime::new(bytecode).map_err(|error| error.to_string())?;
        let mut globals = capabilities::engine_globals(user_settings);
        for (name, value) in values {
            globals.insert(name, stored_value_to_hks(value));
        }
        story.set_globals(globals);
        story
    };
    let call_stack = call_stack
        .into_iter()
        .map(|frame| {
            let source = vfs
                .0
                .read_text(&frame.script)
                .map_err(|error| error.to_string())?;
            let bytecode = compile_story_bytecode(&frame.script, &source)?;
            let story = StoryRuntime::restore(bytecode, frame.snapshot)
                .map_err(|error| error.to_string())?;
            Ok(ScriptCallFrame {
                script: frame.script,
                story,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let restored_boundary = story.restored_boundary_event();
    runtime.story = Some(story);
    runtime.current_script = Some(startup_script);
    runtime.call_stack = call_stack;
    runtime.story_events.clear();
    runtime.wait_request = None;
    runtime.pending_ui_screen = pending_ui_screen;
    runtime.ui_registry = ui_registry;
    runtime.mounted_ui_overlays = mounted_ui_overlays.clone();
    runtime.response_inbox.clear();
    runtime.task_requests.clear();
    for (name, component) in mounted_ui_overlays {
        runtime.story_events.push_back(StoryRuntimeEvent::Effect(
            capabilities::StoryEffect::MountUiOverlay { name, component },
        ));
    }
    if let Some(boundary) = restored_boundary {
        if let Some(path) = runtime.pending_ui_screen.clone() {
            runtime
                .story_events
                .push_back(StoryRuntimeEvent::OpenUi { path });
        } else {
            runtime.story_events.push_back(boundary);
        }
    }
    Ok(())
}

pub fn save_runtime_slot(
    slot: &str,
    runtime: &ScriptRuntimeState,
    shared_state: &SceneSharedState,
) -> Result<(), StorageError> {
    let current_script = runtime.current_script.clone().unwrap_or_default();
    let values = runtime
        .story
        .as_ref()
        .map(|story| hks_globals_to_stored(story.globals()))
        .unwrap_or_default();
    let snapshot = runtime
        .story
        .as_ref()
        .map(StoryRuntime::snapshot)
        .transpose()
        .map_err(|error| StorageError::InvalidSave(error.to_string()))?;
    let script_call_stack = runtime
        .call_stack
        .iter()
        .map(|frame| {
            frame
                .story
                .snapshot()
                .map(|snapshot| crate::state::ScriptCallFrameSnapshot {
                    script: frame.script.clone(),
                    snapshot,
                })
                .map_err(|error| StorageError::InvalidSave(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let data = SaveGameData {
        version: 8,
        resume_script: current_script,
        script_stack: script_call_stack
            .iter()
            .map(|frame| frame.script.clone())
            .collect(),
        globals: values,
        scene: shared_state.0.clone(),
        vm_snapshot: snapshot,
        script_call_stack,
        pending_ui_screen: runtime.pending_ui_screen.clone(),
        ui_registry: runtime.ui_registry.clone(),
        mounted_ui_overlays: runtime.mounted_ui_overlays.clone(),
        ..Default::default()
    };
    write_save_data_to_root(&save_root_path(), slot, &data)
}

fn stored_value_to_hks(value: StoredValue) -> hiraku_script::Value {
    match value {
        StoredValue::Bool(value) => hiraku_script::Value::Bool(value),
        StoredValue::Int(value) => hiraku_script::Value::Number(value as f64),
        StoredValue::Float(value) => hiraku_script::Value::Number(value),
        StoredValue::String(value) => hiraku_script::Value::String(value),
        StoredValue::Array(values) => {
            hiraku_script::Value::List(values.into_iter().map(stored_value_to_hks).collect())
        }
        StoredValue::Map(values) => hiraku_script::Value::Map(
            values
                .into_iter()
                .map(|(name, value)| (name, stored_value_to_hks(value)))
                .collect(),
        ),
    }
}

fn hks_globals_to_stored(
    globals: &BTreeMap<String, hiraku_script::Value>,
) -> BTreeMap<String, StoredValue> {
    globals
        .iter()
        .filter_map(|(name, value)| hks_value_to_stored(value).map(|value| (name.clone(), value)))
        .collect()
}

fn hks_value_to_stored(value: &hiraku_script::Value) -> Option<StoredValue> {
    match value {
        hiraku_script::Value::Bool(value) => Some(StoredValue::Bool(*value)),
        hiraku_script::Value::Number(value) => Some(StoredValue::Float(*value)),
        hiraku_script::Value::String(value) | hiraku_script::Value::Symbol(value) => {
            Some(StoredValue::String(value.clone()))
        }
        hiraku_script::Value::List(values) | hiraku_script::Value::Tuple(values) => Some(
            StoredValue::Array(values.iter().filter_map(hks_value_to_stored).collect()),
        ),
        hiraku_script::Value::Map(values) => Some(StoredValue::Map(
            values
                .iter()
                .filter_map(|(name, value)| {
                    hks_value_to_stored(value).map(|value| (name.clone(), value))
                })
                .collect(),
        )),
        hiraku_script::Value::Typed { value, .. } => hks_value_to_stored(value),
        _ => None,
    }
}
