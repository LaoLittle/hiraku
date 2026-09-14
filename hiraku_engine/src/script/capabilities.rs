//! Engine-owned script capabilities registered into the generic Hiraku VM.

use std::collections::BTreeMap;

use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{
    BuiltinCall, BuiltinId, BuiltinManifest, Bytecode, ScriptType, TextTemplate, Value,
};
use hiraku_script::{RenderOptions, SourceMap, StatementValue, parse_program, render_diagnostics};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::script::animation::{AnimationSpec, Easing, register_animation_api};
use crate::script::navigation::{NavigationOptions, NavigationRequest, NavigationResetValue};
use crate::script::{CameraEffectScope, CameraProjectionMode};
use crate::storage::UserSettings;

mod actor_patch;
mod movie;
mod scene_visuals;
mod sound;
mod spatial_stage;

/// Engine-facing effects produced by HKS native functions.
///
/// Engine code dispatches these effects directly to ECS-facing systems.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoryEffect {
    MovieBackground {
        path: String,
        fade_out_ms: u64,
    },
    StopMovie,
    Spatial(crate::stage::runtime::StageCommand),
    ActorMotion {
        actor_id: String,
        revision: u64,
        transition: super::actor_motion::ActorOffset,
    },
    Picture(crate::scene::pictures::PictureCommand),
    Clip(crate::scene::clipping::ClipCommand),
    SetActorDepth {
        id: String,
        depth: f32,
    },
    Log(String),
    ClearDialogue,
    DialogueSpeed(f32),
    SaveSlot(String),
    HideCharacter {
        actor_id: Option<String>,
        fade_ms: u64,
    },
    StopBgm {
        fade_ms: u64,
    },
    StopSfxChannel {
        channel: String,
        fade_ms: u64,
    },
    PlaySfxChannel {
        channel: String,
        path: String,
        volume: f32,
        fade_in_ms: Option<u64>,
        looped: bool,
    },
    Exit,
    SetBackground {
        texture: String,
        fade_in_ms: Option<u64>,
    },
    Delay {
        duration_ms: u64,
    },
    SetCurtain {
        color: [u8; 3],
        opacity: f32,
        fade_ms: Option<u64>,
        mask: Option<String>,
        softness: f32,
    },
    Navigate(NavigationRequest),
    SetUiRole {
        role: String,
        component: String,
    },
    MountUiOverlay {
        name: String,
        component: String,
        lifetime: Option<f32>,
    },
    UnmountUiOverlay {
        name: String,
    },
    AdjustSetting {
        name: String,
        delta: f32,
    },
    PlayBgm {
        path: String,
        volume: f32,
        fade_in_ms: Option<u64>,
    },
    Say {
        speaker: String,
        text: String,
    },
    ContinueDialogue {
        text: String,
    },
    PlayVoice {
        path: String,
        volume: f32,
    },
    PlaySfx {
        path: String,
        volume: f32,
        fade_in_ms: Option<u64>,
    },
    SetCamera {
        blur: Option<f32>,
        zoom: Option<f32>,
        zoom_view_space: bool,
        offset: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        projection: Option<CameraProjectionMode>,
        scope: CameraEffectScope,
        duration_ms: u64,
        ease: Easing,
    },
    ShowCharacter {
        rotation: f32,
        placement_animation: Option<AnimationSpec>,
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: [f32; 2],
        scale: f32,
        focused: bool,
    },
    StopActorMotion {
        actor_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoryWait {
    DialogueAdvance,
    Movie { path: String, fade_out_ms: u64 },
    Delay { duration_ms: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoryTaskKind {
    Sequence,
    Parallel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoryControl {
    RandomInt { min: i64, max: i64 },
    Navigate(NavigationRequest),
    SpawnTask { kind: StoryTaskKind, closure: Value },
    BeginChoice { prompt: String, closure: Value },
    AddChoiceOption { label: String, closure: Value },
    EnableChoiceOption { id: u64, enabled: bool },
    OpenUi { path: String, arguments: Vec<Value> },
    WaitTask { task: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoryCallOutcome {
    Return(Value),
    Control(StoryControl),
}

impl StoryCallOutcome {
    #[cfg(test)]
    pub fn into_return_value(self) -> Option<Value> {
        match self {
            Self::Return(value) => Some(value),
            Self::Control(_) => None,
        }
    }
}

const ACTOR_HANDLE_TYPE: u32 = 1;
const CAMERA_HANDLE_TYPE: u32 = 3;
/// Manifest used by the direct whole-story HKS runtime. Async capabilities are
/// registered here so the generic compiler can resolve them without engine AST lowering.
pub fn story_manifest() -> BuiltinManifest {
    story_registry().manifest()
}

pub fn compile_story_bytecode(path: &str, source: &str) -> Result<Bytecode, String> {
    compile_story_bytecode_with_options(path, source, RenderOptions::plain())
}

pub fn compile_story_bytecode_with_options(
    path: &str,
    source: &str,
    render_options: RenderOptions,
) -> Result<Bytecode, String> {
    let mut sources = SourceMap::new();
    let source_id = sources.insert(path, source);
    let program = parse_program(source).map_err(|errors| {
        let diagnostics = errors
            .into_iter()
            .map(|error| error.diagnostic(source_id.clone()))
            .collect::<Vec<_>>();
        render_diagnostics(&diagnostics, &sources, render_options)
    })?;
    if !program.warnings.is_empty() {
        let diagnostics = program
            .warnings
            .iter()
            .map(|warning| warning.diagnostic(source_id.clone()))
            .collect::<Vec<_>>();
        let rendered = render_diagnostics(&diagnostics, &sources, render_options);
        super::emit_script_diagnostic("HKS compiler warning:", &rendered);
    }
    let project = super::project::compile_library_project(
        vec![hiraku_script::ScriptSource {
            path: path.into(),
            source: source.into(),
            namespace: None,
        }],
        render_options,
    )?;
    Ok((*project.program.modules[project.paths[path].0 as usize].bytecode).clone())
}

pub fn engine_globals(settings: &UserSettings) -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "settings".to_string(),
        Value::Map(BTreeMap::from([
            (
                "bgmVolume".to_string(),
                Value::Number(f64::from(settings.bgm_volume)),
            ),
            (
                "voiceVolume".to_string(),
                Value::Number(f64::from(settings.voice_volume)),
            ),
            (
                "sfxVolume".to_string(),
                Value::Number(f64::from(settings.sfx_volume)),
            ),
        ])),
    )])
}

pub fn apply_engine_globals(
    globals: &BTreeMap<String, Value>,
    settings: &mut UserSettings,
) -> Result<(), String> {
    let Some(Value::Map(fields)) = globals.get("settings") else {
        return Err("engine global `settings` is missing or not a record".to_string());
    };
    settings.bgm_volume = setting_number(fields, "bgmVolume")?;
    settings.voice_volume = setting_number(fields, "voiceVolume")?;
    settings.sfx_volume = setting_number(fields, "sfxVolume")?;
    Ok(())
}

fn setting_number(fields: &BTreeMap<String, Value>, name: &str) -> Result<f32, String> {
    let Some(Value::Number(value)) = fields.get(name) else {
        return Err(format!("settings.{name} is missing or not a Number"));
    };
    if !value.is_finite() || !(0.0..=1.0).contains(value) {
        return Err(format!("settings.{name} must be between 0 and 1"));
    }
    Ok(*value as f32)
}

fn registry() -> NativeRegistry<CharacterContext> {
    let mut registry = NativeRegistry::new();
    scene_visuals::register(&mut registry);
    spatial_stage::register(&mut registry);
    sound::register(&mut registry);
    movie::register(&mut registry);
    Position::register_hks(&mut registry)
        .expect("Position API registration must be internally consistent");
    CameraScope::register_hks(&mut registry)
        .expect("CameraScope API registration must be internally consistent");
    CameraProjection::register_hks(&mut registry)
        .expect("CameraProjection API registration must be internally consistent");
    register_animation_api(&mut registry)
        .expect("animation API registration must be internally consistent");
    NavigationResetValue::register_hks(&mut registry)
        .expect("navigation reset API registration must be internally consistent");
    registry
        .define_global(
            "settings",
            ScriptType::Record(BTreeMap::from([
                ("bgmVolume".to_string(), ScriptType::Float),
                ("voiceVolume".to_string(), ScriptType::Float),
                ("sfxVolume".to_string(), ScriptType::Float),
            ])),
        )
        .expect("engine settings schema must be defined once");
    native_api::register_hks(&mut registry)
        .expect("story native API registration must be internally consistent");
    for name in [
        "intrinsics.engine.say",
        "intrinsics.engine.continueDialogue",
    ] {
        let id = registry
            .manifest()
            .resolve(name)
            .expect("dialogue primitive is registered");
        registry
            .require_capability(id, super::stdlib::DIALOGUE_CAPABILITY)
            .expect("dialogue primitive capability is valid");
    }
    for name in [
        "intrinsics.engine.actorIdentity",
        "intrinsics.engine.aliasActor",
        "intrinsics.engine.cloneActor",
        "intrinsics.engine.appendExpression",
        "intrinsics.engine.submitActor",
        "intrinsics.engine.placeActor",
    ] {
        let id = registry
            .manifest()
            .resolve(name)
            .expect("actor primitive is registered");
        registry
            .require_capability(id, super::stdlib::ACTOR_CAPABILITY)
            .expect("actor capability");
    }
    story_api::register_hks(&mut registry)
        .expect("story navigation API registration must be internally consistent");
    registry
}

fn story_registry() -> NativeRegistry<CharacterContext> {
    let mut registry = registry();
    random_api::register_hks(&mut registry).expect("random API registration");
    profile_api::register_hks(&mut registry).expect("profile API must register once");
    ui_api::register_hks(&mut registry)
        .expect("story UI API registration must be internally consistent");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("ui.open_any"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String],
                variadic: Some(ScriptType::Any),
                result: ScriptType::Any,
            },
        )
        .expect("raw UI API is registered");
    registry
        .set_signature(
            hiraku_script::native::stable_builtin_id("ui.open"),
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String],
                variadic: Some(ScriptType::Any),
                result: ScriptType::Any,
            },
        )
        .expect("ui.open signature must target its registered builtin");
    registry
        .register_raw_fn("wait", async_capability_placeholder)
        .expect("built-in `wait` registration must be unique");
    let await_task = registry
        .register_raw_fn("await", async_capability_placeholder)
        .expect("await registration must be unique");
    registry
        .set_signature(
            await_task,
            hiraku_script::FunctionSignature {
                receiver: Some(ScriptType::Task),
                parameters: vec![],
                variadic: None,
                result: ScriptType::Unit,
            },
        )
        .expect("await has a typed task receiver");
    for name in ["seq", "par"] {
        let builtin = registry
            .register_raw_fn(name, async_capability_placeholder)
            .expect("task closure builtin registration must be unique");
        registry
            .set_signature(
                builtin,
                hiraku_script::FunctionSignature {
                    receiver: None,
                    parameters: vec![ScriptType::Function],
                    variadic: None,
                    result: ScriptType::Task,
                },
            )
            .expect("task closure signature must target its registered builtin");
    }
    registry
        .register_raw_fn("choice", async_capability_placeholder)
        .expect("built-in `choice` registration must be unique");
    let option = registry
        .register_raw_fn("option", async_capability_placeholder)
        .expect("built-in `option` registration must be unique");
    let option_type = registry
        .manifest()
        .symbols()
        .find("ChoiceOption")
        .expect("choice option type is registered");
    registry
        .set_signature(
            option,
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String, ScriptType::Function],
                variadic: None,
                result: ScriptType::Named(option_type),
            },
        )
        .expect("option signature must target its registered builtin");
    registry
}

