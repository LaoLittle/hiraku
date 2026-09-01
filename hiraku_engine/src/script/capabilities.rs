//! Engine-owned script capabilities registered into the generic Hiraku VM.

use std::collections::BTreeMap;

use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{
    BuiltinCall, BuiltinId, BuiltinManifest, Bytecode, ScriptType, Value, compile_with_manifest,
};
use hiraku_script::{RenderOptions, SourceMap, StatementValue, parse_program, render_diagnostics};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::script::animation::{AnimationSpec, register_animation_api};
use crate::script::navigation::{NavigationHandle, NavigationRequest, NavigationResetValue};
use crate::script::{CameraEffectScope, CameraProjectionMode};
use crate::storage::UserSettings;

/// Engine-facing effects produced by HKS native functions.
///
/// Engine code dispatches these effects directly to ECS-facing systems.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoryEffect {
    Log(String),
    ClearDialogue,
    StopBgm,
    Exit,
    SetBackground {
        texture: String,
    },
    Navigate(NavigationRequest),
    SetUiRole {
        role: String,
        component: String,
    },
    MountUiOverlay {
        name: String,
        component: String,
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
    SetCamera {
        blur: Option<f32>,
        zoom: Option<f32>,
        offset: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        projection: Option<CameraProjectionMode>,
        scope: CameraEffectScope,
        duration_ms: u64,
        ease: String,
    },
    ShowCharacter {
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: [f32; 2],
        scale: f32,
        focused: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoryWait {
    DialogueAdvance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoryTaskKind {
    Sequence,
    Parallel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoryControl {
    SpawnTask { kind: StoryTaskKind, closure: Value },
    BeginChoice { prompt: String, closure: Value },
    AddChoiceOption { label: String, closure: Value },
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
const BGM_HANDLE_TYPE: u32 = 2;
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
    compile_with_manifest(&program, source_hash(path, source), &story_manifest()).map_err(
        |errors| {
            let diagnostics = errors
                .into_iter()
                .map(|error| error.diagnostic(source_id.clone()))
                .collect::<Vec<_>>();
            render_diagnostics(&diagnostics, &sources, render_options)
        },
    )
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

fn source_hash(path: &str, source: &str) -> u64 {
    path.bytes()
        .chain([0])
        .chain(source.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
        })
}

fn registry() -> NativeRegistry<CharacterContext> {
    let mut registry = NativeRegistry::new();
    Position::register_hks(&mut registry)
        .expect("Position API registration must be internally consistent");
    CameraScope::register_hks(&mut registry)
        .expect("CameraScope API registration must be internally consistent");
    CameraEase::register_hks(&mut registry)
        .expect("CameraEase API registration must be internally consistent");
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
                ("bgmVolume".to_string(), ScriptType::Number),
                ("voiceVolume".to_string(), ScriptType::Number),
                ("sfxVolume".to_string(), ScriptType::Number),
            ])),
        )
        .expect("engine settings schema must be defined once");
    native_api::register_hks(&mut registry)
        .expect("story native API registration must be internally consistent");
    story_api::register_hks(&mut registry)
        .expect("story navigation API registration must be internally consistent");
    registry
}

fn story_registry() -> NativeRegistry<CharacterContext> {
    let mut registry = registry();
    ui_api::register_hks(&mut registry)
        .expect("story UI API registration must be internally consistent");
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
    registry
        .set_signature(
            option,
            hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String, ScriptType::Function],
                variadic: None,
                result: ScriptType::Unit,
            },
        )
        .expect("option signature must target its registered builtin");
    registry
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
        context
            .commands
            .push(StoryEffect::MountUiOverlay { name, component });
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
    sequence: BuiltinId,
    parallel: BuiltinId,
    choice: BuiltinId,
    option: BuiltinId,
    open_ui: BuiltinId,
    wait: BuiltinId,
}

impl StoryControlBuiltins {
    fn new(manifest: &BuiltinManifest) -> Self {
        Self {
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
        }
    }
}

