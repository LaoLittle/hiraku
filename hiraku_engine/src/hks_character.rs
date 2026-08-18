//! Engine-owned native capabilities for fluent HKS character statements.

use std::collections::BTreeMap;

use hiraku_script::hks::native::{FromHksValue, IntoHksValue, NativeError, NativeRegistry};
use hiraku_script::hks::vm::{
    BuiltinId, BuiltinManifest, Bytecode, Instruction, Value, Vm, VmEvent, compile_with_manifest,
};
use hiraku_script::hks::{Expr, Program, Stmt};
use thiserror::Error;

use crate::script::IrCommand;

const ACTOR_HANDLE_TYPE: u32 = 1;
const CHAR: BuiltinId = BuiltinId(1);
const EMOTION: BuiltinId = BuiltinId(2);
const AT: BuiltinId = BuiltinId(3);
const SCALE: BuiltinId = BuiltinId(4);

pub fn manifest() -> BuiltinManifest {
    registry().manifest()
}

fn registry() -> NativeRegistry<CharacterContext> {
    let mut registry = NativeRegistry::new();
    registry
        .register_fn_with_id(CHAR, "char", native_char)
        .unwrap();
    registry
        .register_fn_with_id(EMOTION, "e", native_emotion)
        .unwrap();
    registry.register_fn_with_id(AT, "at", native_at).unwrap();
    registry
        .register_fn_with_id(SCALE, "scale", native_scale)
        .unwrap();
    registry
}

pub fn compile_expression(expression: &Expr, source_hash: u64) -> Option<Bytecode> {
    let program = Program {
        statements: vec![Stmt::Expr(expression.clone())],
    };
    let bytecode = compile_with_manifest(&program, source_hash, &manifest()).ok()?;
    matches!(
        bytecode
            .instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::CallBuiltin { builtin, .. } => Some(*builtin),
                _ => None,
            }),
        Some(CHAR)
    )
    .then_some(bytecode)
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

pub fn execute(bytecode: Bytecode) -> Result<Vec<IrCommand>, CharacterCapabilityError> {
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
                let value = registry
                    .call(&mut context, &call)
                    .map_err(|error| CharacterCapabilityError::Native(error.to_string()))?;
                vm.resume_builtin(value)
                    .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?;
            }
            Some(VmEvent::StatementCommit) => context.commit()?,
            Some(VmEvent::Completed(_)) => return Ok(context.commands),
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
        let commands = execute(compile_expression(expression, 42).unwrap()).unwrap();
        assert_eq!(commands.len(), 1);
        assert!(
            matches!(&commands[0], IrCommand::ShowCharacter { actor_id, expressions, position, scale, .. }
            if actor_id == "Alice" && expressions == &["happy_eyes", "happy_face"]
                && position == &[600.0, 0.0] && (*scale - 0.5).abs() < f32::EPSILON)
        );
    }
}
