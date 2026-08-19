//! Engine-owned native capabilities registered into the generic HKS VM.

use std::collections::BTreeMap;

use hiraku_script::hks::native::{FromHksValue, IntoHksValue, NativeError, NativeRegistry};
use hiraku_script::hks::vm::{
    BuiltinId, BuiltinManifest, Bytecode, Value, Vm, VmEvent, compile_with_manifest,
};
use hiraku_script::hks::{Expr, Program, Stmt};
use thiserror::Error;

use crate::script::{CameraEffectScope, IrCommand, IrWaitKind};

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

pub fn manifest() -> BuiltinManifest {
    registry().manifest()
}

fn registry() -> NativeRegistry<CharacterContext> {
    let mut registry = NativeRegistry::new();
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
        .register_raw_fn_with_id(CAMERA_BLUR, "camera.blur", native_camera_blur)
        .expect("built-in `camera.blur` registration must be unique");
    registry
        .register_raw_fn_with_id(CAMERA_ZOOM, "camera.zoom", native_camera_zoom)
        .expect("built-in `camera.zoom` registration must be unique");
    registry
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
    commands: Vec<IrCommand>,
    wait: Option<IrWaitKind>,
}

impl CharacterContext {
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
        position: String,
    ) -> Result<ActorHandle, CharacterCapabilityError> {
        self.actor_mut(handle)?.position = match position.as_str() {
            "left" => [-600.0, 0.0],
            "center" => [0.0, 0.0],
            "right" => [600.0, 0.0],
            _ => return Err(CharacterCapabilityError::InvalidPosition(position)),
        };
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
            IrCommand::ShowCharacter {
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
    position: String,
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
    context.commands.push(IrCommand::Log(message));
    Ok(())
}

fn native_clear_text(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(IrCommand::ClearDialogue);
    Ok(())
}

fn native_stop_bgm(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(IrCommand::StopBgm);
    Ok(())
}

fn native_exit(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(IrCommand::Exit);
    Ok(())
}

fn native_return_to_title(context: &mut CharacterContext) -> Result<(), NativeError> {
    context.commands.push(IrCommand::ReturnToTitle);
    Ok(())
}

fn native_bg(context: &mut CharacterContext, texture: String) -> Result<(), NativeError> {
    context.commands.push(IrCommand::SetBackground { texture });
    Ok(())
}

fn native_load_script(context: &mut CharacterContext, path: String) -> Result<(), NativeError> {
    context.commands.push(IrCommand::LoadScript { path });
    Ok(())
}

fn native_adjust_setting(
    context: &mut CharacterContext,
    name: String,
    delta: f64,
) -> Result<(), NativeError> {
    context.commands.push(IrCommand::AdjustSetting {
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
    context.commands.push(IrCommand::PlayBgm {
        path,
        volume: volume as f32,
        fade_in_ms: Some(fade_ms.round() as u64),
    });
    Ok(())
}

fn native_narrate(context: &mut CharacterContext, text: String) -> Result<(), NativeError> {
    context.commands.push(IrCommand::Say {
        speaker: String::new(),
        text,
    });
    context.wait = Some(IrWaitKind::DialogueAdvance);
    Ok(())
}

fn native_camera_blur(
    context: &mut CharacterContext,
    call: &hiraku_script::hks::vm::BuiltinCall,
) -> Result<Value, NativeError> {
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
    context.commands.push(IrCommand::SetCamera {
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
    call: &hiraku_script::hks::vm::BuiltinCall,
) -> Result<Value, NativeError> {
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
    context.commands.push(IrCommand::SetCamera {
        blur: None,
        zoom: Some(scale as f32),
        scope,
        duration_ms: (duration * 1000.0).round() as u64,
        ease: normalize_ease(&ease)?,
    });
    Ok(Value::Null)
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
    pub commands: Vec<IrCommand>,
    pub wait: Option<IrWaitKind>,
}

pub fn execute(bytecode: Bytecode) -> Result<CapabilityOutput, CharacterCapabilityError> {
    if bytecode.builtin_manifest_hash != manifest().hash() {
        return Err(CharacterCapabilityError::ManifestMismatch);
    }
    let mut vm =
        Vm::new(bytecode).map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
    let mut context = CharacterContext::default();
    let registry = registry();
    loop {
        match vm
            .step()
            .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?
        {
            Some(VmEvent::Call(call)) => {
                if context.wait.is_some() {
                    return Err(CharacterCapabilityError::Native(
                        "a suspending capability must be the final native call in a statement"
                            .to_string(),
                    ));
                }
                let value = registry
                    .call(&mut context, &call)
                    .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
                vm.resume_builtin(value)
                    .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
            }
            Some(VmEvent::StatementCommit) => context.commit()?,
            Some(VmEvent::Completed(_)) => {
                return Ok(CapabilityOutput {
                    commands: context.commands,
                    wait: context.wait,
                });
            }
            Some(VmEvent::SpawnTask(_)) => return Err(CharacterCapabilityError::TasksUnsupported),
            None => {
                return Err(CharacterCapabilityError::Vm(
                    "VM stopped before completion".to_string(),
                ));
            }
        }
    }
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
    #[error("invalid character position `{0}`")]
    InvalidPosition(String),
    #[error("tasks are not supported in a character statement")]
    TasksUnsupported,
    #[error("HKS VM error: {0}")]
    Vm(String),
    #[error("HKS native error: {0}")]
    Native(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiraku_script::hks::parse_program;

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
            matches!(&output.commands[0], IrCommand::ShowCharacter { actor_id, expressions, position, scale, .. }
            if actor_id == "Alice" && expressions == &["happy_eyes", "happy_face"]
                && position == &[600.0, 0.0] && (*scale - 0.5).abs() < f32::EPSILON)
        );
    }
}