#[hiraku_script::hks_module]
mod random_api {
    use super::*;
    #[hks(name = "randomInt")]
    fn random_int(
        _context: &mut CharacterContext,
        _min: i32,
        _max: i32,
    ) -> Result<i32, NativeError> {
        Err(NativeError::message(
            "randomInt requires a story host response",
        ))
    }
}

fn async_capability_placeholder(
    _context: &mut CharacterContext,
    _call: &BuiltinCall,
) -> Result<Value, NativeError> {
    Err(NativeError::message(
        "async capability requires the direct engine HKS runtime",
    ))
}

#[hiraku_script::hks_module("ui")]
mod ui_api {
    use super::*;

    #[hks(name = "open_any")]
    fn native_open_any(
        _context: &mut CharacterContext,
        _role_or_component: String,
    ) -> Result<Value, NativeError> {
        Err(NativeError::message(
            "ui.open_any requires the direct engine HKS runtime",
        ))
    }

    #[hks]
    fn native_open(
        _context: &mut CharacterContext,
        _role_or_component: String,
    ) -> Result<Value, NativeError> {
        Err(NativeError::message(
            "ui.open requires the direct engine HKS runtime",
        ))
    }

    #[hks]
    fn native_set(
        context: &mut CharacterContext,
        role: String,
        component: String,
    ) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::SetUiRole { role, component });
        Ok(())
    }

    #[hks]
    fn native_mount(
        context: &mut CharacterContext,
        name: String,
        component: String,
    ) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::MountUiOverlay {
            name,
            component,
            lifetime: None,
        });
        Ok(())
    }

    /// Presentation-owned lifetime, independent of the calling execution.
    #[hks]
    fn native_mount_for(
        context: &mut CharacterContext,
        name: String,
        component: String,
        seconds: f32,
    ) -> Result<(), NativeError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(NativeError::message(
                "overlay duration must be finite and non-negative",
            ));
        }
        context.commands.push(StoryEffect::MountUiOverlay {
            name,
            component,
            lifetime: Some(seconds),
        });
        Ok(())
    }

    #[hks]
    fn native_unmount(context: &mut CharacterContext, name: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::UnmountUiOverlay { name });
        Ok(())
    }
}

/// Stateful native-function host for the HKS runtime.
///
/// It owns statement-scoped actor builders and exposes effects as plain data so
/// an ECS system can dispatch them without giving native functions world access.
pub struct StoryNativeHost {
    context: CharacterContext,
    registry: NativeRegistry<CharacterContext>,
    controls: StoryControlBuiltins,
}

struct StoryControlBuiltins {
    random_int: BuiltinId,
    enable_option: BuiltinId,
    goto: BuiltinId,
    sequence: BuiltinId,
    parallel: BuiltinId,
    choice: BuiltinId,
    option: BuiltinId,
    open_ui: BuiltinId,
    open_ui_any: BuiltinId,
    wait: BuiltinId,
    await_task: BuiltinId,
}

impl StoryControlBuiltins {
    fn new(manifest: &BuiltinManifest) -> Self {
        Self {
            random_int: manifest
                .resolve("randomInt")
                .expect("random API is registered"),
            open_ui_any: manifest
                .resolve_selector("ui", "open_any")
                .expect("raw UI API is registered"),
            enable_option: manifest
                .resolve("enable")
                .expect("option enable is registered"),
            goto: manifest
                .resolve_selector("story", "goto")
                .expect("story.goto is registered"),
            sequence: manifest.resolve("seq").expect("seq builtin is registered"),
            parallel: manifest.resolve("par").expect("par builtin is registered"),
            choice: manifest
                .resolve("choice")
                .expect("choice builtin is registered"),
            option: manifest
                .resolve("option")
                .expect("option builtin is registered"),
            open_ui: manifest
                .resolve_selector("ui", "open")
                .expect("ui.open builtin is registered"),
            wait: manifest
                .resolve("wait")
                .expect("wait builtin is registered"),
            await_task: manifest
                .resolve("await")
                .expect("await builtin is registered"),
        }
    }
}

impl Default for StoryNativeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl StoryNativeHost {
    pub(super) fn actor_motion_is_current(&self, display: &str, revision: u64) -> bool {
        self.context
            .actors
            .values()
            .any(|actor| actor.display_instance == display && actor.motion_revision == revision)
    }

    pub fn new() -> Self {
        let registry = story_registry();
        let controls = StoryControlBuiltins::new(&registry.manifest());
        Self {
            context: CharacterContext::default(),
            registry,
            controls,
        }
    }