impl Default for StoryNativeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl StoryNativeHost {
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
        if call.builtin == self.controls.open_ui {
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
        if call.builtin == self.controls.wait {
            let task = call
                .arguments
                .first()
                .and_then(|argument| match &argument.value {
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

    pub fn snapshot(&self) -> StoryNativeHostSnapshot {
        StoryNativeHostSnapshot {
            next_handle: self.context.next_handle,
            actors: self.context.actors.clone(),
            handles_by_name: self.context.handles_by_name.clone(),
            last_speaker: self.context.last_speaker.clone(),
            dialogue_buffer: self.context.dialogue_buffer.clone(),
            next_bgm_handle: self.context.next_bgm_handle,
            pending_bgm: self.context.pending_bgm.clone(),
            next_camera_handle: self.context.next_camera_handle,
            pending_cameras: self.context.pending_cameras.clone(),
            next_navigation_handle: self.context.next_navigation_handle,
            pending_navigations: self.context.pending_navigations.clone(),
        }
    }

    pub fn restore(snapshot: StoryNativeHostSnapshot) -> Self {
        let registry = story_registry();
        let controls = StoryControlBuiltins::new(&registry.manifest());
        Self {
            context: CharacterContext {
                next_handle: snapshot.next_handle,
                actors: snapshot.actors,
                handles_by_name: snapshot.handles_by_name,
                commands: Vec::new(),
                wait: None,
                last_speaker: snapshot.last_speaker,
                dialogue_buffer: snapshot.dialogue_buffer,
                next_bgm_handle: snapshot.next_bgm_handle,
                pending_bgm: snapshot.pending_bgm,
                next_camera_handle: snapshot.next_camera_handle,
                pending_cameras: snapshot.pending_cameras,
                next_navigation_handle: snapshot.next_navigation_handle,
                pending_navigations: snapshot.pending_navigations,
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
    next_handle: u64,
    actors: BTreeMap<u64, PendingActor>,
    handles_by_name: BTreeMap<String, u64>,
    last_speaker: Option<String>,
    dialogue_buffer: Option<String>,
    next_bgm_handle: u64,
    pending_bgm: BTreeMap<u64, PendingBgm>,
    next_camera_handle: u64,
    pending_cameras: BTreeMap<u64, PendingCamera>,
    #[serde(default)]
    next_navigation_handle: u64,
    #[serde(default)]
    pending_navigations: BTreeMap<u64, NavigationRequest>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingActor {
    name: String,
    expressions: Vec<String>,
    position: [f32; 2],
    scale: f32,
    dirty: bool,
    focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingBgm {
    path: String,
    volume: f32,
    fade_in_ms: Option<u64>,
    dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingCamera {
    blur: Option<f32>,
    zoom: Option<f32>,
    offset: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    projection: Option<CameraProjectionMode>,
    scope: CameraEffectScope,
    duration_ms: u64,
    ease: String,
}

#[derive(Default)]
struct CharacterContext {
    next_handle: u64,
    actors: BTreeMap<u64, PendingActor>,
    handles_by_name: BTreeMap<String, u64>,
    commands: Vec<StoryEffect>,
    wait: Option<StoryWait>,
    last_speaker: Option<String>,
    dialogue_buffer: Option<String>,
    next_bgm_handle: u64,
    pending_bgm: BTreeMap<u64, PendingBgm>,
    next_camera_handle: u64,
    pending_cameras: BTreeMap<u64, PendingCamera>,
    next_navigation_handle: u64,
    pending_navigations: BTreeMap<u64, NavigationRequest>,
}

impl CharacterContext {
    fn handle_statement(
        &mut self,
        statement: &StatementValue,
    ) -> Result<(), CharacterCapabilityError> {
        self.commit()?;
        if let StatementValue::String(text) = statement {
            native_api::native_narrate(self, text.clone())
                .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
        }
        Ok(())
    }

    fn char(&mut self, name: String) -> Result<ActorHandle, CharacterCapabilityError> {
        if let Some(handle) = self.handles_by_name.get(&name).copied() {
            self.flush(handle)?;
            self.actors.insert(handle, pending_actor(&name));
            return Ok(ActorHandle(handle));
        }
        self.next_handle += 1;
        let handle = self.next_handle;
        self.handles_by_name.insert(name.clone(), handle);
        self.actors.insert(handle, pending_actor(&name));
        Ok(ActorHandle(handle))
    }

    fn emotion(
        &mut self,
        ActorHandle(handle): ActorHandle,
        emotion: String,
    ) -> Result<ActorHandle, CharacterCapabilityError> {
        let pending = self.actor_mut(handle)?;
        pending.expressions.push(emotion);
        pending.dirty = true;
        Ok(ActorHandle(handle))
    }

    fn at(
        &mut self,
        ActorHandle(handle): ActorHandle,
        position: Position,
    ) -> Result<ActorHandle, CharacterCapabilityError> {
        self.actor_mut(handle)?.position = position.resolve();
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorHandle(handle))
    }

    fn scale(
        &mut self,
        ActorHandle(handle): ActorHandle,
        scale: f64,
    ) -> Result<ActorHandle, CharacterCapabilityError> {
        if scale <= 0.0 {
            return Err(CharacterCapabilityError::InvalidArguments(
                "scale must be positive",
            ));
        }
        self.actor_mut(handle)?.scale = scale as f32;
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorHandle(handle))
    }

    fn focus(
        &mut self,
        ActorHandle(handle): ActorHandle,
        focused: bool,
    ) -> Result<ActorHandle, CharacterCapabilityError> {
        self.actor_mut(handle)?.focused = focused;
        self.actor_mut(handle)?.dirty = true;
        Ok(ActorHandle(handle))
    }

    fn bgm(&mut self, path: String) -> Result<BgmHandle, NativeError> {
        if path.trim().is_empty() {
            return Err(NativeError::message("bgm path must not be empty"));
        }
        self.next_bgm_handle += 1;
        let handle = self.next_bgm_handle;
        self.pending_bgm.insert(
            handle,
            PendingBgm {
                path,
                volume: 1.0,
                fade_in_ms: None,
                dirty: true,
            },
        );
        Ok(BgmHandle(handle))
    }

    fn bgm_mut(&mut self, handle: u64) -> Result<&mut PendingBgm, NativeError> {
        self.pending_bgm
            .get_mut(&handle)
            .ok_or_else(|| NativeError::message(format!("unknown bgm handle {handle}")))
    }

    fn camera(&mut self, scope: CameraScope) -> CameraHandle {
        self.next_camera_handle += 1;
        let handle = self.next_camera_handle;
        self.pending_cameras.insert(
            handle,
            PendingCamera {
                blur: None,
                zoom: None,
                offset: None,
                rotation: None,
                projection: None,
                scope: match scope {
                    CameraScope::Scene => CameraEffectScope::World,
                    CameraScope::Canvas => CameraEffectScope::Canvas,
                },
                duration_ms: 0,
                ease: "linear".to_string(),
            },
        );
        CameraHandle(handle)
    }

    fn camera_mut(&mut self, handle: u64) -> Result<&mut PendingCamera, NativeError> {
        self.pending_cameras
            .get_mut(&handle)
            .ok_or_else(|| NativeError::message(format!("unknown camera handle {handle}")))
    }

    fn goto(&mut self, path: String) -> Result<NavigationHandle, NativeError> {
        self.next_navigation_handle += 1;
        let handle = self.next_navigation_handle;
        self.pending_navigations
            .insert(handle, NavigationRequest::goto(path)?);
        Ok(NavigationHandle(handle))
    }

    fn reset_navigation(
        &mut self,
        NavigationHandle(handle): NavigationHandle,
        reset: NavigationResetValue,
    ) -> Result<NavigationHandle, NativeError> {
        let navigation = self
            .pending_navigations
            .get_mut(&handle)
            .ok_or_else(|| NativeError::message(format!("unknown Navigation handle {handle}")))?;
        navigation.reset = reset.into();
        Ok(NavigationHandle(handle))
    }

    fn actor_mut(&mut self, handle: u64) -> Result<&mut PendingActor, CharacterCapabilityError> {
        self.actors
            .get_mut(&handle)
            .ok_or(CharacterCapabilityError::UnknownActor(handle))
    }

    fn flush(&mut self, handle: u64) -> Result<(), CharacterCapabilityError> {
        let command = {
            let pending = self.actor_mut(handle)?;
            if !pending.dirty {
                return Ok(());
            }
            pending.dirty = false;
            StoryEffect::ShowCharacter {
                actor_id: pending.name.clone(),
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
        let handles = self.actors.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            self.flush(handle)?;
        }
        let bgm = std::mem::take(&mut self.pending_bgm);
        for (_, pending) in bgm {
            if pending.dirty {
                self.commands.push(StoryEffect::PlayBgm {
                    path: pending.path,
                    volume: pending.volume,
                    fade_in_ms: pending.fade_in_ms,
                });
            }
        }
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
                    offset: pending.offset,
                    rotation: pending.rotation,
                    projection: pending.projection,
                    scope: pending.scope,
                    duration_ms: pending.duration_ms,
                    ease: pending.ease,
                });
            }
        }
        let navigations = std::mem::take(&mut self.pending_navigations);
        self.commands
            .extend(navigations.into_values().map(StoryEffect::Navigate));
        Ok(())
    }
}

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Actor", handle_type = ACTOR_HANDLE_TYPE)]
struct ActorHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Bgm", handle_type = BGM_HANDLE_TYPE)]
struct BgmHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Camera", handle_type = CAMERA_HANDLE_TYPE)]
struct CameraHandle(u64);

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum Position {
    Absolute(f64, f64),
    Relative(u16, u16),
}

impl Position {
    fn pos(x: f64, y: f64) -> Position {
        Self::Absolute(x, y)
    }

    fn rel(x: f64, y: f64) -> Result<Position, NativeError> {
        fn component(value: f64) -> Result<u16, NativeError> {
            if !value.is_finite() || value.fract() != 0.0 || !(0.0..=100.0).contains(&value) {
                return Err(NativeError::message(
                    "relative position components must be integers from 0 through 100",
                ));
            }
            Ok(value as u16)
        }
        Ok(Self::Relative(component(x)?, component(y)?))
    }

    #[getter]
    fn left() -> Position {
        Self::Absolute(-600.0, -200.0)
    }

    #[getter]
    fn center() -> Position {
        Self::Absolute(0.0, -200.0)
    }

    #[getter]
    fn right() -> Position {
        Self::Absolute(600.0, -200.0)
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
enum CameraEase {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
}

#[allow(non_snake_case)]
impl CameraEase {
    #[getter]
    fn linear() -> CameraEase { Self::Linear }
    #[getter]
    fn ease() -> CameraEase { Self::Ease }
    #[getter]
    fn easeIn() -> CameraEase { Self::EaseIn }
    #[getter]
    fn easeOut() -> CameraEase { Self::EaseOut }
    #[getter]
    fn easeInOut() -> CameraEase { Self::EaseInOut }
    #[getter]
    fn bounce() -> CameraEase { Self::Bounce }
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
            Self::Absolute(x, y) => [x as f32, y as f32],
            // Relative coordinates use a bottom-left origin in the canonical 1920x1080 canvas.
            Self::Relative(x, y) => [
                f32::from(x) / 100.0 * 1920.0 - 960.0,
                f32::from(y) / 100.0 * 1080.0 - 540.0,
            ],
        }
    }
}

#[hiraku_script::hks_module]
mod native_api {
    use super::*;

    #[hks(name = "char")]
    fn native_char(
        context: &mut CharacterContext,
        name: String,
    ) -> Result<ActorHandle, NativeError> {
        context
            .char(name)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks(name = "e", receiver)]
    fn native_emotion(
        context: &mut CharacterContext,
        actor: ActorHandle,
        emotion: String,
    ) -> Result<ActorHandle, NativeError> {
        context
            .emotion(actor, emotion)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks(name = "at", receiver)]
    fn native_at(
        context: &mut CharacterContext,
        actor: ActorHandle,
        position: Position,
    ) -> Result<ActorHandle, NativeError> {
        context
            .at(actor, position)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks(name = "scale", receiver)]
    fn native_scale(
        context: &mut CharacterContext,
        actor: ActorHandle,
        scale: f64,
    ) -> Result<ActorHandle, NativeError> {
        context
            .scale(actor, scale)
            .map_err(|error| NativeError::message(error.to_string()))
    }

    #[hks(name = "focus", receiver)]
    fn native_focus(
        context: &mut CharacterContext,
        actor: ActorHandle,
        focused: Option<bool>,
    ) -> Result<ActorHandle, NativeError> {
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
    fn native_stop_bgm(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::StopBgm);
        Ok(())
    }

    #[hks]
    fn native_exit(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::Exit);
        Ok(())
    }

    #[hks]
    fn native_bg(context: &mut CharacterContext, texture: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::SetBackground { texture });
        Ok(())
    }

    #[hks(name = "reset", receiver)]
    fn native_navigation_reset(
        context: &mut CharacterContext,
        navigation: NavigationHandle,
        reset: NavigationResetValue,
    ) -> Result<NavigationHandle, NativeError> {
        context.reset_navigation(navigation, reset)
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

    #[hks(name = "bgm")]
    fn native_bgm(context: &mut CharacterContext, path: String) -> Result<BgmHandle, NativeError> {
        context.bgm(path)
    }

    #[hks(name = "volume", receiver)]
    fn native_bgm_volume(
        context: &mut CharacterContext,
        BgmHandle(handle): BgmHandle,
        volume: f64,
    ) -> Result<BgmHandle, NativeError> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(NativeError::message("bgm volume must be between 0 and 1"));
        }
        context.bgm_mut(handle)?.volume = volume as f32;
        Ok(BgmHandle(handle))
    }

    #[hks(name = "fadeIn", receiver)]
    fn native_bgm_fade_in(
        context: &mut CharacterContext,
        BgmHandle(handle): BgmHandle,
        fade_ms: f64,
    ) -> Result<BgmHandle, NativeError> {
        if !fade_ms.is_finite() || fade_ms < 0.0 {
            return Err(NativeError::message(
                "bgm fade-in duration must be a non-negative number of milliseconds",
            ));
        }
        context.bgm_mut(handle)?.fade_in_ms = Some(fade_ms.round() as u64);
        Ok(BgmHandle(handle))
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
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(NativeError::message(
                "camera animation time must be non-negative",
            ));
        }
        context.camera_mut(handle)?.duration_ms = (seconds * 1000.0).round() as u64;
        Ok(CameraHandle(handle))
    }

    #[hks(name = "easing", receiver)]
    fn native_camera_easing(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        easing: CameraEase,
    ) -> Result<CameraHandle, NativeError> {
        context.camera_mut(handle)?.ease = match easing {
            CameraEase::Linear => "linear",
            CameraEase::Ease => "ease",
            CameraEase::EaseIn => "easeIn",
            CameraEase::EaseOut => "easeOut",
            CameraEase::EaseInOut => "easeInOut",
            CameraEase::Bounce => "bounce",
        }
        .to_string();
        Ok(CameraHandle(handle))
    }

    #[hks(name = "animation", receiver)]
    fn native_camera_animation(
        context: &mut CharacterContext,
        CameraHandle(handle): CameraHandle,
        animation: AnimationSpec,
    ) -> Result<CameraHandle, NativeError> {
        if animation.repeats() {
            return Err(NativeError::message(
                "camera command animations must complete; repeatForever is only valid for persistent timelines",
            ));
        }
        let pending = context.camera_mut(handle)?;
        pending.duration_ms = (animation.duration() * 1000.0).round() as u64;
        pending.ease = match animation {
            AnimationSpec::Linear(..) => "linear",
            AnimationSpec::EaseIn(..) => "easeIn",
            AnimationSpec::EaseOut(..) => "easeOut",
            AnimationSpec::EaseInOut(..) => "easeInOut",
        }
        .to_string();
        Ok(CameraHandle(handle))
    }

    #[hks]
    pub(super) fn native_narrate(
        context: &mut CharacterContext,
        text: String,
    ) -> Result<(), NativeError> {
        context.last_speaker = Some(String::new());
        context.dialogue_buffer = Some(text.clone());
        context.commands.push(StoryEffect::Say {
            speaker: String::new(),
            text,
        });
        context.wait = Some(StoryWait::DialogueAdvance);
        Ok(())
    }

    #[hks]
    fn native_say(
        context: &mut CharacterContext,
        speaker: String,
        text: String,
    ) -> Result<(), NativeError> {
        context.last_speaker = Some(speaker.clone());
        context.dialogue_buffer = Some(text.clone());
        context.commands.push(StoryEffect::Say { speaker, text });
        context.wait = Some(StoryWait::DialogueAdvance);
        Ok(())
    }

    #[hks(raw, operator = ":")]
    fn native_dialogue_operator(
        context: &mut CharacterContext,
        call: &BuiltinCall,
    ) -> Result<Value, NativeError> {
        if call.receiver.is_some() || call.arguments.len() != 2 {
            return Err(NativeError::message("operator `:` expects two operands"));
        }
        let continuation = matches!(call.arguments[0].value, Value::Ellipsis);
        let speaker = match &call.arguments[0].value {
            Value::Handle {
                type_id: ACTOR_HANDLE_TYPE,
                id,
            } => context
                .actors
                .get(id)
                .map(|actor| actor.name.clone())
                .ok_or_else(|| NativeError::message(format!("unknown actor handle {id}")))?,
            Value::Ellipsis => context.last_speaker.clone().unwrap_or_default(),
            _ => return Err(NativeError::TypeMismatch("actor or ellipsis")),
        };
        let Value::String(text) = &call.arguments[1].value else {
            return Err(NativeError::TypeMismatch("string"));
        };
        if continuation {
            if let Some(buffer) = context.dialogue_buffer.as_mut() {
                buffer.push_str(text);
                context
                    .commands
                    .push(StoryEffect::ContinueDialogue { text: text.clone() });
                context.wait = Some(StoryWait::DialogueAdvance);
            } else {
                bevy::log::warn!("`...` has no dialogue buffer; treating it as narration");
                native_narrate(context, text.clone())?;
            }
        } else {
            native_say(context, speaker, text.clone())?;
        }
        Ok(Value::Unit)
    }

    #[hks]
    fn native_voice(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
        if path.trim().is_empty() {
            return Err(NativeError::message("voice path must not be empty"));
        }
        context
            .commands
            .push(StoryEffect::PlayVoice { path, volume: 1.0 });
        Ok(())
    }
}

#[hiraku_script::hks_module("story")]
mod story_api {
    use super::*;

