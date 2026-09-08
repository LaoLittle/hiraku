use std::{collections::BTreeMap, time::Duration};

use bevy::math::{Vec2, Vec3};

use crate::{
    state::{SaveGameData, SceneSharedState, StoredValue},
    storage::{StorageError, save_root_path, write_save_data_to_root},
    texture::TextureCatalog,
    vfs::VfsResource,
};

pub(crate) mod actor_motion;
mod animation;
pub(crate) mod capabilities;
mod command;
mod execution_runtime;
pub(crate) mod navigation;
mod runtime;
mod story_runtime;
pub mod ui_runtime;
mod ui_vm;
pub(crate) use ui_vm::validate_ui_source;

pub use animation::{AnimationPhase, AnimationSpec};
pub(crate) use command::{
    AnimationCommand, AudioCommand, CameraCommand, CharacterCommand, CharacterEase,
    DialogueCommand, ResolvedCharacterKeyframe, RuntimeCommand, ScriptCommand, SettingsCommand,
    StageCommand, UiCommand, VideoCommand, VoicePlaybackMode,
};
pub(crate) use execution_runtime::ExecutionId;
pub(crate) use runtime::{
    CameraEffectScope, CameraProjectionMode, ScriptCallFrame, ScriptRuntimeState,
};
pub(crate) use story_runtime::{StoryRuntime, StoryRuntimeEvent, StoryRuntimeSnapshot};

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

/// The terminal boundary for script/UI/HSON diagnostics. Never pass rendered
/// reports through tracing: its formatter may escape their ANSI sequences.
/// Callers keep the error as data until this boundary, avoiding duplicate output.
pub(crate) fn emit_script_diagnostic(context: &str, diagnostic: &str) {
    if let Err(error) = hiraku_script::emit_rendered_diagnostic(context, diagnostic) {
        bevy::log::error!("failed to write script diagnostic to stderr: {error}");
    }
}

pub use ui_runtime::{UiContext, UiIntent};
pub(crate) use ui_vm::evaluate_ui_callback_with_args;
pub(crate) use ui_vm::evaluate_ui_component_named_with_args;
pub(crate) use ui_vm::evaluate_ui_reactive_binding;