    pub fn call(
        &mut self,
        call: &BuiltinCall,
    ) -> Result<StoryCallOutcome, CharacterCapabilityError> {
        if call.builtin == self.controls.goto {
            let request = NavigationRequest::from_goto_call(call)
                .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
            return Ok(StoryCallOutcome::Control(StoryControl::Navigate(request)));
        }
        if call.builtin == self.controls.sequence || call.builtin == self.controls.parallel {
            let closure = call
                .arguments
                .first()
                .map(|argument| argument.value.clone())
                .filter(is_callable)
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "seq/par require a trailing closure",
                ))?;
            let kind = if call.builtin == self.controls.sequence {
                StoryTaskKind::Sequence
            } else {
                StoryTaskKind::Parallel
            };
            return Ok(StoryCallOutcome::Control(StoryControl::SpawnTask {
                kind,
                closure,
            }));
        }
        if call.builtin == self.controls.choice {
            let closure = call
                .arguments
                .iter()
                .find_map(|argument| is_callable(&argument.value).then(|| argument.value.clone()))
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "choice requires a trailing closure",
                ))?;
            let prompt = call
                .arguments
                .iter()
                .find_map(|argument| match &argument.value {
                    Value::String(prompt) => Some(prompt.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            return Ok(StoryCallOutcome::Control(StoryControl::BeginChoice {
                prompt,
                closure,
            }));
        }
        if call.builtin == self.controls.option {
            let label = call
                .arguments
                .first()
                .and_then(|argument| match &argument.value {
                    Value::String(label) => Some(label.clone()),
                    _ => None,
                })
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "option requires a string label",
                ))?;
            let closure = call
                .arguments
                .iter()
                .find_map(|argument| is_callable(&argument.value).then(|| argument.value.clone()))
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "option requires a trailing closure",
                ))?;
            return Ok(StoryCallOutcome::Control(StoryControl::AddChoiceOption {
                label,
                closure,
            }));
        }
        if call.builtin == self.controls.enable_option {
            let Some(Value::Handle {
                type_id: CHOICE_OPTION_HANDLE_TYPE,
                id,
            }) = &call.receiver
            else {
                return Err(CharacterCapabilityError::InvalidArguments(
                    "enable requires a ChoiceOption receiver",
                ));
            };
            let Some(hiraku_script::runtime::CallArgument {
                value: Value::Bool(enabled),
                ..
            }) = call.arguments.first()
            else {
                return Err(CharacterCapabilityError::InvalidArguments(
                    "enable requires Bool",
                ));
            };
            return Ok(StoryCallOutcome::Control(
                StoryControl::EnableChoiceOption {
                    id: *id,
                    enabled: *enabled,
                },
            ));
        }
        if call.builtin == self.controls.random_int {
            let values = call
                .arguments
                .iter()
                .map(|a| match a.value {
                    Value::Number(n)
                        if n.is_finite()
                            && n.fract() == 0.0
                            && n >= i32::MIN as f64
                            && n <= i32::MAX as f64 =>
                    {
                        Some(n as i64)
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "randomInt expects exact integers",
                ))?;
            let [min, max] = values.as_slice() else {
                return Err(CharacterCapabilityError::InvalidArguments(
                    "randomInt requires min and exclusive max",
                ));
            };
            if min >= max {
                return Err(CharacterCapabilityError::InvalidArguments(
                    "randomInt requires min < max",
                ));
            }
            return Ok(StoryCallOutcome::Control(StoryControl::RandomInt {
                min: *min,
                max: *max,
            }));
        }
        if call.builtin == self.controls.open_ui || call.builtin == self.controls.open_ui_any {
            let path = call
                .arguments
                .first()
                .and_then(|argument| match &argument.value {
                    Value::String(path) => Some(path.clone()),
                    _ => None,
                })
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "ui.open requires a string role or component path",
                ))?;
            return Ok(StoryCallOutcome::Control(StoryControl::OpenUi {
                path,
                arguments: call
                    .arguments
                    .iter()
                    .skip(1)
                    .map(|argument| argument.value.clone())
                    .collect(),
            }));
        }
        if call.builtin == self.controls.wait || call.builtin == self.controls.await_task {
            let task = call
                .receiver
                .as_ref()
                .or_else(|| call.arguments.first().map(|argument| &argument.value))
                .and_then(|value| match value {
                    Value::Task(task) => Some(*task),
                    _ => None,
                })
                .ok_or(CharacterCapabilityError::InvalidArguments(
                    "wait requires a task handle",
                ))?;
            return Ok(StoryCallOutcome::Control(StoryControl::WaitTask { task }));
        }

        self.registry
            .call(&mut self.context, call)
            .map(StoryCallOutcome::Return)
            .map_err(|error| CharacterCapabilityError::Native(error.to_string()))
    }

    #[cfg(test)]
    pub fn commit_statement(&mut self) -> Result<(), CharacterCapabilityError> {
        self.context.commit()
    }

    pub fn handle_statement(
        &mut self,
        statement: &StatementValue,
    ) -> Result<(), CharacterCapabilityError> {
        self.context.handle_statement(statement)
    }

    pub fn drain_effects(&mut self) -> Vec<StoryEffect> {
        std::mem::take(&mut self.context.commands)
    }

    pub fn take_wait(&mut self) -> Option<StoryWait> {
        self.context.wait.take()
    }

    pub(crate) fn take_animation_await(&mut self) -> bool {
        std::mem::take(&mut self.context.await_effects)
    }

    pub fn snapshot(&self) -> StoryNativeHostSnapshot {
        StoryNativeHostSnapshot {
            await_effects: self.context.await_effects,
            next_handle: self.context.next_handle,
            actors: self.context.actors.clone(),
            handles_by_name: self.context.handles_by_name.clone(),
            last_speaker: self.context.last_speaker.clone(),
            dialogue_buffer: self.context.dialogue_buffer.clone(),
            sound: self.context.sound.clone(),
            movie: self.context.movie.clone(),
            next_camera_handle: self.context.next_camera_handle,
            pending_cameras: self.context.pending_cameras.clone(),
            scene_visuals: self.context.scene_visuals.clone(),
        }
    }

    pub fn reset_presentation(&mut self) {
        for actor in self.context.actors.values_mut() {
            actor.visible = false;
            actor.dirty = false;
        }
        self.context.last_speaker = None;
        self.context.dialogue_buffer = None;
    }

    pub fn restore(snapshot: StoryNativeHostSnapshot) -> Self {
        let registry = story_registry();
        let controls = StoryControlBuiltins::new(&registry.manifest());
        Self {
            context: CharacterContext {
                await_effects: snapshot.await_effects,
                next_handle: snapshot.next_handle,
                actors: snapshot.actors,
                handles_by_name: snapshot.handles_by_name,
                commands: Vec::new(),
                wait: None,
                last_speaker: snapshot.last_speaker,
                dialogue_buffer: snapshot.dialogue_buffer,
                sound: snapshot.sound,
                movie: snapshot.movie,
                next_camera_handle: snapshot.next_camera_handle,
                pending_cameras: snapshot.pending_cameras,
                scene_visuals: snapshot.scene_visuals,
            },
            registry,
            controls,
        }
    }
}