    #[hks(name = "goto")]
    fn native_goto_story(
        context: &mut CharacterContext,
        path: String,
    ) -> Result<NavigationHandle, NativeError> {
        context.goto(path)
    }

    #[hks(name = "call")]
    fn native_call_story(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Navigate(NavigationRequest::call(path)?));
        Ok(())
    }
}

fn pending_actor(name: &str) -> PendingActor {
    PendingActor {
        name: name.to_string(),
        expressions: Vec::new(),
        position: [0.0, 0.0],
        scale: 1.0,
        dirty: true,
        focused: false,
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
    use crate::script::hks_runtime::{HksRuntime, HksRuntimeEvent};

    #[test]
    fn engine_settings_roundtrip_through_the_fixed_global_record() {
        let original = UserSettings {
            bgm_volume: 0.8,
            voice_volume: 0.7,
            sfx_volume: 0.6,
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
        assert!(error.contains("receiver expects Named"));
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
            "story.goto(\"ending.hks\").reset(.presentation)\nstory.call(\"credits.hks\")",
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
            runtime.step().expect("call must execute"),
            Some(crate::script::StoryRuntimeEvent::Effect(
                StoryEffect::Navigate(NavigationRequest {
                    path: "credits.hks".into(),
                    kind: crate::script::navigation::NavigationKind::Call,
                    reset: crate::script::navigation::NavigationReset::None,
                    origin: None,
                })
            ))
        );
    }

    #[test]
    fn engine_hooks_dialogue_sugar_without_vm_story_knowledge() {
        let bytecode = compile_story_bytecode(
            "dialogue.story.hks",
            r#"
                let alice = char("alice")
                alice: "first"
                ...: "continued"
                "narration"
                char("alice").e("happy"): "inline"
            "#,
        )
        .expect("dialogue sugar must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("runtime must initialize");
        let mut host = StoryNativeHost::new();
        loop {
            match runtime.step().expect("runtime must advance") {
                Some(HksRuntimeEvent::Call(call)) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
                    runtime
                        .resume_main(value)
                        .expect("native result must resume VM");
                }
                Some(HksRuntimeEvent::Statement(value)) => host
                    .handle_statement(&value)
                    .expect("statement hook must succeed"),
                Some(HksRuntimeEvent::Completed(_)) => break,
                Some(event) => panic!("unexpected runtime event: {event:?}"),
                None => panic!("runtime stopped before completion"),
            }
        }

        let dialogue = host
            .drain_effects()
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
        let mut runtime = HksRuntime::new(bytecode).expect("runtime must initialize");
        let mut host = StoryNativeHost::new();
        let Some(HksRuntimeEvent::Call(call)) = runtime.step().expect("runtime must advance")
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
            "camera().zoom(1.2).animation(.easeInOut(0.5))",
        )
        .expect("camera animation spec should type-check");
    }
}
