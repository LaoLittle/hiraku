//! Engine-owned script capabilities registered into the generic Hiraku VM.

use std::collections::BTreeMap;

use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{
    BuiltinCall, BuiltinManifest, Bytecode, ScriptType, Value, compile_with_manifest,
};
use hiraku_script::{RenderOptions, SourceMap, StatementValue, parse_program, render_diagnostics};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::script::CameraEffectScope;
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
    ReturnToTitle,
    SetBackground {
        texture: String,
    },
    GotoScript {
        path: String,
    },
    CallScript {
        path: String,
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
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoryWait {
    DialogueAdvance,
}

const ACTOR_HANDLE_TYPE: u32 = 1;
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
    registry
}

fn story_registry() -> NativeRegistry<CharacterContext> {
    let mut registry = registry();
    registry
        .register_raw_fn("openUi", async_capability_placeholder)
        .expect("built-in `openUi` registration must be unique");
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

/// Stateful native-function host for the HKS runtime.
///
/// It owns statement-scoped actor builders and exposes effects as plain data so
/// an ECS system can dispatch them without giving native functions world access.
pub struct StoryNativeHost {
    context: CharacterContext,
    registry: NativeRegistry<CharacterContext>,
}

impl Default for StoryNativeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl StoryNativeHost {
    pub fn new() -> Self {
        Self {
            context: CharacterContext::default(),
            registry: story_registry(),
        }
    }

    pub fn call(&mut self, call: &BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        self.registry
            .call(&mut self.context, call)
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
        }
    }

    pub fn restore(snapshot: StoryNativeHostSnapshot) -> Self {
        Self {
            context: CharacterContext {
                next_handle: snapshot.next_handle,
                actors: snapshot.actors,
                handles_by_name: snapshot.handles_by_name,
                commands: Vec::new(),
                wait: None,
                last_speaker: snapshot.last_speaker,
                dialogue_buffer: snapshot.dialogue_buffer,
            },
            registry: story_registry(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoryNativeHostSnapshot {
    next_handle: u64,
    actors: BTreeMap<u64, PendingActor>,
    handles_by_name: BTreeMap<String, u64>,
    last_speaker: Option<String>,
    dialogue_buffer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingActor {
    name: String,
    expressions: Vec<String>,
    position: [f32; 2],
    scale: f32,
    dirty: bool,
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
        Ok(())
    }
}

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Actor", handle_type = ACTOR_HANDLE_TYPE)]
struct ActorHandle(u64);

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
    fn native_return_to_title(context: &mut CharacterContext) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::ReturnToTitle);
        Ok(())
    }

    #[hks]
    fn native_bg(context: &mut CharacterContext, texture: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::SetBackground { texture });
        Ok(())
    }

    #[hks]
    fn native_goto_script(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::GotoScript { path });
        Ok(())
    }

    #[hks]
    fn native_call_script(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::CallScript { path });
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

    #[hks]
    fn native_play_bgm(
        context: &mut CharacterContext,
        path: String,
        volume: f64,
        fade_ms: f64,
    ) -> Result<(), NativeError> {
        if !(0.0..=1.0).contains(&volume) || fade_ms < 0.0 {
            return Err(NativeError::message(
                "playBgm volume must be between 0 and 1 and fade must be non-negative",
            ));
        }
        context.commands.push(StoryEffect::PlayBgm {
            path,
            volume: volume as f32,
            fade_in_ms: Some(fade_ms.round() as u64),
        });
        Ok(())
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
        Ok(Value::Null)
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

    #[hks(raw, selector = "camera", name = "blur")]
    fn native_camera_blur(
        context: &mut CharacterContext,
        call: &hiraku_script::BuiltinCall,
    ) -> Result<Value, NativeError> {
        require_selector(call, "camera")?;
        let mut intensity = None;
        let mut duration = 0.0;
        let mut ease = "linear".to_string();
        let mut scope = CameraEffectScope::World;
        for argument in &call.arguments {
            match argument.label.as_deref() {
                None if intensity.is_none() => intensity = Some(number_value(&argument.value)?),
                Some("duration") => duration = number_value(&argument.value)?,
                Some("ease") => ease = symbol_value(&argument.value)?,
                Some("scope") => scope = camera_scope_value(&argument.value)?,
                _ => return Err(NativeError::message("invalid camera.blur arguments")),
            }
        }
        let intensity =
            intensity.ok_or_else(|| NativeError::message("blur intensity is required"))?;
        if intensity < 0.0 || duration < 0.0 {
            return Err(NativeError::message(
                "blur intensity and duration must be non-negative",
            ));
        }
        context.commands.push(StoryEffect::SetCamera {
            blur: Some(intensity as f32),
            zoom: None,
            scope,
            duration_ms: (duration * 1000.0).round() as u64,
            ease: normalize_ease(&ease)?,
        });
        Ok(Value::Null)
    }

    #[hks(raw, selector = "camera", name = "zoom")]
    fn native_camera_zoom(
        context: &mut CharacterContext,
        call: &hiraku_script::BuiltinCall,
    ) -> Result<Value, NativeError> {
        require_selector(call, "camera")?;
        let mut scale = None;
        let mut duration = 0.0;
        let mut ease = "linear".to_string();
        let mut scope = CameraEffectScope::World;
        for argument in &call.arguments {
            match argument.label.as_deref() {
                None if scale.is_none() => scale = Some(number_value(&argument.value)?),
                Some("duration") => duration = number_value(&argument.value)?,
                Some("ease") => ease = symbol_value(&argument.value)?,
                Some("scope") => scope = camera_scope_value(&argument.value)?,
                Some("at") if matches!(argument.value, Value::Symbol(ref value) if value == "center") =>
                    {}
                _ => return Err(NativeError::message("invalid camera.zoom arguments")),
            }
        }
        let scale = scale.ok_or_else(|| NativeError::message("zoom scale is required"))?;
        if scale <= 0.0 || duration < 0.0 {
            return Err(NativeError::message(
                "zoom scale must be positive and duration non-negative",
            ));
        }
        context.commands.push(StoryEffect::SetCamera {
            blur: None,
            zoom: Some(scale as f32),
            scope,
            duration_ms: (duration * 1000.0).round() as u64,
            ease: normalize_ease(&ease)?,
        });
        Ok(Value::Null)
    }
}