fn is_callable(value: &Value) -> bool {
    matches!(value, Value::Closure { .. } | Value::Function { .. })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoryNativeHostSnapshot {
    await_effects: bool,
    scene_visuals: scene_visuals::SceneVisualState,
    next_handle: u64,
    actors: BTreeMap<u64, ActorPresentation>,
    handles_by_name: BTreeMap<String, u64>,
    last_speaker: Option<String>,
    dialogue_buffer: Option<String>,
    sound: sound::SoundState,
    movie: movie::MovieState,
    next_camera_handle: u64,
    pending_cameras: BTreeMap<u64, PendingCamera>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ActorPresentation {
    rotation: f32,
    display_instance: String,
    placement_animation: Option<AnimationSpec>,
    name: String,
    instance: String,
    motion_revision: u64,
    pending_offset: Option<super::actor_motion::ActorOffset>,
    expressions: Vec<String>,
    position: [f32; 2],
    scale: f32,
    dirty: bool,
    focused: bool,
    visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingCamera {
    blur: Option<f32>,
    zoom: Option<f32>,
    zoom_view_space: bool,
    offset: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    projection: Option<CameraProjectionMode>,
    scope: CameraEffectScope,
    duration_ms: u64,
    ease: Easing,
}

#[derive(Default)]
struct CharacterContext {
    await_effects: bool,
    scene_visuals: scene_visuals::SceneVisualState,
    next_handle: u64,
    actors: BTreeMap<u64, ActorPresentation>,
    handles_by_name: BTreeMap<String, u64>,
    commands: Vec<StoryEffect>,
    wait: Option<StoryWait>,
    last_speaker: Option<String>,
    dialogue_buffer: Option<String>,
    sound: sound::SoundState,
    movie: movie::MovieState,
    next_camera_handle: u64,
    pending_cameras: BTreeMap<u64, PendingCamera>,
}

impl CharacterContext {
    fn handle_statement(
        &mut self,
        statement: &StatementValue,
    ) -> Result<(), CharacterCapabilityError> {
        self.commit()?;
        if let StatementValue::String(text) = statement {
            native_api::native_narrate(self, TextTemplate(text.clone()))
                .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
        }
        Ok(())
    }

    fn char(&mut self, name: String) -> Result<ActorIdentity, CharacterCapabilityError> {
        self.character_instance(name.clone(), name)
    }

    fn character_instance(
        &mut self,
        name: String,
        instance: String,
    ) -> Result<ActorIdentity, CharacterCapabilityError> {
        if instance.is_empty() || instance.contains("::") {
            return Err(CharacterCapabilityError::InvalidArguments(
                "actor instance must be nonempty and cannot contain ::",
            ));
        }
        if let Some(handle) = self.handles_by_name.get(&instance).copied() {
            if self.actor_mut(handle)?.name != name {
                return Err(CharacterCapabilityError::InvalidArguments(
                    "actor instance is already assigned to another character",
                ));
            }
            return Ok(ActorIdentity(handle));
        }
        self.next_handle += 1;
        let handle = self.next_handle;
        self.handles_by_name.insert(instance.clone(), handle);
        let mut actor = pending_actor(&name);
        actor.instance = instance;
        actor.display_instance = actor.instance.clone();
        self.actors.insert(handle, actor);
        Ok(ActorIdentity(handle))
    }

    fn emotion(
        &mut self,
        ActorIdentity(handle): ActorIdentity,
        emotion: String,
    ) -> Result<ActorIdentity, CharacterCapabilityError> {
        let pending = self.actor_mut(handle)?;
        pending.expressions.retain(|previous| previous != &emotion);
        pending.expressions.push(emotion);
        pending.dirty = true;
        Ok(ActorIdentity(handle))
    }

    fn at(
        &mut self,
        ActorIdentity(handle): ActorIdentity,
        position: Position,
    ) -> Result<ActorIdentity, CharacterCapabilityError> {
        self.actor_mut(handle)?.position = position.resolve();
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorIdentity(handle))
    }

    fn scale(
        &mut self,
        ActorIdentity(handle): ActorIdentity,
        scale: f64,
    ) -> Result<ActorIdentity, CharacterCapabilityError> {
        if scale <= 0.0 {
            return Err(CharacterCapabilityError::InvalidArguments(
                "scale must be positive",
            ));
        }
        self.actor_mut(handle)?.scale = scale as f32;
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorIdentity(handle))
    }

    fn focus(
        &mut self,
        ActorIdentity(handle): ActorIdentity,
        focused: bool,
    ) -> Result<ActorIdentity, CharacterCapabilityError> {
        self.actor_mut(handle)?.focused = focused;
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorIdentity(handle))
    }

    fn camera(&mut self, scope: CameraScope) -> CameraHandle {
        self.next_camera_handle += 1;
        let handle = self.next_camera_handle;
        self.pending_cameras.insert(
            handle,
            PendingCamera {
                blur: None,
                zoom: None,
                zoom_view_space: false,
                offset: None,
                rotation: None,
                projection: None,
                scope: match scope {
                    CameraScope::Scene => CameraEffectScope::World,
                    CameraScope::Canvas => CameraEffectScope::Canvas,
                },
                duration_ms: 0,
                ease: Easing::Linear,
            },
        );
        CameraHandle(handle)
    }

    fn camera_mut(&mut self, handle: u64) -> Result<&mut PendingCamera, NativeError> {
        self.pending_cameras
            .get_mut(&handle)
            .ok_or_else(|| NativeError::message(format!("unknown camera handle {handle}")))
    }

    fn invalidate_actor_motions(&mut self, display: Option<&str>) -> Result<(), NativeError> {
        let revision = self
            .actors
            .values()
            .map(|actor| actor.motion_revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| NativeError::message("actor motion revisions exhausted"))?;
        for actor in self
            .actors
            .values_mut()
            .filter(|actor| display.is_none_or(|id| actor.display_instance == id))
        {
            actor.motion_revision = revision;
            actor.pending_offset = None;
        }
        Ok(())
    }

    fn actor_mut(
        &mut self,
        handle: u64,
    ) -> Result<&mut ActorPresentation, CharacterCapabilityError> {
        self.actors
            .get_mut(&handle)
            .ok_or(CharacterCapabilityError::UnknownActor(handle))
    }

    fn flush(&mut self, handle: u64) -> Result<(), CharacterCapabilityError> {
        let command = {
            let pending = self.actor_mut(handle)?;
            if !pending.dirty || !pending.visible {
                return Ok(());
            }
            pending.dirty = false;
            StoryEffect::ShowCharacter {
                rotation: pending.rotation,
                placement_animation: pending.placement_animation.take(),
                actor_id: pending.display_instance.clone(),
                character_name: pending.name.clone(),
                expressions: pending.expressions.clone(),
                position: pending.position,
                scale: pending.scale,
                focused: pending.focused,
            }
        };
        self.commands.push(command);
        Ok(())
    }

    fn commit(&mut self) -> Result<(), CharacterCapabilityError> {
        self.scene_visuals.commit(&mut self.commands);
        self.sound.commit(&mut self.commands);
        self.movie.commit(&mut self.commands, &mut self.wait);
        self.commit_cameras();
        Ok(())
    }

    fn commit_actor(&mut self, handle: u64) -> Result<(), CharacterCapabilityError> {
        self.flush(handle)?;
        if self.actor_mut(handle)?.pending_offset.is_none() {
            return Ok(());
        }
        let next_revision = self
            .actors
            .values()
            .map(|actor| actor.motion_revision)
            .max()
            .unwrap_or(0);
        let actor = self.actor_mut(handle)?;
        if let Some(mut transition) = actor.pending_offset.take() {
            if !actor.visible {
                // A child sequence can reach its first offset after the
                // root story has hidden the actor. Commit the target and
                // complete normally, without resurrecting the actor.
                transition.animation = super::AnimationSpec::Linear(0.0, false);
            }
            actor.motion_revision =
                next_revision
                    .checked_add(1)
                    .ok_or(CharacterCapabilityError::InvalidArguments(
                        "actor motion revisions exhausted",
                    ))?;
            let effect = StoryEffect::ActorMotion {
                actor_id: actor.display_instance.clone(),
                revision: actor.motion_revision,
                transition,
            };
            self.commands.push(effect);
        }
        Ok(())
    }

    fn commit_cameras(&mut self) {
        let cameras = std::mem::take(&mut self.pending_cameras);
        for (_, pending) in cameras {
            if pending.blur.is_some()
                || pending.zoom.is_some()
                || pending.offset.is_some()
                || pending.rotation.is_some()
                || pending.projection.is_some()
            {
                self.commands.push(StoryEffect::SetCamera {
                    blur: pending.blur,
                    zoom: pending.zoom,
                    zoom_view_space: pending.zoom_view_space,
                    offset: pending.offset,
                    rotation: pending.rotation,
                    projection: pending.projection,
                    scope: pending.scope,
                    duration_ms: pending.duration_ms,
                    ease: pending.ease,
                });
            }
        }
    }
}

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "ActorIdentity", handle_type = ACTOR_HANDLE_TYPE)]
struct ActorIdentity(u64);

pub(super) const CHOICE_OPTION_HANDLE_TYPE: u32 = 0x434f5054;
#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "ChoiceOption", handle_type = CHOICE_OPTION_HANDLE_TYPE)]
struct ChoiceOptionHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Camera", handle_type = CAMERA_HANDLE_TYPE)]
struct CameraHandle(u64);

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum Position {
    Absolute(f64, f64),
    Relative(f64, f64),
    Left,
    Center,
    Right,
}

impl Position {
    fn pos(x: f64, y: f64) -> Position {
        Self::Absolute(x, y)
    }

    fn rel(x: f64, y: f64) -> Result<Position, NativeError> {
        fn component(value: f64) -> Result<f64, NativeError> {
            if !value.is_finite() || value.abs() > 100000.0 {
                return Err(NativeError::message(
                    "relative position components must be finite and within 100000 percent",
                ));
            }
            Ok(value)
        }
        Ok(Self::Relative(component(x)?, component(y)?))
    }

    #[getter]
    fn left() -> Position {
        Self::Left
    }

    #[getter]
    fn center() -> Position {
        Self::Center
    }

    #[getter]
    fn right() -> Position {
        Self::Right
    }
}
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum CameraScope {
    Scene,
    Canvas,
}

impl CameraScope {
    #[getter]
    fn scene() -> CameraScope { Self::Scene }

    #[getter]
    fn canvas() -> CameraScope { Self::Canvas }
}
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum CameraProjection {
    Orthographic,
    Perspective,
}

impl CameraProjection {
    #[getter]
    fn orthographic() -> CameraProjection { Self::Orthographic }
    #[getter]
    fn perspective() -> CameraProjection { Self::Perspective }
}
}

impl Position {
    fn resolve(self) -> [f32; 2] {
        match self {
            Self::Left => [-600.0, -200.0],
            Self::Center => [0.0, -200.0],
            Self::Right => [600.0, -200.0],
            Self::Absolute(x, y) => [x as f32, y as f32],
            // Relative coordinates use a bottom-left origin in the canonical 1920x1080 canvas.
            Self::Relative(x, y) => [
                x as f32 / 100.0 * 1920.0 - 960.0,
                y as f32 / 100.0 * 1080.0 - 540.0,
            ],
        }
    }
}

fn hide_duration(value: Option<f64>) -> Result<u64, NativeError> {
    let value = value.unwrap_or(0.0);
    if !value.is_finite() || !(0.0..=60_000.0).contains(&value) {
        return Err(NativeError::message(
            "hide fade duration must be between 0 and 60000 milliseconds",
        ));
    }
    Ok(value.round() as u64)
}

#[hiraku_script::hks_module]
mod native_api {
    use super::*;

    #[hks(name = "intrinsics.engine.appendExpression")]
    fn append_expression(
        _: &mut CharacterContext,
        mut expressions: Vec<String>,
        emotion: String,
    ) -> Result<Vec<String>, NativeError> {
        expressions.retain(|previous| previous != &emotion);
        expressions.push(emotion);
        Ok(expressions)
    }

