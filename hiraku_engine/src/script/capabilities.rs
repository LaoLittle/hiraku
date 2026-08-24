//! Engine-owned script capabilities registered into the generic Hiraku VM.

use std::collections::BTreeMap;

use hiraku_script::native::{FromHksValue, IntoHksValue, NativeError, NativeRegistry};
use hiraku_script::vm::{
    BuiltinCall, BuiltinId, BuiltinManifest, Bytecode, FunctionSignature, ScriptType,
    StaticMemberKind, TaskEvent, TaskMode, TaskScheduler, TaskStatus, Value, Vm, VmEvent,
    compile_with_manifest,
};
use hiraku_script::{Expr, Program, StatementValue, Stmt, parse_program};
use thiserror::Error;

use crate::script::CameraEffectScope;

/// Engine-facing effects produced by HKS native functions.
///
/// This is deliberately independent of the transitional story IR. Engine code
/// dispatches these effects directly to ECS-facing script commands.
#[derive(Clone, Debug, PartialEq)]
pub enum StoryEffect {
    Log(String),
    ClearDialogue,
    StopBgm,
    Exit,
    ReturnToTitle,
    SetBackground {
        texture: String,
    },
    LoadScript {
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
const CHAR: BuiltinId = BuiltinId(1);
const EMOTION: BuiltinId = BuiltinId(2);
const AT: BuiltinId = BuiltinId(3);
const SCALE: BuiltinId = BuiltinId(4);
const LOG: BuiltinId = BuiltinId(10);
const CLEAR_TEXT: BuiltinId = BuiltinId(11);
const STOP_BGM: BuiltinId = BuiltinId(12);
const EXIT: BuiltinId = BuiltinId(13);
const RETURN_TO_TITLE: BuiltinId = BuiltinId(14);
const BG: BuiltinId = BuiltinId(15);
const LOAD_SCRIPT: BuiltinId = BuiltinId(16);
const ADJUST_SETTING: BuiltinId = BuiltinId(17);
const PLAY_BGM: BuiltinId = BuiltinId(18);
const NARRATE: BuiltinId = BuiltinId(19);
const CAMERA_BLUR: BuiltinId = BuiltinId(20);
const CAMERA_ZOOM: BuiltinId = BuiltinId(21);
const VOICE: BuiltinId = BuiltinId(22);
const SAY: BuiltinId = BuiltinId(23);
pub const OPEN_UI: BuiltinId = BuiltinId(24);
pub const WAIT: BuiltinId = BuiltinId(25);
const DIALOGUE_OPERATOR: BuiltinId = BuiltinId(26);
const POSITION_ABSOLUTE: BuiltinId = BuiltinId(27);
const POSITION_RELATIVE: BuiltinId = BuiltinId(28);
const POSITION_LEFT: BuiltinId = BuiltinId(29);
const POSITION_CENTER: BuiltinId = BuiltinId(30);
const POSITION_RIGHT: BuiltinId = BuiltinId(31);

pub fn manifest() -> BuiltinManifest {
    registry().manifest()
}

/// Manifest used by the direct whole-story HKS runtime. Async capabilities are
/// registered here so the generic compiler can resolve them without engine AST lowering.
pub fn story_manifest() -> BuiltinManifest {
    story_registry().manifest()
}

pub fn compile_story_bytecode(path: &str, source: &str) -> Result<Bytecode, String> {
    let program = parse_program(source).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| format!("{} at byte {}", error.message, error.span.start))
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    compile_with_manifest(&program, source_hash(path, source), &story_manifest()).map_err(
        |errors| {
            errors
                .into_iter()
                .map(|error| format!("{} at byte {}", error.message, error.span.start))
                .collect::<Vec<_>>()
                .join("; ")
        },
    )
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
    let actor_type = registry.define_type("Actor");
    let position_type = registry.define_type("Position");
    registry
        .register_fn_with_id(CHAR, "char", native_char)
        .expect("built-in `char` registration must be unique");
    registry
        .register_fn_with_id(EMOTION, "e", native_emotion)
        .expect("built-in `e` registration must be unique");
    registry
        .register_fn_with_id(AT, "at", native_at)
        .expect("built-in `at` registration must be unique");
    registry
        .register_fn_with_id(SCALE, "scale", native_scale)
        .expect("built-in `scale` registration must be unique");
    registry
        .register_fn_with_id(LOG, "log", native_log)
        .expect("built-in `log` registration must be unique");
    registry
        .register_fn_with_id(CLEAR_TEXT, "clearText", native_clear_text)
        .expect("built-in `clearText` registration must be unique");
    registry
        .register_fn_with_id(STOP_BGM, "stopBgm", native_stop_bgm)
        .expect("built-in `stopBgm` registration must be unique");
    registry
        .register_fn_with_id(EXIT, "exit", native_exit)
        .expect("built-in `exit` registration must be unique");
    registry
        .register_fn_with_id(RETURN_TO_TITLE, "returnToTitle", native_return_to_title)
        .expect("built-in `returnToTitle` registration must be unique");
    registry
        .register_fn_with_id(BG, "bg", native_bg)
        .expect("built-in `bg` registration must be unique");
    registry
        .register_fn_with_id(LOAD_SCRIPT, "loadScript", native_load_script)
        .expect("built-in `loadScript` registration must be unique");
    registry
        .register_fn_with_id(ADJUST_SETTING, "adjustSetting", native_adjust_setting)
        .expect("built-in `adjustSetting` registration must be unique");
    registry
        .register_fn_with_id(PLAY_BGM, "playBgm", native_play_bgm)
        .expect("built-in `playBgm` registration must be unique");
    registry
        .register_fn_with_id(NARRATE, "narrate", native_narrate)
        .expect("built-in `narrate` registration must be unique");
    registry
        .register_fn_with_id(VOICE, "voice", native_voice)
        .expect("built-in `voice` registration must be unique");
    registry
        .register_fn_with_id(SAY, "say", native_say)
        .expect("built-in `say` registration must be unique");
    registry
        .register_operator_raw_fn_with_id(DIALOGUE_OPERATOR, ":", native_dialogue_operator)
        .expect("built-in `:` operator registration must be unique");
    registry
        .register_selector_raw_fn_with_id(CAMERA_BLUR, "camera", "blur", native_camera_blur)
        .expect("built-in `camera.blur` registration must be unique");
    registry
        .register_selector_raw_fn_with_id(CAMERA_ZOOM, "camera", "zoom", native_camera_zoom)
        .expect("built-in `camera.zoom` registration must be unique");
    registry
        .set_signature(
            CHAR,
            FunctionSignature {
                receiver: None,
                parameters: vec![ScriptType::String],
                result: ScriptType::Named(actor_type),
            },
        )
        .expect("char signature must target a registered builtin");
    for (builtin, parameters) in [
        (EMOTION, vec![ScriptType::String]),
        (
            AT,
            vec![ScriptType::Union(vec![
                ScriptType::String,
                ScriptType::Named(position_type),
            ])],
        ),
        (SCALE, vec![ScriptType::Number]),
    ] {
        registry
            .set_signature(
                builtin,
                FunctionSignature {
                    receiver: Some(ScriptType::Named(actor_type)),
                    parameters,
                    result: ScriptType::Named(actor_type),
                },
            )
            .expect("actor method signature must target a registered builtin");
    }
    register_position_api(&mut registry, position_type);
    registry
}

fn register_position_api(
    registry: &mut NativeRegistry<CharacterContext>,
    position_type: hiraku_script::SymbolId,
) {
    for (builtin, name, kind, position) in [
        (
            POSITION_LEFT,
            "left",
            StaticMemberKind::Getter,
            Position::Absolute(-600.0, -200.0),
        ),
        (
            POSITION_CENTER,
            "center",
            StaticMemberKind::Getter,
            Position::Absolute(0.0, -200.0),
        ),
        (
            POSITION_RIGHT,
            "right",
            StaticMemberKind::Getter,
            Position::Absolute(600.0, -200.0),
        ),
    ] {
        registry
            .register_static_raw_fn_with_id(
                builtin,
                position_type,
                name,
                FunctionSignature {
                    receiver: None,
                    parameters: Vec::new(),
                    result: ScriptType::Named(position_type),
                },
                kind,
                move |_context, _call| Ok(position_value(position_type, position)),
            )
            .expect("position getter registration must be unique");
    }
    for (builtin, name, relative) in [
        (POSITION_ABSOLUTE, "pos", false),
        (POSITION_RELATIVE, "rel", true),
    ] {
        registry
            .register_static_raw_fn_with_id(
                builtin,
                position_type,
                name,
                FunctionSignature {
                    receiver: None,
                    parameters: vec![ScriptType::Number, ScriptType::Number],
                    result: ScriptType::Named(position_type),
                },
                StaticMemberKind::Method,
                move |_context, call| {
                    let [x, y] = call.arguments.as_slice() else {
                        return Err(NativeError::Arity {
                            expected: 2,
                            actual: call.arguments.len(),
                        });
                    };
                    let Value::Number(x) = &x.value else {
                        return Err(NativeError::TypeMismatch("number"));
                    };
                    let Value::Number(y) = &y.value else {
                        return Err(NativeError::TypeMismatch("number"));
                    };
                    let position = if relative {
                        Position::relative(*x, *y)?
                    } else {
                        Position::Absolute(*x, *y)
                    };
                    Ok(position_value(position_type, position))
                },
            )
            .expect("position constructor registration must be unique");
    }
}

fn story_registry() -> NativeRegistry<CharacterContext> {
    let mut registry = registry();
    registry
        .register_raw_fn_with_id(OPEN_UI, "openUi", async_capability_placeholder)
        .expect("built-in `openUi` registration must be unique");
    registry
        .register_raw_fn_with_id(WAIT, "wait", async_capability_placeholder)
        .expect("built-in `wait` registration must be unique");
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

/// Stateful native-function host for the direct HKS runtime.
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
}

pub fn compile_expression(
    expression: &Expr,
    functions: &[Stmt],
    source_hash: u64,
) -> Option<Bytecode> {
    let mut statements = functions.to_vec();
    statements.push(Stmt::Expr(expression.clone()));
    let program = Program { statements };
    compile_with_manifest(&program, source_hash, &manifest()).ok()
}

pub fn compile_statement(
    statement: &Stmt,
    functions: &[Stmt],
    source_hash: u64,
) -> Option<Bytecode> {
    let mut statements = functions.to_vec();
    statements.push(statement.clone());
    compile_with_manifest(&Program { statements }, source_hash, &manifest()).ok()
}

#[derive(Clone, Debug)]
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
            native_narrate(self, text.clone())
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

#[derive(Clone, Copy)]
struct ActorHandle(u64);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Position {
    Absolute(f64, f64),
    Relative(u16, u16),
}

impl Position {
    fn relative(x: f64, y: f64) -> Result<Self, NativeError> {
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

impl FromHksValue for Position {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::String(preset) => match preset.as_str() {
                "left" => Ok(Self::Absolute(-600.0, -200.0)),
                "center" => Ok(Self::Absolute(0.0, -200.0)),
                "right" => Ok(Self::Absolute(600.0, -200.0)),
                _ => Err(NativeError::message(format!(
                    "unknown character position `{preset}`"
                ))),
            },
            Value::Typed { value, .. } => position_payload(value),
            _ => Err(NativeError::TypeMismatch("Position")),
        }
    }
}

fn position_value(type_id: hiraku_script::SymbolId, position: Position) -> Value {
    let (kind, x, y) = match position {
        Position::Absolute(x, y) => ("absolute", x, y),
        Position::Relative(x, y) => ("relative", f64::from(x), f64::from(y)),
    };
    Value::Typed {
        type_id,
        value: Box::new(Value::Tuple(vec![
            Value::Symbol(kind.to_string()),
            Value::Number(x),
            Value::Number(y),
        ])),
    }
}

fn position_payload(value: &Value) -> Result<Position, NativeError> {
    let Value::Tuple(fields) = value else {
        return Err(NativeError::TypeMismatch("Position"));
    };
    let [Value::Symbol(kind), Value::Number(x), Value::Number(y)] = fields.as_slice() else {
        return Err(NativeError::TypeMismatch("Position"));
    };
    match kind.as_str() {
        "absolute" => Ok(Position::Absolute(*x, *y)),
        "relative" => Position::relative(*x, *y),
        _ => Err(NativeError::message("unknown Position variant")),
    }
}

impl FromHksValue for ActorHandle {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        actor_handle(value)
            .map(Self)
            .map_err(|error| NativeError::message(error.to_string()))
    }
}