pub(crate) fn script_command_from_effect(
    effect: capabilities::StoryEffect,
    textures: Option<&TextureCatalog>,
) -> Result<ScriptCommand, String> {
    use capabilities::StoryEffect;

    Ok(match effect {
        StoryEffect::Log(message) => ScriptCommand::Runtime(RuntimeCommand::Log(message)),
        StoryEffect::ClearDialogue => ScriptCommand::Dialogue(DialogueCommand::Clear),
        StoryEffect::DialogueSpeed(multiplier) => {
            ScriptCommand::Dialogue(DialogueCommand::Speed(multiplier))
        }
        StoryEffect::StopBgm => ScriptCommand::Audio(AudioCommand::StopBgm),
        StoryEffect::Exit => ScriptCommand::Runtime(RuntimeCommand::Exit),
        StoryEffect::Navigate(navigation) => {
            ScriptCommand::Runtime(RuntimeCommand::Navigate(navigation))
        }
        StoryEffect::AdjustSetting { name, delta } => {
            ScriptCommand::Settings(SettingsCommand::Adjust { name, delta })
        }
        StoryEffect::Say { speaker, text } => ScriptCommand::Dialogue(DialogueCommand::Say {
            speaker,
            text,
            animation_id: None,
        }),
        StoryEffect::ContinueDialogue { text } => {
            ScriptCommand::Dialogue(DialogueCommand::Continue {
                text,
                animation_id: None,
            })
        }
        StoryEffect::SetCamera {
            blur,
            zoom,
            zoom_view_space,
            offset,
            rotation,
            projection,
            scope,
            duration_ms,
            ease,
        } => ScriptCommand::Camera(CameraCommand::Set {
            blur_intensity: blur,
            zoom,
            zoom_view_space,
            offset: offset.map(Vec3::from_array),
            rotation: rotation.map(Vec3::from_array),
            projection,
            scope,
            duration: Duration::from_millis(duration_ms),
            ease: parse_camera_ease(&ease)?,
            animation_id: None,
        }),
        StoryEffect::Delay { .. } => {
            return Err("delay must be dispatched through the story wait boundary".into());
        }
        StoryEffect::SetCurtain {
            opacity,
            fade_ms,
            mask,
            softness,
        } => ScriptCommand::Stage(StageCommand::SetCurtain {
            opacity,
            fade: fade_ms.map(Duration::from_millis),
            softness,
            mask: mask
                .map(|name| {
                    let definition = textures
                        .and_then(|catalog| catalog.resolve(&name))
                        .ok_or_else(|| format!("dissolve texture `{name}` is not defined"))?;
                    if definition.rect.is_some() {
                        return Err("dissolve masks must use a standalone texture".to_string());
                    }
                    Ok(definition.path.clone())
                })
                .transpose()?,
        }),
        StoryEffect::SetBackground {
            texture,
            fade_in_ms,
        } => {
            let definition = textures
                .and_then(|catalog| catalog.resolve(&texture))
                .ok_or_else(|| format!("texture `{texture}` is not defined"))?;
            ScriptCommand::Stage(StageCommand::SetBackground {
                path: definition.path.clone(),
                fade: fade_in_ms.map(Duration::from_millis),
                animation_id: None,
            })
        }
        StoryEffect::Picture(mut picture) => {
            if let crate::scene::pictures::PictureCommand::Show { path, rect, .. } = &mut picture {
                let texture = textures
                    .and_then(|catalog| catalog.resolve(path))
                    .ok_or_else(|| format!("texture `{path}` is not defined"))?;
                *path = texture.path.clone();
                *rect = texture.rect;
            }
            ScriptCommand::Stage(StageCommand::Picture(picture))
        }
        StoryEffect::ActorMotion {
            actor_id,
            revision,
            transition,
        } => ScriptCommand::Character(CharacterCommand::Motion {
            actor_id,
            revision,
            transition,
            animation_id: None,
        }),
        StoryEffect::SaveSlot(slot) => ScriptCommand::Runtime(RuntimeCommand::SaveSlot(slot)),
        StoryEffect::HideCharacter { actor_id, fade_ms } => {
            ScriptCommand::Character(CharacterCommand::Hide { actor_id, fade_ms })
        }
        StoryEffect::ShowCharacter {
            placement_animation,
            actor_id,
            character_name,
            expressions,
            position,
            scale,
            focused,
        } => ScriptCommand::Character(CharacterCommand::Show {
            placement_animation,
            actor_id,
            character_name,
            expressions,
            position: Vec2::new(position[0], position[1]),
            scale,
            focused,
            fade: placement_animation
                .map(|animation| std::time::Duration::from_secs_f32(animation.duration())),
            animation_id: None,
        }),
        StoryEffect::SetUiRole { .. }
        | StoryEffect::MountUiOverlay { .. }
        | StoryEffect::UnmountUiOverlay { .. }
        | StoryEffect::PlayBgm { .. }
        | StoryEffect::PlaySfx { .. }
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

#[derive(Debug, Clone)]
pub enum ScriptResponse {
    Continue,
    Choice(StoredValue),
    UiResult(hiraku_script::Value),
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
    pub pending_ui_arguments: Vec<StoredValue>,
    pub ui_registry: BTreeMap<String, String>,
    pub mounted_ui_overlays: BTreeMap<String, String>,
}

impl ScriptBootstrap {
    pub fn from_save(data: &SaveGameData) -> Self {
        let mut values = data.globals.clone();
        values.extend(data.scope.clone());
        Self {
            startup_script: data.resume_script.clone(),
            values,
            snapshot: data.vm_snapshot.clone(),
            call_stack: data.script_call_stack.clone(),
            pending_ui_screen: data.pending_ui_screen.clone(),
            pending_ui_arguments: data.pending_ui_arguments.clone(),
            ui_registry: data.ui_registry.clone(),
            mounted_ui_overlays: data.mounted_ui_overlays.clone(),
        }
    }
}

pub fn start_story_runtime(
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
        pending_ui_arguments,
        ui_registry,
        mounted_ui_overlays,
    } = bootstrap;
    let source = vfs
        .0
        .read_text(&startup_script)
        .map_err(|error| error.to_string())?;
    let bytecode = compile_story_bytecode(&startup_script, &source)?;
    let mut story = if let Some(snapshot) = snapshot {
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
    for (name, component) in &mounted_ui_overlays {
        story.enqueue_event(StoryRuntimeEvent::Effect(
            capabilities::StoryEffect::MountUiOverlay {
                name: name.clone(),
                component: component.clone(),
            },
        ));
    }
    if let Some(boundary) = restored_boundary {
        story.enqueue_event(if let Some(path) = pending_ui_screen.clone() {
            StoryRuntimeEvent::OpenUi {
                path,
                arguments: pending_ui_arguments
                    .iter()
                    .cloned()
                    .map(stored_value_to_hks)
                    .collect(),
            }
        } else {
            boundary
        });
    }
    runtime.story = Some(story);
    runtime.current_script = Some(startup_script);
    runtime.call_stack = call_stack;
    runtime.wait_request = None;
    runtime.pending_ui_screen = pending_ui_screen;
    runtime.pending_ui_arguments = pending_ui_arguments;
    runtime.ui_registry = ui_registry;
    runtime.mounted_ui_overlays = mounted_ui_overlays.clone();
    runtime.response_inbox.clear();
    runtime.task_requests.clear();
    Ok(())
}

pub fn save_runtime_slot(
    slot: &str,
    runtime: &ScriptRuntimeState,
    shared_state: &SceneSharedState,
    thumbnail: &[u8],
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
        thumbnail_png: thumbnail.to_vec(),
        version: crate::state::CURRENT_SAVE_VERSION,
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
        pending_ui_arguments: runtime.pending_ui_arguments.clone(),
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