    #[hks(name = "intrinsics.engine.submitActor")]
    fn submit_actor(
        context: &mut CharacterContext,
        patch: Value,
        await_effects: bool,
    ) -> Result<(), NativeError> {
        actor_patch::apply(context, patch, await_effects)
    }

    #[hks(name = "enable", receiver)]
    fn native_option_enable(
        _context: &mut CharacterContext,
        _option: ChoiceOptionHandle,
        _enabled: bool,
    ) -> Result<ChoiceOptionHandle, NativeError> {
        Err(NativeError::message(
            "option enable requires the story choice builder",
        ))
    }

    #[hks(name = "intrinsics.engine.actorIdentity")]
    fn native_char(
        context: &mut CharacterContext,
        name: String,
    ) -> Result<ActorIdentity, NativeError> {
        context
            .char(name)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks(name = "intrinsics.engine.cloneActor")]
    pub(super) fn native_clone(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        instance: String,
    ) -> Result<ActorIdentity, NativeError> {
        let name = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?
            .name
            .clone();
        if let Some(handle) = context.handles_by_name.get(&instance).copied() {
            let pending = context
                .actor_mut(handle)
                .map_err(|e| NativeError::message(e.to_string()))?;
            if pending.display_instance != instance {
                return Err(NativeError::message(
                    "actor clone name is already used by an alias",
                ));
            }
        }
        context
            .character_instance(name, instance)
            .map_err(|e| NativeError::message(e.to_string()))
    }

    #[hks(name = "intrinsics.engine.aliasActor")]
    pub(super) fn native_alias(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        alias: String,
    ) -> Result<ActorIdentity, NativeError> {
        let display = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?
            .display_instance
            .clone();
        let name = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?
            .name
            .clone();
        if let Some(handle) = context.handles_by_name.get(&alias).copied() {
            if context
                .actor_mut(handle)
                .map_err(|e| NativeError::message(e.to_string()))?
                .display_instance
                != display
            {
                return Err(NativeError::message(
                    "actor alias name is already assigned to another display",
                ));
            }
        }
        let handle = context
            .character_instance(name, alias)
            .map_err(|e| NativeError::message(e.to_string()))?;
        let pending = context
            .actor_mut(handle.0)
            .map_err(|e| NativeError::message(e.to_string()))?;
        pending.display_instance = display;
        Ok(handle)
    }

    pub(super) fn native_emotion(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        emotion: String,
    ) -> Result<ActorIdentity, NativeError> {
        context
            .emotion(actor, emotion)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    pub(super) fn native_show(
        context: &mut CharacterContext,
        actor: ActorIdentity,
    ) -> Result<ActorIdentity, NativeError> {
        let display = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?
            .display_instance
            .clone();
        if context.actors.iter().any(|(id, pending)| {
            *id != actor.0 && pending.visible && pending.display_instance == display
        }) {
            context.invalidate_actor_motions(Some(&display))?;
        }
        for (id, pending) in &mut context.actors {
            if *id != actor.0 && pending.display_instance == display {
                pending.visible = false;
                pending.pending_offset = None;
            }
        }
        let pending = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?;
        pending.visible = true;
        pending.dirty = true;
        Ok(actor)
    }

    pub(super) fn native_hide(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        fade_ms: Option<f64>,
    ) -> Result<ActorIdentity, NativeError> {
        let fade_ms = hide_duration(fade_ms)?;
        let pending = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?;
        pending.dirty = false;
        pending.visible = false;
        let actor_id = pending.display_instance.clone();
        context.invalidate_actor_motions(Some(&actor_id))?;
        for pending in context
            .actors
            .values_mut()
            .filter(|pending| pending.display_instance == actor_id)
        {
            pending.visible = false;
            pending.pending_offset = None;
        }
        context.commands.push(StoryEffect::HideCharacter {
            actor_id: Some(actor_id),
            fade_ms,
        });
        Ok(actor)
    }

    #[hks(name = "hideCharacters", selector = "scene")]
    fn native_hide_characters(
        context: &mut CharacterContext,
        fade_ms: Option<f64>,
    ) -> Result<scene_visuals::SceneTransitionHandle, NativeError> {
        let fade_ms = hide_duration(fade_ms)?;
        context.invalidate_actor_motions(None)?;
        for actor in context.actors.values_mut() {
            actor.dirty = false;
            actor.visible = false;
        }
        context.scene_visuals.hide_characters(fade_ms)
    }

    #[hks(name = "save", selector = "story")]
    fn native_save(context: &mut CharacterContext, slot: String) -> Result<(), NativeError> {
        if slot.is_empty()
            || !slot
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(NativeError::message(
                "save slot must contain letters, digits, underscores or hyphens",
            ));
        }
        context.commands.push(StoryEffect::SaveSlot(slot));
        Ok(())
    }

    pub(super) fn native_actor_offset(
        context: &mut CharacterContext,
        ActorIdentity(handle): ActorIdentity,
        position: Position,
    ) -> Result<ActorIdentity, NativeError> {
        let Position::Absolute(x, y) = position else {
            return Err(NativeError::message(
                "offset uses pixel coordinates: .pos(x, y)",
            ));
        };
        let transition = super::super::actor_motion::ActorOffset {
            oscillation: None,
            target: [x as f32, y as f32],
            animation: AnimationSpec::EaseOut(0.3, false),
        };
        transition.validate().map_err(NativeError::message)?;
        let actor = context
            .actor_mut(handle)
            .map_err(|e| NativeError::message(e.to_string()))?;
        actor.pending_offset = Some(transition);
        Ok(ActorIdentity(handle))
    }

    /// Add a transient two-axis wave to placement, sampled with story time.
    pub(super) fn native_actor_oscillate(
        context: &mut CharacterContext,
        ActorIdentity(handle): ActorIdentity,
        amplitude: Position,
        period_x: f64,
        period_y: f64,
    ) -> Result<ActorIdentity, NativeError> {
        let Position::Absolute(x, y) = amplitude else {
            return Err(NativeError::message(
                "oscillation amplitude uses .pos(x, y) canvas units",
            ));
        };
        let transition = super::super::actor_motion::ActorOffset {
            target: [0.0; 2],
            animation: AnimationSpec::Linear(0.3, false),
            oscillation: Some(super::super::actor_motion::ActorOscillation {
                amplitude: [x as f32, y as f32],
                period: [period_x as f32, period_y as f32],
            }),
        };
        transition.validate().map_err(NativeError::message)?;
        context
            .actor_mut(handle)
            .map_err(|e| NativeError::message(e.to_string()))?
            .pending_offset = Some(transition);
        Ok(ActorIdentity(handle))
    }

    pub(super) fn native_actor_stop_motion(
        context: &mut CharacterContext,
        actor: ActorIdentity,
    ) -> Result<ActorIdentity, NativeError> {
        let actor_id = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?
            .display_instance
            .clone();
        context.invalidate_actor_motions(Some(&actor_id))?;
        context
            .commands
            .push(StoryEffect::StopActorMotion { actor_id });
        Ok(actor)
    }

    pub(super) fn actor_time(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        seconds: f64,
    ) -> Result<ActorIdentity, NativeError> {
        let spec = actor_animation_spec(context, actor)?.with_time(seconds)?;
        native_actor_animation(context, actor, spec)
    }
    pub(super) fn actor_easing(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        easing: Easing,
    ) -> Result<ActorIdentity, NativeError> {
        let spec = actor_animation_spec(context, actor)?.with_easing(easing)?;
        native_actor_animation(context, actor, spec)
    }
    fn actor_animation_spec(
        context: &mut CharacterContext,
        actor: ActorIdentity,
    ) -> Result<AnimationSpec, NativeError> {
        let actor = context
            .actor_mut(actor.0)
            .map_err(|e| NativeError::message(e.to_string()))?;
        Ok(actor
            .pending_offset
            .map(|v| v.animation)
            .or(actor.placement_animation)
            .unwrap_or(AnimationSpec::EaseOut(0.3, false)))
    }
    fn native_actor_animation(
        context: &mut CharacterContext,
        ActorIdentity(handle): ActorIdentity,
        animation: AnimationSpec,
    ) -> Result<ActorIdentity, NativeError> {
        let actor = context
            .actor_mut(handle)
            .map_err(|e| NativeError::message(e.to_string()))?;
        let Some(transition) = actor.pending_offset.as_mut() else {
            let validation = super::super::actor_motion::ActorOffset {
                oscillation: None,
                target: [0.0; 2],
                animation,
            };
            validation.validate().map_err(NativeError::message)?;
            if !actor.dirty {
                return Err(NativeError::message(
                    "actor animation requires an uncommitted position or scale",
                ));
            }
            actor.placement_animation = Some(animation);
            return Ok(ActorIdentity(handle));
        };
        let updated = super::super::actor_motion::ActorOffset {
            animation,
            ..*transition
        };
        updated.validate().map_err(NativeError::message)?;
        *transition = updated;
        Ok(ActorIdentity(handle))
    }