impl IntoHksValue for ActorHandle {
    fn into_hks_value(self) -> Value {
        actor_value(self.0)
    }
}

fn native_char(context: &mut CharacterContext, name: String) -> Result<ActorHandle, NativeError> {
    context
        .char(name)
        .map_err(|error| NativeError::message(error.to_string()))
}

fn native_emotion(
    context: &mut CharacterContext,
    actor: ActorHandle,
    emotion: String,
) -> Result<ActorHandle, NativeError> {
    context
        .emotion(actor, emotion)
        .map_err(|error| NativeError::message(error.to_string()))
}

fn native_at(
    context: &mut CharacterContext,
    actor: ActorHandle,
    position: Position,
) -> Result<ActorHandle, NativeError> {
    context
        .at(actor, position)
        .map_err(|error| NativeError::message(error.to_string()))
}

fn native_scale(
    context: &mut CharacterContext,
    actor: ActorHandle,
    scale: f64,
) -> Result<ActorHandle, NativeError> {
    context
        .scale(actor, scale)
        .map_err(|error| NativeError::message(error.to_string()))
}

fn native_log(context: &mut CharacterContext, message: String) -> Result<(), NativeError> {
    context.commands.push(StoryEffect::Log(message));
    Ok(())
}

fn native_clear_text(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.last_speaker = None;
    context.dialogue_buffer = None;
    context.commands.push(StoryEffect::ClearDialogue);
    Ok(())
}