fn require_selector(call: &hiraku_script::BuiltinCall, expected: &str) -> Result<(), NativeError> {
    match &call.receiver {
        Some(Value::Selector(selector)) if selector == expected => Ok(()),
        Some(_) => Err(NativeError::message(format!(
            "expected `{expected}` selector receiver"
        ))),
        None => Err(NativeError::message(format!(
            "selector method requires `{expected}` receiver"
        ))),
    }
}

fn number_value(value: &Value) -> Result<f64, NativeError> {
    match value {
        Value::Number(value) => Ok(*value),
        _ => Err(NativeError::TypeMismatch("number")),
    }
}

fn symbol_value(value: &Value) -> Result<String, NativeError> {
    match value {
        Value::Symbol(value) => Ok(value.clone()),
        _ => Err(NativeError::TypeMismatch("symbol")),
    }
}

fn camera_scope_value(value: &Value) -> Result<CameraEffectScope, NativeError> {
    match value {
        Value::Symbol(value) if value == "world" => Ok(CameraEffectScope::World),
        Value::Symbol(value) if value == "canvas" => Ok(CameraEffectScope::Canvas),
        Value::Symbol(_) => Err(NativeError::message(
            "camera scope must be .world or .canvas",
        )),
        _ => Err(NativeError::TypeMismatch("symbol")),
    }
}

fn normalize_ease(ease: &str) -> Result<String, NativeError> {
    match ease {
        "linear" => Ok("linear".to_string()),
        "easeIn" => Ok("ease_in".to_string()),
        "easeOut" => Ok("ease_out".to_string()),
        "easeInOut" => Ok("ease_in_out".to_string()),
        _ => Err(NativeError::message(format!("unsupported easing `{ease}`"))),
    }
}

fn pending_actor(name: &str) -> PendingActor {
    PendingActor {
        name: name.to_string(),
        expressions: Vec::new(),
        position: [0.0, 0.0],
        scale: 1.0,
        dirty: true,
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
    fn script_transfer_api_uses_explicit_goto_and_call_names() {
        let manifest = story_manifest();
        assert!(manifest.resolve("gotoScript").is_some());
        assert!(manifest.resolve("callScript").is_some());
        assert!(manifest.resolve("loadScript").is_none());
        compile_story_bytecode(
            "entry.hks",
            "callScript(\"chapter.hks\")\ngotoScript(\"ending.hks\")",
        )
        .expect("ordinary .hks paths must compile as story scripts");
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
                    let value = host.call(&call).expect("native call must succeed");
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
}
