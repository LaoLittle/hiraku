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
    pub module_globals: std::collections::BTreeMap<u32, Vec<Value>>,
    pub modules: Vec<crate::ProgramFingerprint>,
    pub frames: Vec<LinkedVmFrameSnapshot>,
}

pub struct LinkedVm {
    objects: crate::ObjectHeap,
    module_globals: std::collections::BTreeMap<u32, crate::RegisterFrame>,
    program: LinkedProgram,
    frames: Vec<(ModuleId, Vm)>,
}

/// Shared dispatch for linked executors. Module ownership is interpreted once,
/// before any region or symbol is looked up in a bytecode table.
pub enum PreparedInvocation {
    Script { module: ModuleId, vm: Vm },
    Native(BuiltinCall),
}

pub fn prepare_invocation(
    program: &LinkedProgram,
    callable: Value,
    mut arguments: Vec<crate::CallArgument>,
    objects: &mut crate::ObjectHeap,
) -> Result<PreparedInvocation, LinkedVmError> {
    let owner = match &callable {
        Value::Function {
            module: Some(owner),
            ..
        }
        | Value::Closure {
            module: Some(owner),
            ..
        } => ModuleId(*owner),
        _ => {
            return Err(VmError::TypeMismatch(
                "linked invocation requires a module-owned callable",
            )
            .into());
        }
    };
    let source = program
        .modules
        .get(owner.0 as usize)
        .ok_or(LinkedVmError::UnknownModule(owner))?;
    let target = match &callable {
        Value::Function { symbol, .. } => Some(
            source
                .bytecode
                .functions
                .iter()
                .position(|function| function.name == *symbol)
                .map(|function| LinkedFunction::Script {
                    module: owner,
                    function: function as u32,
                })
                .or_else(|| source.resolve(*symbol))
                .ok_or(LinkedVmError::UnlinkedCall(*symbol))?,
        ),
        _ => None,
    };
    if let Some(LinkedFunction::Native(builtin)) = target {
        return Ok(PreparedInvocation::Native(BuiltinCall {
            builtin,
            receiver: None,
            arguments,
        }));
    }
    // Import portable graphs into the execution's shared heap, not a temporary
    // callee heap which will be swapped out before the callee starts.
    for argument in &mut arguments {
        argument.value = objects.import(argument.value.clone());
    }
    let values = arguments
        .into_iter()
        .map(|argument| argument.value)
        .collect();
    let (module, vm) = match target {
        Some(LinkedFunction::Script { module, function }) => {
            let code = program
                .modules
                .get(module.0 as usize)
                .ok_or(LinkedVmError::UnknownModule(module))?
                .bytecode
                .clone();
            (module, Vm::from_function(code, function, values)?)
        }
        None => {
            let callable = objects.import(callable);
            (
                owner,
                Vm::from_callable(source.bytecode.clone(), &callable, values)?,
            )
        }
        Some(LinkedFunction::Native(_)) => unreachable!("native call returned above"),
    };
    Ok(PreparedInvocation::Script { module, vm })
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
            module_globals: Default::default(),
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
        let _owner = match callable {
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
        let mut objects = crate::ObjectHeap::default();
        let prepared = prepare_invocation(
            &program,
            callable.clone(),
            arguments
                .into_iter()
                .map(|value| crate::CallArgument { label: None, value })
                .collect(),
            &mut objects,
        )?;
        let PreparedInvocation::Script { module, vm } = prepared else {
            return Err(LinkedVmError::Vm(VmError::TypeMismatch(
                "an independent VM entry requires a script callable",
            )));
        };
        Ok(Self {
            objects,
            module_globals: Default::default(),
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

    /// Check entry arguments before any script statement or host effect executes.
    pub fn validate_invocation(&mut self) -> Result<(), LinkedVmError> {
        let (_, vm) = self.frames.last_mut().ok_or(LinkedVmError::NoFrame)?;
        vm.swap_objects(&mut self.objects);
        let result = vm.validate_function_arguments();
        vm.swap_objects(&mut self.objects);
        result.map_err(Into::into)
    }

    /// A safe-point collection across all linked call frames, including host roots.
    pub fn collect_objects(&mut self, host_roots: &[Value]) -> Result<usize, VmError> {
        let roots: Vec<_> = self
            .frames
            .iter()
            .flat_map(|(_, vm)| vm.object_roots())
            .chain(
                self.module_globals
                    .values()
                    .flat_map(|frame| frame.values()),
            )
            .collect();
        self.objects.collect(roots.iter().chain(host_roots))
    }

    /// Named globals of the currently executing frame, for embedding commit boundaries.
    pub fn current_globals(
        &self,
    ) -> Result<std::collections::BTreeMap<String, Value>, LinkedVmError> {
        let Some((_, vm)) = self.frames.last() else {
            return Err(LinkedVmError::NoFrame);
        };
        vm.bytecode()
            .globals
            .iter()
            .zip(vm.globals())
            .filter_map(|(symbol, value)| {
                vm.bytecode().symbols.resolve(*symbol).map(|name| {
                    Ok((
                        name.to_owned(),
                        self.objects.export(&value).map_err(|error| {
                            LinkedVmError::GlobalExport {
                                name: name.to_owned(),
                                error,
                            }
                        })?,
                    ))
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
            vm.set_module(*module_id);
            vm.swap_objects(&mut self.objects);
            let event = vm.step_with_budget(remaining);
            vm.swap_objects(&mut self.objects);
            self.module_globals
                .insert(module_id.0, vm.compact_globals());
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
                VmEvent::Invoke {
                    callable,
                    arguments,
                } => {
                    let caller_module = *module_id;
                    let arguments = arguments
                        .into_iter()
                        .map(|mut argument| {
                            argument.value = bind_value_module(argument.value, caller_module);
                            argument
                        })
                        .collect();
                    let policy = vm.read_only_globals().clone();
                    match prepare_invocation(&self.program, callable, arguments, &mut self.objects)?
                    {
                        PreparedInvocation::Script { module, mut vm } => {
                            vm.set_read_only_globals(policy);
                            if let Some(globals) = self.module_globals.get(&module.0) {
                                vm.set_compact_globals(globals)?;
                            }
                            self.frames.push((module, vm));
                        }
                        PreparedInvocation::Native(mut call) => {
                            for argument in &mut call.arguments {
                                argument.value = self.objects.export(&argument.value)?;
                            }
                            return Ok(Some(LinkedVmEvent::Call(call)));
                        }
                    }
                }
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
                    if let Some((caller_module, caller)) = self.frames.last_mut() {
                        if let Some(globals) = self.module_globals.get(&caller_module.0) {
                            caller.set_compact_globals(globals)?;
                        }
                        caller.resume(value)?;
                        continue;
                    }
                    return Ok(Some(LinkedVmEvent::Completed(value)));
                }
                VmEvent::Call(mut call) => {
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
                            call.resolve_literal_arguments(
                                &bytecode.functions[function as usize].signature,
                            );
                            let arguments = call
                                .arguments
                                .into_iter()
                                .map(|argument| bind_value_module(argument.value, *module_id))
                                .collect();
                            let mut callee = Vm::from_function(bytecode, function, arguments)?;
                            callee.set_type_bindings(call.type_bindings);
                            callee.set_read_only_globals(vm.read_only_globals().clone());
                            if let Some(globals) = self.module_globals.get(&module.0) {
                                callee.set_compact_globals(globals)?;
                            }
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
            module_globals: self
                .module_globals
                .iter()
                .map(|(id, frame)| (*id, frame.values().collect()))
                .collect(),
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
        if snapshot.frames.is_empty() || snapshot.frames.len() > 1024 {
            return Err(VmError::InvalidSnapshot(
                "linked execution requires between 1 and 1024 frames".into(),
            )
            .into());
        }
        for frame in snapshot.frames.iter().take(snapshot.frames.len() - 1) {
            if frame.vm.status != crate::vm::VmStatus::WaitingForHost {
                return Err(VmError::InvalidSnapshot(
                    "linked caller is not suspended awaiting its callee".into(),
                )
                .into());
            }
        }
        for (id, globals) in &snapshot.module_globals {
            let module = program
                .modules
                .get(*id as usize)
                .ok_or(LinkedVmError::UnknownModule(ModuleId(*id)))?;
            if globals.len() != module.bytecode.globals.len() {
                return Err(VmError::FrameShapeMismatch.into());
            }
        }
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
            module_globals: snapshot
                .module_globals
                .into_iter()
                .map(|(id, values)| {
                    (
                        id,
                        crate::RegisterFrame::from_values(values, Default::default()),
                    )
                })
                .collect(),
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
    GlobalExport { name: String, error: VmError },
}

pub fn bind_value_module(value: Value, module: ModuleId) -> Value {
    match value {
        Value::TextTemplate(mut template) => {
            template.captures = template
                .captures
                .iter()
                .map(|(name, value)| (name.clone(), bind_value_module(value.clone(), module)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into();
            Value::TextTemplate(template)
        }
        Value::Optional(value) => {
            Value::Optional(value.map(|value| Box::new(bind_value_module(*value, module))))
        }
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

impl std::error::Error for LinkedVmError {}

impl std::fmt::Display for LinkedVmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Vm(error) = self {
            return std::fmt::Display::fmt(error, formatter);
        }
        if let Self::GlobalExport { name, error } = self {
            return write!(formatter, "cannot export global `{name}`: {error}");
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

    #[test]
    fn independent_imported_function_entry_uses_linker_relocation() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let project = crate::compile_project(
            [
                ("library.hks", "global fn answer() -> Int { 17 }"),
                ("main.hks", "global let callback = answer"),
            ]
            .into_iter()
            .map(|(path, source)| crate::ScriptSource {
                path: path.into(),
                source: source.into(),
                namespace: None,
            })
            .collect(),
            &natives,
        )
        .expect("project");
        let mut vm = LinkedVm::new(project.program.clone(), project.paths["main.hks"]).expect("VM");
        for _ in 0..100 {
            if matches!(
                vm.step().expect("initialize"),
                Some(LinkedVmEvent::Completed(_))
            ) {
                break;
            }
        }
        let callback = vm
            .current_globals()
            .expect("export")
            .remove("callback")
            .expect("callback");
        let mut invocation = LinkedVm::from_callable(project.program, &callback, vec![])
            .expect("linked function entry");
        for _ in 0..100 {
            if let Some(LinkedVmEvent::Completed(value)) = invocation.step().expect("invoke") {
                assert_eq!(value, Value::Int(17));
                return;
            }
        }
        panic!("expected completion");
    }

    #[test]
    fn module_owned_callbacks_preserve_code_captures_and_yield_checkpoints() {
        let mut natives = crate::native::NativeRegistry::<Vec<i64>>::new();
        natives
            .register_fn("record", |output: &mut Vec<i64>, value: i64| {
                output.push(value);
                Ok(())
            })
            .expect("native");
        let sources = [
            (
                "library.hks",
                r#"
                let decoy: () -> Int = { return 99 }
                global fn apply(callback: () -> Int) -> Int { callback() }
                global struct CallbackBox { callback: () -> Int }
                global fn applyBox(value: CallbackBox) -> Int { value.callback() }
                global fn applyNative(callback: (Int) -> Unit) { callback(13) }
                global fn make() -> () -> Int {
                    let captured = 9
                    { return captured }
                }
            "#,
            ),
            (
                "main.hks",
                r#"
                let unused = 0
                let callback: () -> Int = { return 7 }
                record(apply(callback))
                let state = .{ count: 1 }
                let increment: () -> Int = { state.count += 1; state.count }
                record(apply(increment))
                record(state.count)
                let returned = make()
                record(returned())
                fn privateValue() -> Int { 11 }
                record(apply(privateValue))
                let box = CallbackBox.{ callback: callback }
                record(applyBox(box))
                applyNative(record)
            "#,
            ),
        ]
        .into_iter()
        .map(|(path, source)| crate::ScriptSource {
            path: path.into(),
            namespace: None,
            source: source.into(),
        })
        .collect();
        let project =
            crate::compile_project(sources, &natives.manifest()).expect("project compiles");
        let mut vm = LinkedVm::new(project.program.clone(), project.paths["main.hks"]).expect("VM");
        let mut output = Vec::new();
        for _ in 0..2000 {
            let event = vm.step_with_budget(&mut 1).expect("callback step");
            let bytes = crate::hson::to_vec(&vm.snapshot()).expect("serialize checkpoint");
            vm = LinkedVm::restore(
                crate::hson::from_slice(&bytes).expect("decode checkpoint"),
                project.program.clone(),
            )
            .expect("restore checkpoint");
            match event {
                Some(LinkedVmEvent::Call(call)) => {
                    let value = natives.call(&mut output, &call).expect("native call");
                    vm.resume(value).expect("resume");
                }
                Some(LinkedVmEvent::Completed(_)) => {
                    assert_eq!(output, [7, 2, 2, 9, 11, 7, 13]);
                    return;
                }
                _ => {}
            }
        }
        panic!("expected completion");
    }

    #[test]
    fn cyclic_global_export_is_an_error_not_a_raw_object_id() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile("global var value: Any = 1", &natives);
        let program = link_register_modules(vec![code], &natives).expect("link");
        let mut vm = LinkedVm::new(program, ModuleId(0)).expect("VM");
        let reference = vm.objects.allocate(Value::Unit);
        let Value::Object(id) = reference else {
            panic!("object reference");
        };
        vm.objects
            .replace(
                id,
                Value::Map(std::collections::BTreeMap::from([(
                    "self".into(),
                    reference.clone(),
                )])),
            )
            .expect("create cycle");
        vm.frames[0]
            .1
            .set_global_values(vec![reference])
            .expect("global");
        assert!(
            matches!(vm.current_globals(), Err(LinkedVmError::GlobalExport { name, error: VmError::CyclicHostValue }) if name == "value")
        );
    }

    #[test]
    fn linked_restore_rejects_invalid_frame_and_global_tables() {
        let natives = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile("global var value = 1", &natives);
        let program = link_register_modules(vec![code], &natives).expect("link");
        let vm = LinkedVm::new(program.clone(), ModuleId(0)).expect("VM");
        let saved = vm.snapshot();
        let mut invalid = saved.clone();
        invalid.frames.clear();
        assert!(LinkedVm::restore(invalid, program.clone()).is_err());
        let mut invalid = saved.clone();
        invalid.frames.push(invalid.frames[0].clone());
        assert!(LinkedVm::restore(invalid, program.clone()).is_err());
        let mut invalid = saved.clone();
        invalid.module_globals.insert(u32::MAX, vec![]);
        assert!(LinkedVm::restore(invalid, program.clone()).is_err());
        let mut invalid = saved;
        invalid.module_globals.insert(0, vec![]);
        assert!(LinkedVm::restore(invalid, program).is_err());
    }

    fn compile(source: &str, manifest: &BuiltinManifest) -> Bytecode {
        compile_with_manifest(&parse_program(source).expect("source parses"), 31, manifest)
            .expect("source compiles")
    }

    #[test]
    fn project_links_protocol_witnesses_for_structs_and_restores_calls() {
        let mut natives = crate::native::NativeRegistry::<Vec<String>>::new();
        natives
            .register_fn("print", |output: &mut Vec<String>, message: String| {
                output.push(message);
                Ok(())
            })
            .expect("register typed print");
        let project = crate::compile_project(
            vec![crate::ScriptSource {
                path: "main.hks".into(),
                namespace: None,
                source: r#"
                struct TestFoo { name: String? }
                protocol Test { fn test(self) -> Unit }
                extend TestFoo: Test {
                    fn test(self) { print("name: ${self.name ?: "no way"}") }
                }
                extend TestFoo: Colon<String> {
                    type Output = Unit
                    fn colon(self, rhs: String) {
                        self.test()
                        print("rhs: ${rhs}")
                    }
                }
                fn what<T: Test>(t: T) { t.test() }
                fn forward<U: Test>(value: U) { what(value) }
                let s: TestFoo = .{ name: "Alice" }
                s: "123"
                what(s)
                forward(s)
                what(TestFoo.{ name: null })
            "#
                .into(),
            }],
            &natives.manifest(),
        )
        .expect("generic protocol calls link through the project pipeline");
        let program = project.program;
        let mut vm = LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
        let mut output = Vec::new();
        let mut completed = false;
        for _ in 0..100 {
            match vm.step().expect("linked generic call") {
                Some(LinkedVmEvent::Call(call)) => {
                    vm = LinkedVm::restore(vm.snapshot(), program.clone())
                        .expect("restore witness call");
                    let value = natives
                        .call(&mut output, &call)
                        .expect("typed native dispatch");
                    vm.resume(value).expect("resume witness call");
                }
                Some(LinkedVmEvent::Completed(_)) => {
                    completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(completed, "program must terminate");
        assert_eq!(
            output,
            [
                "name: Alice",
                "rhs: 123",
                "name: Alice",
                "name: Alice",
                "name: no way"
            ]
        );
    }

    #[test]
    fn executes_and_restores_cross_module_global_calls() {
        let natives = BuiltinManifest::new([("nativeEcho", BuiltinId(4))]);
        let provider = compile(
            "global fn greet(name: String) { nativeEcho(name) }",
            &natives,
        );
        let consumer = compile(
            "greet(\"alice\")",
            &BuiltinManifest::new([("greet", BuiltinId(100))]),
        );
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