fn native_stop_bgm(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(StoryEffect::StopBgm);
    Ok(())
}

fn native_exit(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(StoryEffect::Exit);
    Ok(())
}

fn native_return_to_title(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(StoryEffect::ReturnToTitle);
    Ok(())
}

fn native_bg(context: &mut CharacterContext, texture: String) -> Result<(), NativeError> {
    context
        .commands
        .push(StoryEffect::SetBackground { texture });
    Ok(())
}

fn native_load_script(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
    context.commands.push(StoryEffect::LoadScript { path });
    Ok(())
}

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

fn native_narrate(context: &mut CharacterContext, text: String) -> Result<(), NativeError> {
    context.last_speaker = Some(String::new());
    context.dialogue_buffer = Some(text.clone());
    context.commands.push(StoryEffect::Say {
        speaker: String::new(),
        text,
    });
    context.wait = Some(StoryWait::DialogueAdvance);
    Ok(())
}

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

fn native_voice(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
    if path.trim().is_empty() {
        return Err(NativeError::message("voice path must not be empty"));
    }
    context
        .commands
        .push(StoryEffect::PlayVoice { path, volume: 1.0 });
    Ok(())
}

fn native_camera_blur(
    context: &mut CharacterContext,
    call: &hiraku_script::vm::BuiltinCall,
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
    let intensity = intensity.ok_or_else(|| NativeError::message("blur intensity is required"))?;
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

fn native_camera_zoom(
    context: &mut CharacterContext,
    call: &hiraku_script::vm::BuiltinCall,
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

fn require_selector(
    call: &hiraku_script::vm::BuiltinCall,
    expected: &str,
) -> Result<(), NativeError> {
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

fn actor_value(id: u64) -> Value {
    Value::Handle {
        type_id: ACTOR_HANDLE_TYPE,
        id,
    }
}

fn actor_handle(value: &Value) -> Result<u64, CharacterCapabilityError> {
    match value {
        Value::Handle {
            type_id: ACTOR_HANDLE_TYPE,
            id,
        } => Ok(*id),
        _ => Err(CharacterCapabilityError::InvalidActorHandle),
    }
}

pub struct CapabilityOutput {
    pub commands: Vec<StoryEffect>,
    pub wait: Option<StoryWait>,
    pub tasks: Vec<CapabilityTask>,
    pub locals: BTreeMap<String, Value>,
}

#[derive(Debug, PartialEq)]
pub struct CapabilityTask {
    pub mode: TaskMode,
    pub commands: Vec<StoryEffect>,
}

pub fn execute(bytecode: Bytecode) -> Result<CapabilityOutput, CharacterCapabilityError> {
    execute_with_host(bytecode, &mut StoryNativeHost::new(), BTreeMap::new())
}

pub fn execute_with_host(
    bytecode: Bytecode,
    host: &mut StoryNativeHost,
    locals: BTreeMap<String, Value>,
) -> Result<CapabilityOutput, CharacterCapabilityError> {
    if bytecode.builtin_manifest_hash != manifest().hash() {
        return Err(CharacterCapabilityError::ManifestMismatch);
    }
    let scheduler_bytecode = bytecode.clone();
    let mut vm =
        Vm::new(bytecode).map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
    vm.set_locals(locals);
    let registry = registry();
    let mut tasks = Vec::new();
    loop {
        match vm
            .step()
            .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?
        {
            Some(VmEvent::Call(call)) => {
                if host.context.wait.is_some() {
                    return Err(CharacterCapabilityError::Native(
                        "a suspending capability must be the final native call in a statement"
                            .to_string(),
                    ));
                }
                let value = registry
                    .call(&mut host.context, &call)
                    .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
                vm.resume_builtin(value)
                    .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
            }
            Some(VmEvent::Statement(value)) => host.context.handle_statement(&value)?,
            Some(VmEvent::Completed(_)) => {
                return Ok(CapabilityOutput {
                    commands: host.drain_effects(),
                    wait: host.take_wait(),
                    tasks,
                    locals: vm.locals().clone(),
                });
            }
            Some(VmEvent::SpawnTask(request)) => {
                tasks.push(execute_task(
                    scheduler_bytecode.clone(),
                    request.task,
                    request.template.mode,
                    &registry,
                )?);
                vm.resume(Value::Task(request.task as u64))
                    .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
            }
            None => {
                return Err(CharacterCapabilityError::Vm(
                    "VM stopped before completion".to_string(),
                ));
            }
        }
    }
}

fn execute_task(
    bytecode: Bytecode,
    template: u32,
    mode: TaskMode,
    registry: &NativeRegistry<CharacterContext>,
) -> Result<CapabilityTask, CharacterCapabilityError> {
    let mut scheduler = TaskScheduler::new(bytecode)
        .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
    let root = scheduler
        .spawn(template)
        .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
    let mut contexts = BTreeMap::<u64, CharacterContext>::new();

    while !matches!(scheduler.status(root), Some(TaskStatus::Completed(_))) {
        match scheduler
            .step()
            .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?
        {
            Some(TaskEvent::Call { task, call }) => {
                let context = contexts.entry(task).or_default();
                if context.wait.is_some() {
                    return Err(CharacterCapabilityError::SuspendingTaskCapability);
                }
                let value = registry
                    .call(context, &call)
                    .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
                if context.wait.is_some() {
                    return Err(CharacterCapabilityError::SuspendingTaskCapability);
                }
                scheduler
                    .resume(task, value)
                    .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
            }
            Some(TaskEvent::Statement { task, value }) => {
                contexts.entry(task).or_default().handle_statement(&value)?;
            }
            Some(TaskEvent::Completed { .. }) => {}
            None => {
                return Err(CharacterCapabilityError::Vm(
                    "task scheduler stopped before the root task completed".to_string(),
                ));
            }
        }
    }

    Ok(CapabilityTask {
        mode,
        commands: contexts
            .into_values()
            .flat_map(|context| context.commands)
            .collect(),
    })
}

#[derive(Debug, Error, PartialEq)]
pub enum CharacterCapabilityError {
    #[error("character builtin manifest does not match bytecode")]
    ManifestMismatch,
    #[error("invalid native arguments: {0}")]
    InvalidArguments(&'static str),
    #[error("invalid actor handle")]
    InvalidActorHandle,
    #[error("unknown actor handle {0}")]
    UnknownActor(u64),
    #[error("capabilities which suspend for host input are not yet supported inside seq/par")]
    SuspendingTaskCapability,
    #[error("HKS VM error: {0}")]
    Vm(String),
    #[error("HKS native error: {0}")]
    Native(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::hks_runtime::{HksRuntime, HksRuntimeEvent};
    use hiraku_script::parse_program;

    #[test]
    fn fluent_calls_flush_once_at_the_statement_boundary() {
        let program = parse_program(
            r#"char("Alice").e("happy_eyes").e("happy_face").at("right").scale(0.5)"#,
        )
        .unwrap();
        let Stmt::Expr(expression) = &program.statements[0] else {
            panic!()
        };
        let output = execute(compile_expression(expression, &[], 42).unwrap()).unwrap();
        assert_eq!(output.commands.len(), 1);
        assert!(
            matches!(&output.commands[0], StoryEffect::ShowCharacter { actor_id, expressions, position, scale, .. }
            if actor_id == "Alice" && expressions == &["happy_eyes", "happy_face"]
                && position == &[600.0, -200.0] && (*scale - 0.5).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn typed_positions_support_relative_constructors_and_getters() {
        for (source, expected) in [
            (r#"char("Alice").at(.rel(50, 50))"#, [0.0, 0.0]),
            (r#"char("Alice").at(.left)"#, [-600.0, -200.0]),
        ] {
            let program = parse_program(source).expect("typed position syntax must parse");
            let Stmt::Expr(expression) = &program.statements[0] else {
                panic!("expected character expression")
            };
            let output = execute(
                compile_expression(expression, &[], 48)
                    .expect("typed position must pass signature checking"),
            )
            .expect("typed position calls must execute");
            assert!(matches!(
                &output.commands[0],
                StoryEffect::ShowCharacter { position, .. } if position == &expected
            ));
        }
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
    fn voice_is_a_non_suspending_native_command_by_default() {
        let program = parse_program(r#"voice("voice/scene01/hash1")"#).unwrap();
        let Stmt::Expr(expression) = &program.statements[0] else {
            panic!("expected a voice expression")
        };
        let output = execute(compile_expression(expression, &[], 43).unwrap()).unwrap();
        assert_eq!(output.wait, None);
        assert_eq!(
            output.commands,
            vec![StoryEffect::PlayVoice {
                path: "voice/scene01/hash1".to_string(),
                volume: 1.0,
            }]
        );
    }

    #[test]
    fn parallel_voice_block_is_executed_through_the_task_scheduler() {
        let program = parse_program(
            r#"par {
                voice("voice/scene01/first")
                voice("voice/scene01/second")
            }"#,
        )
        .expect("parallel voice block must parse");
        let Stmt::Expr(expression) = &program.statements[0] else {
            panic!("expected a parallel task expression")
        };
        let output = execute(
            compile_expression(expression, &[], 45)
                .expect("parallel voice block must compile to native bytecode"),
        )
        .expect("parallel voice block must execute through the task scheduler");

        assert!(output.commands.is_empty());
        assert_eq!(output.tasks.len(), 1);
        assert_eq!(output.tasks[0].mode, TaskMode::Parallel);
        assert_eq!(
            output.tasks[0].commands,
            vec![
                StoryEffect::PlayVoice {
                    path: "voice/scene01/first".to_string(),
                    volume: 1.0,
                },
                StoryEffect::PlayVoice {
                    path: "voice/scene01/second".to_string(),
                    volume: 1.0,
                },
            ]
        );
    }

    #[test]
    fn character_calls_are_valid_sequence_and_parallel_task_commands() {
        for source in [
            r#"seq { char("Alice").e("happy").at("left") }"#,
            "par {\nchar(\"Alice\").e(\"happy\")\nchar(\"Bob\").at(\"right\")\n}",
        ] {
            let program = parse_program(source).expect("character task must parse");
            let Stmt::Expr(expression) = &program.statements[0] else {
                panic!("expected a task expression")
            };
            let output = execute(
                compile_expression(expression, &[], 46)
                    .expect("character task must compile to bytecode"),
            )
            .expect("character task must execute through the scheduler");
            assert_eq!(output.tasks.len(), 1);
            assert!(
                output.tasks[0]
                    .commands
                    .iter()
                    .all(|command| matches!(command, StoryEffect::ShowCharacter { .. }))
            );
        }
    }

    #[test]
    fn say_emits_dialogue_and_suspends_for_advance() {
        let program = parse_program(r#"say("Alice", "Hello")"#).unwrap();
        let Stmt::Expr(expression) = &program.statements[0] else {
            panic!("expected a say expression")
        };
        let output = execute(compile_expression(expression, &[], 44).unwrap()).unwrap();
        assert_eq!(output.wait, Some(StoryWait::DialogueAdvance));
        assert_eq!(
            output.commands,
            vec![StoryEffect::Say {
                speaker: "Alice".to_string(),
                text: "Hello".to_string(),
            }]
        );
    }

    #[test]
    fn engine_hooks_dialogue_sugar_without_vm_story_knowledge() {
        let bytecode = compile_story_bytecode(
            "dialogue.story.hks",
            r#"
                let ema = char("ema")
                ema: "first"
                ...: "continued"
                "narration"
                char("ema").e("happy"): "inline"
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
                (false, "ema".to_string(), "first".to_string()),
                (true, String::new(), "continued".to_string()),
                (false, String::new(), "narration".to_string()),
                (false, "ema".to_string(), "inline".to_string()),
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