    pub(super) fn await_actor(
        context: &mut CharacterContext,
        ActorIdentity(id): ActorIdentity,
    ) -> Result<(), NativeError> {
        let actor = context
            .actor_mut(id)
            .map_err(|e| NativeError::message(e.to_string()))?;
        let pending = actor.pending_offset.is_some() || (actor.dirty && actor.visible);
        let instance = actor.display_instance.clone();
        let hiding = context.commands.iter().any(|e| {
            matches!(e,
            StoryEffect::HideCharacter { actor_id: Some(id), .. } if id == &instance)
        });
        if !pending && !hiding {
            return Err(NativeError::message(
                "await requires an actor animation in the same statement",
            ));
        }
        context.await_effects = true;
        Ok(())
    }

    #[hks(name = "await", selector = "Camera", receiver)]
    fn await_camera(
        context: &mut CharacterContext,
        CameraHandle(id): CameraHandle,
    ) -> Result<(), NativeError> {
        context.camera_mut(id)?;
        context.await_effects = true;
        Ok(())
    }

    pub(super) fn native_at(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        position: Position,
    ) -> Result<ActorIdentity, NativeError> {
        context
            .at(actor, position)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    pub(super) fn native_scale(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        scale: f64,
    ) -> Result<ActorIdentity, NativeError> {
        context
            .scale(actor, scale)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    pub(super) fn native_actor_rotation(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        degrees: f64,
    ) -> Result<ActorIdentity, NativeError> {
        if !degrees.is_finite() || !(degrees as f32).is_finite() {
            return Err(NativeError::message("actor rotation must be finite"));
        }
        let pending = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?;
        pending.rotation = degrees as f32;
        pending.dirty = true;
        Ok(actor)
    }

    /// Scene-space depth, shared by aliases but independent for clones. Leave
    /// one unit below the curtain for stable ordering within each depth band.
    pub(super) fn native_actor_depth(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        depth: f64,
    ) -> Result<ActorIdentity, NativeError> {
        if !depth.is_finite() || !(0.0..=29.0).contains(&depth) {
            return Err(NativeError::message(
                "actor depth must be finite and in 0..=29 (below the curtain)",
            ));
        }
        let id = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?
            .display_instance
            .clone();
        context.commands.push(StoryEffect::SetActorDepth {
            id,
            depth: depth as f32,
        });
        Ok(actor)
    }

    /// Clipping belongs to the display identity, shared by aliases but not clones.
    pub(super) fn native_actor_clip(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        region: Option<String>,
    ) -> Result<ActorIdentity, NativeError> {
        let id = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?
            .display_instance
            .clone();
        context.commands.push(StoryEffect::Clip(
            crate::scene::clipping::ClipCommand::Actor { id, region },
        ));
        Ok(actor)
    }

    pub(super) fn native_focus(
        context: &mut CharacterContext,
        actor: ActorIdentity,
        focused: Option<bool>,
    ) -> Result<ActorIdentity, NativeError> {
        context
            .focus(actor, focused.unwrap_or(true))
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks]
    fn native_log(context: &mut CharacterContext, message: String) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::Log(message));
        Ok(())
    }

    #[hks]
    fn native_clear_text(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.last_speaker = None;
        context.dialogue_buffer = None;
        context.commands.push(StoryEffect::ClearDialogue);
        Ok(())
    }

    #[hks]
    fn native_exit(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::Exit);
        Ok(())
    }

    #[hks]
    fn native_adjust_setting(
        context: &mut CharacterContext,
        name: String,
        delta: f64,
    ) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::AdjustSetting {
            name,
            delta: delta as f32,
        });
        Ok(())
    }

    #[hks(name = "camera")]
    fn native_camera(
        context: &mut CharacterContext,
        scope: Option<CameraScope>,
    ) -> Result<CameraHandle, NativeError> {
        Ok(context.camera(scope.unwrap_or(CameraScope::Scene)))
    }

    #[hks(name = "blur", receiver)]
    fn native_camera_blur(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        intensity: f64,
    ) -> Result<CameraHandle, NativeError> {
        if !intensity.is_finite() || intensity < 0.0 {
            return Err(NativeError::message(
                "camera blur intensity must be non-negative",
            ));
        }
        context.camera_mut(handle)?.blur = Some(intensity as f32);
        Ok(CameraHandle(handle))
    }

    #[hks(name = "zoom", receiver)]
    fn native_camera_zoom(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        zoom: f64,
    ) -> Result<CameraHandle, NativeError> {
        if !zoom.is_finite() || zoom <= 0.0 {
            return Err(NativeError::message("camera zoom must be positive"));
        }
        context.camera_mut(handle)?.zoom = Some(zoom as f32);
        context.camera_mut(handle)?.zoom_view_space = false;
        Ok(CameraHandle(handle))
    }

    /// Scale of the visible camera area relative to the default view.
    /// Unlike zoom(), transitions interpolate view size, not magnification.
    #[hks(name = "viewScale", receiver)]
    fn native_camera_view_scale(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        scale: f64,
    ) -> Result<CameraHandle, NativeError> {
        if !scale.is_finite() || !(0.0001..=100.0).contains(&scale) {
            return Err(NativeError::message(
                "camera viewScale must be in 0.0001..=100",
            ));
        }
        let camera = context.camera_mut(handle)?;
        camera.zoom = Some((1.0 / scale) as f32);
        camera.zoom_view_space = true;
        Ok(CameraHandle(handle))
    }

    #[hks(name = "offset", receiver)]
    fn native_camera_offset(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<CameraHandle, NativeError> {
        if ![x, y, z].into_iter().all(f64::is_finite) {
            return Err(NativeError::message("camera offset must be finite"));
        }
        context.camera_mut(handle)?.offset = Some([x as f32, y as f32, z as f32]);
        Ok(CameraHandle(handle))
    }

    #[hks(name = "rotation", receiver)]
    fn native_camera_rotation(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<CameraHandle, NativeError> {
        if ![x, y, z].into_iter().all(f64::is_finite) {
            return Err(NativeError::message("camera rotation must be finite"));
        }
        context.camera_mut(handle)?.rotation = Some([x as f32, y as f32, z as f32]);
        Ok(CameraHandle(handle))
    }

    #[hks(name = "roll", receiver)]
    fn native_camera_roll(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        degrees: f64,
    ) -> Result<CameraHandle, NativeError> {
        if !degrees.is_finite() {
            return Err(NativeError::message("camera roll must be finite"));
        }
        let pending = context.camera_mut(handle)?;
        let mut rotation = pending.rotation.unwrap_or([0.0; 3]);
        rotation[2] = degrees as f32;
        pending.rotation = Some(rotation);
        Ok(CameraHandle(handle))
    }

    #[hks(name = "projection", receiver)]
    fn native_camera_projection(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        projection: CameraProjection,
    ) -> Result<CameraHandle, NativeError> {
        context.camera_mut(handle)?.projection = Some(match projection {
            CameraProjection::Orthographic => CameraProjectionMode::Orthographic,
            CameraProjection::Perspective => CameraProjectionMode::Perspective,
        });
        Ok(CameraHandle(handle))
    }

    #[hks(name = "time", receiver)]
    fn native_camera_time(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        seconds: f64,
    ) -> Result<CameraHandle, NativeError> {
        AnimationSpec::Linear(0.0, false).with_time(seconds)?;
        context.camera_mut(handle)?.duration_ms = (seconds * 1000.0).round() as u64;
        Ok(CameraHandle(handle))
    }

    #[hks(name = "easing", receiver)]
    fn native_camera_easing(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        easing: Easing,
    ) -> Result<CameraHandle, NativeError> {
        easing.validate()?;
        context.camera_mut(handle)?.ease = easing;
        Ok(CameraHandle(handle))
    }

    #[hks]
    pub(super) fn native_narrate(
        context: &mut CharacterContext,
        text: TextTemplate,
    ) -> Result<(), NativeError> {
        let text = text.into_string();
        context.last_speaker = Some(String::new());
        context.dialogue_buffer = Some(text.clone());
        context.commands.push(StoryEffect::Say {
            speaker: String::new(),
            text,
        });
        context.wait = Some(StoryWait::DialogueAdvance);
        Ok(())
    }

    /// Story-authored speed, independent of the user's text-speed preference.
    #[hks(name = "speed", selector = "printer")]
    fn native_printer_speed(
        context: &mut CharacterContext,
        multiplier: f64,
    ) -> Result<(), NativeError> {
        if !multiplier.is_finite()
            || (multiplier as f32) <= 0.0
            || multiplier > f32::MAX as f64 / 30.0
        {
            return Err(NativeError::message(
                "printer.speed requires a finite positive multiplier",
            ));
        }
        context
            .commands
            .push(StoryEffect::DialogueSpeed(multiplier as f32));
        Ok(())
    }

    #[hks(name = "intrinsics.engine.say")]
    fn native_say(
        context: &mut CharacterContext,
        speaker: String,
        text: TextTemplate,
    ) -> Result<(), NativeError> {
        let text = text.into_string();
        context.last_speaker = Some(speaker.clone());
        context.dialogue_buffer = Some(text.clone());
        context.commands.push(StoryEffect::Say { speaker, text });
        context.wait = Some(StoryWait::DialogueAdvance);
        Ok(())
    }

    #[hks(name = "intrinsics.engine.continueDialogue")]
    fn native_continue_dialogue(
        context: &mut CharacterContext,
        text: TextTemplate,
    ) -> Result<(), NativeError> {
        let text = text.into_string();
        if let Some(buffer) = context.dialogue_buffer.as_mut() {
            buffer.push_str(&text);
            context
                .commands
                .push(StoryEffect::ContinueDialogue { text: text.clone() });
            context.wait = Some(StoryWait::DialogueAdvance);
        } else {
            bevy::log::warn!("`...` has no dialogue buffer; treating it as narration");
            native_narrate(context, TextTemplate(text.clone()))?;
        }
        Ok(())
    }
}

#[hiraku_script::hks_module("story")]
mod story_api {
    use super::*;

    #[hks(name = "goto")]
    fn native_goto_story(
        context: &mut CharacterContext,
        _path: String,
        _options: Option<NavigationOptions>,
    ) -> Result<hiraku_script::native::Never, NativeError> {
        let _ = context;
        Err(NativeError::message(
            "story.goto requires a story execution host",
        ))
    }

    #[hks(name = "call")]
    fn native_call_story(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Navigate(NavigationRequest::call(path)?));
        Ok(())
    }
}

