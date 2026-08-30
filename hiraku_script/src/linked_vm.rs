//! Execution of runtime-linked native and cross-module script calls.
use serde::{Deserialize, Serialize};

use crate::{
    BuiltinCall, BuiltinManifest, Bytecode, LinkedFunction, LinkedProgram, ModuleId,
    StatementValue, Value, Vm, VmError, VmEvent, VmSnapshot, link_register_modules,
};

#[derive(Clone, Debug, PartialEq)]
pub enum LinkedVmEvent {
    Call(BuiltinCall),
    Statement(StatementValue),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkedVmFrameSnapshot {
    pub module: ModuleId,
    pub vm: VmSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkedVmSnapshot {
    pub modules: Vec<Bytecode>,
    pub frames: Vec<LinkedVmFrameSnapshot>,
}

pub struct LinkedVm {
    program: LinkedProgram,
    frames: Vec<(ModuleId, Vm)>,
}

impl LinkedVm {
    pub fn new(program: LinkedProgram, entry: ModuleId) -> Result<Self, LinkedVmError> {
        let module = program
            .modules
            .get(entry.0 as usize)
            .ok_or(LinkedVmError::UnknownModule(entry))?;
        let vm = Vm::new(module.bytecode.clone())?;
        Ok(Self {
            program,
            frames: vec![(entry, vm)],
        })
    }

    /// Starts an independent invocation of a save-safe callable while sharing
    /// the same linked module set. This is used by embeddings which evaluate a
    /// trailing closure after a native builder has inspected it.
    pub fn from_callable(
        program: LinkedProgram,
        callable: &Value,
        arguments: Vec<Value>,
    ) -> Result<Self, LinkedVmError> {
        let module = match callable {
            Value::Closure {
                module: Some(module),
                ..
            } => ModuleId(*module),
            Value::Closure { module: None, .. } => {
                return Err(LinkedVmError::UnboundClosureModule);
            }
            _ => {
                return Err(LinkedVmError::Vm(VmError::TypeMismatch(
                    "expected Function",
                )));
            }
        };
        let bytecode = program
            .modules
            .get(module.0 as usize)
            .ok_or(LinkedVmError::UnknownModule(module))?
            .bytecode
            .clone();
        let vm = Vm::from_callable(bytecode, callable, arguments)?;
        Ok(Self {
            program,
            frames: vec![(module, vm)],
        })
    }

    pub fn program(&self) -> &LinkedProgram {
        &self.program
    }

    pub fn step(&mut self) -> Result<Option<LinkedVmEvent>, LinkedVmError> {
        loop {
            let (module_id, vm) = self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?;
            let Some(event) = vm.step()? else {
                return Ok(None);
            };
            match event {
                VmEvent::Statement(value) => {
                    return Ok(Some(LinkedVmEvent::Statement(value)));
                }
                VmEvent::Completed(value) => {
                    let value = bind_value_module(value, *module_id);
                    self.frames.pop();
                    if let Some((_, caller)) = self.frames.last_mut() {
                        caller.resume(value)?;
                        continue;
                    }
                    return Ok(Some(LinkedVmEvent::Completed(value)));
                }
                VmEvent::Call(call) => {
                    let module = &self.program.modules[module_id.0 as usize];
                    match module.resolve(call.function) {
                        Some(LinkedFunction::Native(builtin)) => {
                            let receiver = call
                                .receiver
                                .map(|value| bind_value_module(value, *module_id));
                            let arguments = call
                                .arguments
                                .into_iter()
                                .map(|mut argument| {
                                    argument.value = bind_value_module(argument.value, *module_id);
                                    argument
                                })
                                .collect();
                            return Ok(Some(LinkedVmEvent::Call(BuiltinCall {
                                builtin,
                                receiver,
                                arguments,
                            })));
                        }
                        Some(LinkedFunction::Script { module, function }) => {
                            if call.receiver.is_some() {
                                return Err(LinkedVmError::ScriptReceiver);
                            }
                            let bytecode = self
                                .program
                                .modules
                                .get(module.0 as usize)
                                .ok_or(LinkedVmError::UnknownModule(module))?
                                .bytecode
                                .clone();
                            let arguments = call
                                .arguments
                                .into_iter()
                                .map(|argument| bind_value_module(argument.value, *module_id))
                                .collect();
                            let callee = Vm::from_function(bytecode, function, arguments)?;
                            self.frames.push((module, callee));
                        }
                        None => return Err(LinkedVmError::UnlinkedCall(call.function)),
                    }
                }
            }
        }
    }

    pub fn resume(&mut self, value: Value) -> Result<(), LinkedVmError> {
        self.frames
            .last_mut()
            .ok_or(LinkedVmError::NoFrame)?
            .1
            .resume(value)?;
        Ok(())
    }

    pub fn snapshot(&self) -> LinkedVmSnapshot {
        LinkedVmSnapshot {
            modules: self
                .program
                .modules
                .iter()
                .map(|module| module.bytecode.clone())
                .collect(),
            frames: self
                .frames
                .iter()
                .map(|(module, vm)| LinkedVmFrameSnapshot {
                    module: *module,
                    vm: vm.snapshot(),
                })
                .collect(),
        }
    }

    pub fn restore(
        snapshot: LinkedVmSnapshot,
        natives: &BuiltinManifest,
    ) -> Result<Self, LinkedVmError> {
        let program =
            link_register_modules(snapshot.modules, natives).map_err(LinkedVmError::Link)?;
        let frames = snapshot
            .frames
            .into_iter()
            .map(|frame| {
                let bytecode = program
                    .modules
                    .get(frame.module.0 as usize)
                    .ok_or(LinkedVmError::UnknownModule(frame.module))?
                    .bytecode
                    .clone();
                Ok((frame.module, Vm::restore(bytecode, frame.vm)?))
            })
            .collect::<Result<_, LinkedVmError>>()?;
        Ok(Self { program, frames })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkedVmError {
    Vm(VmError),
    Link(Vec<crate::LinkError>),
    UnknownModule(ModuleId),
    UnlinkedCall(crate::SymbolId),
    ScriptReceiver,
    NoFrame,
    UnboundClosureModule,
}

fn bind_value_module(value: Value, module: ModuleId) -> Value {
    match value {
        Value::Closure {
            module: owner,
            region,
            captures,
        } => Value::Closure {
            module: owner.or(Some(module.0)),
            region,
            captures: captures
                .into_iter()
                .map(|value| bind_value_module(value, module))
                .collect(),
        },
        Value::Typed { type_id, value } => Value::Typed {
            type_id,
            value: Box::new(bind_value_module(*value, module)),
        },
        Value::Tuple(values) => Value::Tuple(
            values
                .into_iter()
                .map(|value| bind_value_module(value, module))
                .collect(),
        ),
        Value::List(values) => Value::List(
            values
                .into_iter()
                .map(|value| bind_value_module(value, module))
                .collect(),
        ),
        Value::Map(values) => Value::Map(
            values
                .into_iter()
                .map(|(key, value)| (key, bind_value_module(value, module)))
                .collect(),
        ),
        value => value,
    }
}

impl From<VmError> for LinkedVmError {
    fn from(value: VmError) -> Self {
        Self::Vm(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinId, compile_with_manifest, parse_program};

    use super::*;

    fn compile(source: &str, manifest: &BuiltinManifest) -> Bytecode {
        compile_with_manifest(&parse_program(source).expect("source parses"), 31, manifest)
            .expect("source compiles")
    }

    #[test]
    fn executes_and_restores_cross_module_global_calls() {
        let natives = BuiltinManifest::new([("nativeEcho", BuiltinId(4))]);
        let provider = compile(
            "global fn greet(name: String) { nativeEcho(name) }",
            &natives,
        );
        let consumer = compile("greet(\"alice\")", &natives);
        let program =
            link_register_modules(vec![provider, consumer], &natives).expect("modules link");
        let mut vm = LinkedVm::new(program, ModuleId(1)).expect("entry starts");
        let Some(LinkedVmEvent::Call(call)) = vm.step().expect("native call yields") else {
            panic!("expected native call")
        };
        assert_eq!(call.builtin, BuiltinId(4));
        assert_eq!(call.arguments[0].value, Value::String("alice".into()));

        let snapshot = vm.snapshot();
        let mut restored = LinkedVm::restore(snapshot, &natives).expect("linked frames restore");
        restored
            .resume(Value::String("hello".into()))
            .expect("call resumes");
        loop {
            if matches!(
                restored.step().expect("execution succeeds"),
                Some(LinkedVmEvent::Completed(_))
            ) {
                break;
            }
        }
    }

    #[test]
    fn closures_keep_their_module_when_crossing_a_native_boundary() {
        let natives = BuiltinManifest::new([("capture", BuiltinId(9))]);
        let module = compile("capture { \"child\" }", &natives);
        let program = link_register_modules(vec![module], &natives).expect("module links");
        let mut vm = LinkedVm::new(program, ModuleId(0)).expect("entry starts");
        let Some(LinkedVmEvent::Call(call)) = vm.step().expect("capture yields") else {
            panic!("expected native capture call")
        };
        let closure = call.arguments[0].value.clone();
        assert!(matches!(
            closure,
            Value::Closure {
                module: Some(0),
                ..
            }
        ));

        let mut child = LinkedVm::from_callable(vm.program().clone(), &closure, Vec::new())
            .expect("bound closure invokes");
        assert_eq!(
            child.step().expect("closure executes"),
            Some(LinkedVmEvent::Statement(StatementValue::String(
                "child".into()
            )))
        );
    }
}
