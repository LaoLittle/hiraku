//! Execution of runtime-linked native and cross-module script calls.
use serde::{Deserialize, Serialize};

use crate::{
    BuiltinCall, LinkedFunction, LinkedProgram, ModuleId, StatementValue, Value, Vm, VmError,
    VmEvent, VmSnapshot,
};

#[derive(Clone, Debug, PartialEq)]
pub enum LinkedVmEvent {
    BudgetExhausted,
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
    pub objects: crate::ObjectHeap,
    pub modules: Vec<crate::ProgramFingerprint>,
    pub frames: Vec<LinkedVmFrameSnapshot>,
}

pub struct LinkedVm {
    objects: crate::ObjectHeap,
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
            objects: crate::ObjectHeap::default(),
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
            Value::Function {
                module: Some(module),
                ..
            } => ModuleId(*module),
            Value::Function { module: None, .. } => {
                return Err(LinkedVmError::UnboundFunctionModule);
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
        let mut vm = Vm::from_callable(bytecode, callable, arguments)?;
        let mut objects = crate::ObjectHeap::default();
        vm.swap_objects(&mut objects);
        Ok(Self {
            objects,
            program,
            frames: vec![(module, vm)],
        })
    }

    pub fn program(&self) -> &LinkedProgram {
        &self.program
    }

    /// Host access policy, inherited by subsequently invoked script modules.
    pub fn set_read_only_globals(&mut self, names: std::collections::BTreeSet<String>) {
        for (_, vm) in &mut self.frames {
            vm.set_read_only_globals(names.clone());
        }
    }

    /// Make a fresh invocation's input object graph read-only without restricting
    /// objects subsequently allocated by the function itself.
    pub fn freeze_invocation_inputs(&mut self) -> Result<(), LinkedVmError> {
        let (_, vm) = self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?;
        vm.swap_objects(&mut self.objects);
        let result = vm.freeze_invocation_inputs();
        vm.swap_objects(&mut self.objects);
        result.map_err(Into::into)
    }

    /// A safe-point collection across all linked call frames, including host roots.
    pub fn collect_objects(&mut self, host_roots: &[Value]) -> Result<usize, VmError> {
        self.objects.collect(
            self.frames
                .iter()
                .flat_map(|(_, vm)| vm.object_roots())
                .chain(host_roots),
        )
    }

    /// Named globals of the currently executing frame, for embedding commit boundaries.
    pub fn current_globals(&self) -> std::collections::BTreeMap<String, Value> {
        let Some((_, vm)) = self.frames.last() else {
            return Default::default();
        };
        vm.bytecode()
            .globals
            .iter()
            .zip(vm.globals())
            .filter_map(|(symbol, value)| {
                vm.bytecode().symbols.resolve(*symbol).map(|name| {
                    (
                        name.to_owned(),
                        self.objects.export(value).unwrap_or_else(|_| value.clone()),
                    )
                })
            })
            .collect()
    }

    pub fn step(&mut self) -> Result<Option<LinkedVmEvent>, LinkedVmError> {
        self.step_with_budget(&mut 10_000)
    }