fn pending_actor(name: &str) -> ActorPresentation {
    ActorPresentation {
        rotation: 0.0,
        placement_animation: None,
        name: name.to_string(),
        instance: name.to_string(),
        display_instance: name.to_string(),
        motion_revision: 0,
        pending_offset: None,
        expressions: Vec::new(),
        position: [0.0, 0.0],
        scale: 1.0,
        dirty: false,
        focused: false,
        visible: false,
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum CharacterCapabilityError {
    #[error("invalid native arguments: {0}")]
    InvalidArguments(&'static str),
    #[error("unknown actor handle {0}")]
    UnknownActor(u64),
    #[error("HKS native error: {0}")]
    Native(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_depth_uses_display_identity_without_showing_the_actor() {
        compile_story_bytecode("depth.hks", "char(\"alice\").clone(\"closeup\").depth(14)")
            .expect("typed depth API");
        let mut host = StoryNativeHost::new();
        let alice = host.context.char("alice".into()).expect("actor");
        let alias =
            native_api::native_alias(&mut host.context, alice, "alternate".into()).expect("alias");
        let copy =
            native_api::native_clone(&mut host.context, alice, "closeup".into()).expect("clone");
        native_api::native_actor_depth(&mut host.context, alias, 12.0).expect("alias depth");
        native_api::native_actor_depth(&mut host.context, copy, 14.0).expect("clone depth");
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 30.0] {
            assert!(native_api::native_actor_depth(&mut host.context, copy, invalid).is_err());
        }
        host.context.commit().expect("commit");
        assert_eq!(
            host.drain_effects(),
            vec![
                StoryEffect::SetActorDepth {
                    id: "alice".into(),
                    depth: 12.0
                },
                StoryEffect::SetActorDepth {
                    id: "closeup".into(),
                    depth: 14.0
                },
            ]
        );
    }

    #[test]
    fn aliases_share_display_but_preserve_state_and_clones_remain_visible() {
        compile_story_bytecode(
            "aliases.hks",
            "let alice = char(\"alice\")\nlet middle = alice.alias(\"middle\")\nmiddle.show()",
        )
        .expect("alias API compiles");
        let mut host = StoryNativeHost::new();
        let alice = host.context.char("alice".into()).expect("actor");
        let middle =
            native_api::native_alias(&mut host.context, alice, "middle".into()).expect("alias");
        let copy =
            native_api::native_clone(&mut host.context, alice, "copy".into()).expect("clone");
        host.context
            .emotion(alice, "happy".into())
            .expect("expression");
        host.context
            .emotion(middle, "sad".into())
            .expect("expression");
        native_api::native_show(&mut host.context, alice).expect("show");
        native_api::native_show(&mut host.context, copy).expect("show clone");
        native_api::native_show(&mut host.context, middle).expect("switch alias");
        assert!(!host.context.actors[&alice.0].visible);
        assert!(host.context.actors[&middle.0].visible);
        assert!(host.context.actors[&copy.0].visible);
        assert_eq!(host.context.actors[&alice.0].expressions, ["happy"]);
        assert_eq!(host.context.actors[&middle.0].expressions, ["sad"]);
        host.context.commit_actor(middle.0).expect("commit alias");
        host.context.commit_actor(copy.0).expect("commit clone");
        let effects = host.drain_effects();
        let mut displays = effects
            .iter()
            .filter_map(|effect| match effect {
                StoryEffect::ShowCharacter { actor_id, .. } => Some(actor_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        displays.sort();
        assert_eq!(displays, ["alice", "copy"]);
        let mut restored = StoryNativeHost::restore(host.snapshot());
        assert_eq!(restored.context.actors[&middle.0].display_instance, "alice");
        native_api::native_hide(&mut restored.context, alice, None).expect("hide shared display");
        assert!(!restored.context.actors[&middle.0].visible);
        assert!(restored.context.actors[&copy.0].visible);
        assert!(native_api::native_clone(&mut restored.context, alice, "middle".into()).is_err());
        assert!(native_api::native_alias(&mut restored.context, alice, "copy".into()).is_err());
    }

    #[test]
    fn actor_instances_have_independent_state_and_restore_identity() {
        compile_story_bytecode("instances.hks", "let alice = char(\"alice\")\nlet middle = alice.clone(\"alice-middle\")\nmiddle.e(\"happy\").show()")
            .expect("instance fluent API compiles");
        let mut host = StoryNativeHost::new();
        let base = host.context.char("alice".into()).expect("base");
        let middle = host
            .context
            .character_instance("alice".into(), "alice-middle".into())
            .expect("instance");
        host.context
            .emotion(middle, "happy".into())
            .expect("expression");
        native_api::native_show(&mut host.context, middle).expect("show instance");
        host.context
            .commit_actor(middle.0)
            .expect("commit instance");
        assert!(
            matches!(&host.drain_effects()[0], StoryEffect::ShowCharacter { actor_id, character_name, .. }
            if actor_id == "alice-middle" && character_name == "alice")
        );
        assert!(
            host.context
                .actor_mut(base.0)
                .expect("base")
                .expressions
                .is_empty()
        );
        let mut restored = StoryNativeHost::restore(host.snapshot());
        assert_eq!(
            restored
                .context
                .character_instance("alice".into(), "alice-middle".into())
                .expect("restored handle")
                .0,
            middle.0
        );
        assert!(
            restored
                .context
                .character_instance("bob".into(), "alice-middle".into())
                .is_err()
        );
        native_api::native_hide(&mut restored.context, middle, None).expect("hide instance");
        assert!(
            matches!(&restored.drain_effects()[0], StoryEffect::HideCharacter {actor_id: Some(id), ..} if id == "alice-middle")
        );
    }

    #[test]
    fn actor_identity_visibility_and_retained_state_survive_host_restore() {
        let mut host = StoryNativeHost::new();
        let alice = host.context.char("alice".into()).expect("actor handle");
        native_api::native_actor_rotation(&mut host.context, alice, 40.0).expect("rotation");
        assert!(native_api::native_actor_rotation(&mut host.context, alice, f64::NAN).is_err());
        host.context.scale(alice, 0.5).expect("scale");
        host.context
            .emotion(alice, "happy".into())
            .expect("emotion");
        host.context
            .commit_actor(alice.0)
            .expect("commit hidden actor");
        assert!(host.drain_effects().is_empty());
        native_api::native_show(&mut host.context, alice).expect("show");
        host.context.commit_actor(alice.0).expect("commit show");
        host.drain_effects();
        let mut host = StoryNativeHost::restore(host.snapshot());
        assert_eq!(
            host.context
                .actor_mut(alice.0)
                .expect("restored actor")
                .rotation,
            40.0
        );
        assert_eq!(
            host.context.char("alice".into()).expect("same actor").0,
            alice.0
        );
        native_api::native_hide(&mut host.context, alice, None).expect("hide");
        host.drain_effects();
        host.context
            .emotion(alice, "sad".into())
            .expect("edit hidden actor");
        host.context
            .commit_actor(alice.0)
            .expect("commit while hidden");
        assert!(host.drain_effects().is_empty());
        native_api::native_show(&mut host.context, alice).expect("show retained actor");
        host.context
            .commit_actor(alice.0)
            .expect("commit retained actor");
        assert!(
            matches!(host.drain_effects().as_slice(),[StoryEffect::ShowCharacter {scale,expressions,..}] if *scale==0.5 && expressions==&["happy","sad"])
        );
    }
    use crate::script::execution_runtime::{ExecutionEvent, ExecutionRuntime};

    #[test]
    fn engine_settings_roundtrip_through_the_fixed_global_record() {
        let original = UserSettings {
            bgm_volume: 0.8,
            voice_volume: 0.7,
            sfx_volume: 0.6,
            ..UserSettings::default()
        };
        let mut globals = engine_globals(&original);
        if let Some(Value::Map(settings)) = globals.get_mut("settings") {
            settings.insert("bgmVolume".to_string(), Value::Number(0.25));
        } else {
            panic!("settings must be a record")
        }
        let mut restored = UserSettings::default();
        apply_engine_globals(&globals, &mut restored).expect("valid settings must apply");
        assert!((restored.bgm_volume - 0.25).abs() < f32::EPSILON);
        assert!((restored.voice_volume - 0.7).abs() < f32::EPSILON);

        if let Some(Value::Map(settings)) = globals.get_mut("settings") {
            settings.insert("bgmVolume".to_string(), Value::Number(2.0));
        }
        assert!(apply_engine_globals(&globals, &mut restored).is_err());
    }

    #[test]
    fn actor_receiver_types_are_checked_across_let_bindings() {
        let error = compile_story_bytecode(
            "invalid.story.hks",
            r#"let not_actor = "text"
not_actor.at(.left)"#,
        )
        .expect_err("a string must not be accepted as an Actor receiver");
        assert!(
            error.contains("cannot call") || error.contains("receiver expects"),
            "{error}"
        );
    }

    #[test]
    fn story_compile_errors_include_rustc_style_source_context() {
        let error = compile_story_bytecode(
            "scripts/invalid.hks",
            "let count = 1\nwhile count {\n    \"never\"\n}\n",
        )
        .expect_err("a numeric condition must be rejected");
        assert!(error.contains("[HKS-COMPILE] Error: condition expects Bool, got Int"));
        assert!(error.contains("scripts/invalid.hks:2:7"));
        assert!(error.contains("while count {"));
        assert!(error.contains("use a comparison such as `value < limit`"));
    }

    #[test]
    fn story_navigation_uses_namespaced_goto_and_call() {
        let manifest = story_manifest();
        assert!(manifest.resolve_selector("story", "goto").is_some());
        assert!(manifest.resolve_selector("story", "call").is_some());
        assert!(manifest.resolve("gotoScript").is_none());
        assert!(manifest.resolve("callScript").is_none());
        assert!(manifest.resolve("loadScript").is_none());
        let bytecode = compile_story_bytecode(
            "entry.hks",
            "story.goto(\"ending.hks\", .{ reset: .presentation })\nstory.call(\"credits.hks\")",
        )
        .expect("ordinary .hks paths must compile as story scripts");
        let mut runtime = crate::script::StoryRuntime::new(bytecode)
            .expect("story navigation runtime must initialize");
        assert_eq!(
            runtime.step().expect("goto must execute"),
            Some(crate::script::StoryRuntimeEvent::Effect(
                StoryEffect::Navigate(NavigationRequest {
                    path: "ending.hks".into(),
                    kind: crate::script::navigation::NavigationKind::Goto,
                    reset: crate::script::navigation::NavigationReset::Presentation,
                    origin: None,
                })
            ))
        );
        assert_eq!(
            runtime
                .step()
                .expect("goto terminates the source execution"),
            None
        );
        let bytecode = compile_story_bytecode("entry.hks", "story.call(\"credits.hks\")")
            .expect("call compiles separately");
        let mut runtime = crate::script::StoryRuntime::new(bytecode).expect("runtime initializes");
        assert!(matches!(
            runtime.step().expect("call executes"),
            Some(crate::script::StoryRuntimeEvent::Effect(
                StoryEffect::Navigate(NavigationRequest {
                    kind: crate::script::navigation::NavigationKind::Call,
                    ..
                })
            ))
        ));
    }

    #[test]
    fn movie_is_a_blocking_story_wait() {
        let manifest = story_manifest();
        assert!(manifest.resolve("movie").is_some());
        let bytecode = compile_story_bytecode("entry.hks", "movie(\"movies/opening.webm\")")
            .expect("the movie story API must compile");
        let mut runtime =
            crate::script::StoryRuntime::new(bytecode).expect("the movie story must initialize");
        assert_eq!(
            runtime.step().expect("movie must execute"),
            Some(crate::script::StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "movies/opening.webm".into(),
                fade_out_ms: 0,
            }))
        );
    }

    #[test]
    fn engine_hooks_dialogue_sugar_without_vm_story_knowledge() {
        let bytecode = compile_story_bytecode(
            "dialogue.story.hks",
            r#"
                global let alice = char("alice")
                alice: "first"
                ...: "continued"
                "narration"
                char("alice").e("happy"): "inline"
            "#,
        )
        .expect("dialogue sugar must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("runtime must initialize");
        let mut host = StoryNativeHost::new();
        loop {
            match runtime.step().expect("runtime must advance") {
                Some(ExecutionEvent::Call { call, .. }) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
                    runtime
                        .resume(crate::script::ExecutionId::MAIN, value)
                        .expect("native result must resume VM");
                }
                Some(ExecutionEvent::Statement { value, .. }) => host
                    .handle_statement(&value)
                    .expect("statement hook must succeed"),
                Some(ExecutionEvent::Completed { .. }) => break,
                None => panic!("runtime stopped before completion"),
            }
        }

        let effects = host.drain_effects();
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, StoryEffect::ShowCharacter { .. })),
            "speaker identities must not display an actor implicitly"
        );
        let dialogue = effects
            .into_iter()
            .filter_map(|effect| match effect {
                StoryEffect::Say { speaker, text } => Some((false, speaker, text)),
                StoryEffect::ContinueDialogue { text } => Some((true, String::new(), text)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            dialogue,
            vec![
                (false, "alice".to_string(), "first".to_string()),
                (true, String::new(), "continued".to_string()),
                (false, String::new(), "narration".to_string()),
                (false, "alice".to_string(), "inline".to_string()),
            ]
        );
    }

    #[test]
    fn orphaned_continuation_falls_back_to_narration() {
        let bytecode = compile_story_bytecode("orphan.story.hks", r#"...: "orphan""#)
            .expect("orphaned continuation must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("runtime must initialize");
        let mut host = StoryNativeHost::new();
        let Some(ExecutionEvent::Call { call, .. }) = runtime.step().expect("runtime must advance")
        else {
            panic!("expected dialogue operator call")
        };
        host.call(&call)
            .expect("orphaned continuation must degrade gracefully");
        assert_eq!(
            host.drain_effects(),
            vec![StoryEffect::Say {
                speaker: String::new(),
                text: "orphan".to_string(),
            }]
        );
    }

    #[test]
    fn camera_consumes_the_shared_animation_spec() {
        compile_story_bytecode(
            "animation.hks",
            "camera().zoom(1.2).time(0.5).easing(.easeInOut)",
        )
        .expect("camera animation spec should type-check");
        compile_story_bytecode(
            "camera.hks",
            "camera().offset(800, 0, 0).viewScale(0.4).time(0.9).easing(.ease).await()",
        )
        .expect("view-scale transitions should support the shared await API");
    }

    #[test]
    fn printer_speed_is_a_validated_native_effect() {
        for (source, expected) in [
            ("printer.speed(0.05)", Some(0.05)),
            ("printer.speed(0)", None),
        ] {
            let bytecode = compile_story_bytecode("printer.hks", source).expect("speed signature");
            let mut runtime = ExecutionRuntime::new(bytecode).expect("runtime");
            let Some(ExecutionEvent::Call { call, .. }) = runtime.step().expect("call") else {
                panic!("expected native call");
            };
            let mut host = StoryNativeHost::new();
            let result = host.call(&call);
            if let Some(value) = expected {
                result.expect("valid speed");
                assert_eq!(
                    host.drain_effects(),
                    vec![StoryEffect::DialogueSpeed(value)]
                );
            } else {
                assert!(result.is_err());
            }
        }
    }
}
#[hiraku_script::hks_module("profile")]
mod profile_api {
    use super::*;
    #[hks(name = "readBool")]
    fn read_bool(_context: &mut CharacterContext, key: String) -> Result<bool, NativeError> {
        crate::storage::profile::read_bool(&key)
            .map_err(|error| NativeError::message(error.to_string()))
    }
    #[hks(name = "writeBool")]
    fn write_bool(
        _context: &mut CharacterContext,
        key: String,
        value: bool,
    ) -> Result<(), NativeError> {
        crate::storage::profile::write_bool(&key, value)
            .map_err(|error| NativeError::message(error.to_string()))
    }
}
