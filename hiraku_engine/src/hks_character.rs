//! Engine-owned native capabilities for fluent HKS character statements.

use std::collections::BTreeMap;

use hiraku_script::hks::vm::{
    BuiltinCall, BuiltinId, BuiltinManifest, Bytecode, Instruction, Value, Vm, VmEvent,
    compile_with_manifest,
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
    BuiltinManifest::new([("char", CHAR), ("e", EMOTION), ("at", AT), ("scale", SCALE)])
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
    fn call(&mut self, call: BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        match call.builtin {
            CHAR => self.char_call(&call),
            EMOTION => self.emotion_call(&call),
            AT => self.at_call(&call),
            SCALE => self.scale_call(&call),
            id => Err(CharacterCapabilityError::UnknownBuiltin(id)),
        }
    }

    fn char_call(&mut self, call: &BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        let [argument] = call.arguments.as_slice() else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "char expects one name",
            ));
        };
        let Value::String(name) = &argument.value else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "char name must be a string",
            ));
        };
        if let Some(handle) = self.handles_by_name.get(name).copied() {
            self.flush(handle)?;
            self.actors.insert(handle, pending_actor(name));
            return Ok(actor_value(handle));
        }
        self.next_handle += 1;
        let handle = self.next_handle;
        self.handles_by_name.insert(name.clone(), handle);
        self.actors.insert(handle, pending_actor(name));
        Ok(actor_value(handle))
    }

    fn emotion_call(&mut self, call: &BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        let [actor, emotion] = call.arguments.as_slice() else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "e expects an actor and emotion",
            ));
        };
        let handle = actor_handle(&actor.value)?;
        let Value::String(emotion) = &emotion.value else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "emotion must be a string",
            ));
        };
        let pending = self.actor_mut(handle)?;
        pending.expressions.push(emotion.clone());
        pending.dirty = true;
        Ok(actor_value(handle))
    }

    fn at_call(&mut self, call: &BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        let [actor, position] = call.arguments.as_slice() else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "at expects an actor and position",
            ));
        };
        let handle = actor_handle(&actor.value)?;
        let Value::String(position) = &position.value else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "position must be a string",
            ));
        };
        self.actor_mut(handle)?.position = match position.as_str() {
            "left" => [-600.0, 0.0],
            "center" => [0.0, 0.0],
            "right" => [600.0, 0.0],
            _ => return Err(CharacterCapabilityError::InvalidPosition(position.clone())),
        };
        self.actor_mut(handle)?.dirty = true;
        Ok(actor_value(handle))
    }

    fn scale_call(&mut self, call: &BuiltinCall) -> Result<Value, CharacterCapabilityError> {
        let [actor, scale] = call.arguments.as_slice() else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "scale expects an actor and number",
            ));
        };
        let handle = actor_handle(&actor.value)?;
        let Value::Number(scale) = scale.value else {
            return Err(CharacterCapabilityError::InvalidArguments(
                "scale must be numeric",
            ));
        };
        if scale <= 0.0 {
            return Err(CharacterCapabilityError::InvalidArguments(
                "scale must be positive",
            ));
        }
        self.actor_mut(handle)?.scale = scale as f32;
        self.actor_mut(handle)?.dirty = true;
        Ok(actor_value(handle))
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
    loop {
        match vm
            .step()
            .map_err(|error| CharacterCapabilityError::Vm(format!("{error:?}")))?
        {
            Some(VmEvent::Call(call)) => {
                let value = context.call(call)?;
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
    #[error("unknown builtin {0:?}")]
    UnknownBuiltin(BuiltinId),
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