    pub fn step_with_budget(
        &mut self,
        remaining: &mut u32,
    ) -> Result<Option<LinkedVmEvent>, LinkedVmError> {
        loop {
            if self.objects.collection_due() {
                // LinkedVm exports owned values/portable closures at every host
                // boundary; no host-side object IDs refer into this heap.
                self.collect_objects(&[])?;
            }
            let is_root = self.frames.len() == 1;
            let (module_id, vm) = self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?;
            vm.swap_objects(&mut self.objects);
            let event = vm.step_with_budget(remaining);
            vm.swap_objects(&mut self.objects);
            if let Err(mut error) = event {
                if let VmError::Panic { frames, .. } = &mut error {
                    for (_, caller) in self.frames.iter().rev().skip(1) {
                        frames.extend(caller.stack_trace());
                    }
                }
                return Err(error.into());
            }
            let Some(event) = event? else {
                return Ok(None);
            };
            match event {
                VmEvent::BudgetExhausted => return Ok(Some(LinkedVmEvent::BudgetExhausted)),
                VmEvent::Statement(mut value) => {
                    if let StatementValue::Value(item) = &mut value {
                        *item = self.objects.export(item)?;
                    }
                    return Ok(Some(LinkedVmEvent::Statement(value)));
                }
                VmEvent::Completed(value) => {
                    let value = bind_value_module(value, *module_id);
                    // Retain the completed root's globals for the embedding's commit.
                    if is_root {
                        return Ok(Some(LinkedVmEvent::Completed(self.objects.export(&value)?)));
                    }
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
                                .map(|value| {
                                    self.objects
                                        .export(&value)
                                        .map(|value| bind_value_module(value, *module_id))
                                })
                                .transpose()?;
                            let arguments = call
                                .arguments
                                .into_iter()
                                .map(|mut argument| {
                                    argument.value = bind_value_module(
                                        self.objects.export(&argument.value)?,
                                        *module_id,
                                    );
                                    Ok(argument)
                                })
                                .collect::<Result<_, VmError>>()?;
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
                            let mut callee = Vm::from_function(bytecode, function, arguments)?;
                            callee.set_type_bindings(call.type_bindings);
                            callee.set_read_only_globals(vm.read_only_globals().clone());
                            self.frames.push((module, callee));
                        }
                        None => return Err(LinkedVmError::UnlinkedCall(call.function)),
                    }
                }
            }
        }
    }

    pub fn resume(&mut self, value: Value) -> Result<(), LinkedVmError> {
        let vm = &mut self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?.1;
        vm.swap_objects(&mut self.objects);
        let result = vm.resume(value);
        vm.swap_objects(&mut self.objects);
        result.map_err(Into::into)
    }

    /// Replaces the globals of the currently executing frame by symbol name.
    /// Embeddings use this to evaluate a saved closure against a newer model.
    pub fn set_current_globals(
        &mut self,
        values: &std::collections::BTreeMap<String, Value>,
    ) -> Result<(), LinkedVmError> {
        let (_, vm) = self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?;
        let globals = vm
            .bytecode()
            .globals
            .iter()
            .map(|symbol| {
                vm.bytecode()
                    .symbols
                    .resolve(*symbol)
                    .and_then(|name| values.get(name))
                    .cloned()
                    .unwrap_or(Value::Uninitialized)
            })
            .collect();
        vm.swap_objects(&mut self.objects);
        let result = vm.set_global_values(globals);
        vm.swap_objects(&mut self.objects);
        result?;
        Ok(())
    }

    pub fn snapshot(&self) -> LinkedVmSnapshot {
        LinkedVmSnapshot {
            objects: self.objects.clone(),
            modules: self
                .program
                .modules
                .iter()
                .map(|module| module.fingerprint.clone())
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
        program: LinkedProgram,
    ) -> Result<Self, LinkedVmError> {
        if snapshot.modules
            != program
                .modules
                .iter()
                .map(|module| module.fingerprint.clone())
                .collect::<Vec<_>>()
        {
            return Err(VmError::ProgramFingerprintMismatch.into());
        }
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
        Ok(Self {
            program,
            frames,
            objects: snapshot.objects,
        })
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
    UnboundFunctionModule,
}

fn bind_value_module(value: Value, module: ModuleId) -> Value {
    match value {
        Value::Function {
            module: owner,
            symbol,
        } => Value::Function {
            module: owner.or(Some(module.0)),
            symbol,
        },
        Value::Closure {
            type_bindings,
            module: owner,
            region,
            captures,
            objects,
        } => Value::Closure {
            type_bindings,
            objects,
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

impl std::fmt::Display for LinkedVmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Vm(error) = self {
            return std::fmt::Display::fmt(error, formatter);
        }
        write!(formatter, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        BuiltinId, BuiltinManifest, Bytecode, compile_with_manifest, link_register_modules,
        parse_program,
    };

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
        let mut vm = LinkedVm::new(program.clone(), ModuleId(1)).expect("entry starts");
        let Some(LinkedVmEvent::Call(call)) = vm.step().expect("native call yields") else {
            panic!("expected native call")
        };
        assert_eq!(call.builtin, BuiltinId(4));
        assert_eq!(call.arguments[0].value, Value::String("alice".into()));

        let snapshot = vm.snapshot();
        let mut restored = LinkedVm::restore(snapshot, program).expect("linked frames restore");
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
            Some(LinkedVmEvent::Statement(StatementValue::TextTemplate(
                "child".into()
            )))
        );
    }

    #[test]
    fn named_functions_keep_their_module_when_crossing_a_native_boundary() {
        let natives = BuiltinManifest::new([("capture", BuiltinId(10))]);
        let module = compile("fn read() -> String { \"value\" }\ncapture(read)", &natives);
        let program = link_register_modules(vec![module], &natives).expect("module links");
        let mut vm = LinkedVm::new(program, ModuleId(0)).expect("entry starts");
        let Some(LinkedVmEvent::Call(call)) = vm.step().expect("capture yields") else {
            panic!("expected native capture call")
        };
        let function = call.arguments[0].value.clone();
        assert!(matches!(
            function,
            Value::Function {
                module: Some(0),
                ..
            }
        ));

        let mut child = LinkedVm::from_callable(vm.program().clone(), &function, Vec::new())
            .expect("bound function invokes");
        assert_eq!(
            child.step().expect("function executes"),
            Some(LinkedVmEvent::Completed(Value::String("value".into())))
        );
    }
}
