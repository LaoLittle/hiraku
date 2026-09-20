//! Unified execution model for a linked story program.
//!
//! Every running VM, including the root program and engine-created closures,
//! lives in the same execution table and crosses the same host event boundary.

use std::{collections::BTreeMap, fmt};

use hiraku_script::{
    BuiltinCall, Bytecode, LinkedBytecode, LinkedFunction, StatementValue, TemplateError, Value,
    Vm, VmError, VmEvent, VmSnapshot, VmStatus,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionId(u64);

impl ExecutionId {
    pub const MAIN: Self = Self(0);

    pub const fn is_main(self) -> bool {
        self.0 == Self::MAIN.0
    }

    pub const fn from_task_handle(value: u64) -> Self {
        Self(value)
    }

    pub const fn task_handle(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_main() {
            formatter.write_str("main")
        } else {
            write!(formatter, "{}", self.0)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    Main,
    Interactive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionEvent {
    Call {
        execution: ExecutionId,
        call: BuiltinCall,
    },
    Statement {
        execution: ExecutionId,
        value: StatementValue,
    },
    Completed {
        execution: ExecutionId,
        value: Value,
    },
}

impl ExecutionEvent {
    pub const fn execution(&self) -> ExecutionId {
        match self {
            Self::Call { execution, .. }
            | Self::Statement { execution, .. }
            | Self::Completed { execution, .. } => *execution,
        }
    }
}

struct ExecutionState {
    vm: Vm,
    module: hiraku_script::ModuleId,
    callers: Vec<(hiraku_script::ModuleId, Vm)>,
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ExecutionSnapshot {
    vm: VmSnapshot,
    module: hiraku_script::ModuleId,
    callers: Vec<(hiraku_script::ModuleId, VmSnapshot)>,
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRuntimeSnapshot {
    objects: hiraku_script::ObjectHeap,
    /// The save contains state only. Code must be recompiled and match exactly.
    pub program: hiraku_script::ProgramFingerprint,
    modules: Vec<hiraku_script::ProgramFingerprint>,
    next_execution: u64,
    executions: BTreeMap<ExecutionId, ExecutionSnapshot>,
    module_globals: BTreeMap<String, Value>,
}

pub struct ExecutionRuntime {
    objects: hiraku_script::ObjectHeap,
    linked: LinkedBytecode,
    program: hiraku_script::LinkedProgram,
    entry: hiraku_script::ModuleId,
    module_globals: BTreeMap<String, Value>,
    executions: BTreeMap<ExecutionId, ExecutionState>,
    next_execution: u64,
    globals: BTreeMap<String, Value>,
}

impl ExecutionRuntime {
    pub(super) fn has_executions(&self) -> bool {
        !self.executions.is_empty()
    }
    pub(super) fn resource_positions(&self) -> Vec<(String, usize)> {
        self.executions
            .values()
            .flat_map(|state| {
                std::iter::once(&state.vm).chain(state.callers.iter().map(|(_, vm)| vm))
            })
            .flat_map(|vm| vm.source_positions())
            .map(|(path, span)| (path.to_owned(), span.start))
            .collect()
    }
    pub fn program_for_path(&self, path: &str) -> Option<super::StoryProgram> {
        let entry = self
            .program
            .modules
            .iter()
            .find(|module| {
                module
                    .bytecode
                    .debug
                    .source
                    .as_ref()
                    .is_some_and(|source| source.path == path)
            })?
            .id;
        Some(super::StoryProgram::Project {
            program: self.program.clone(),
            entry,
        })
    }

    pub fn new(code: impl Into<super::StoryProgram>) -> Result<Self, ExecutionRuntimeError> {
        let (program, entry) = code.into().link()?;
        let linked = program.modules[entry.0 as usize].clone();
        let bytecode = linked.bytecode.clone();
        let mut executions = BTreeMap::new();
        executions.insert(
            ExecutionId::MAIN,
            ExecutionState {
                vm: Vm::new(bytecode)?,
                module: entry,
                callers: Vec::new(),
                mode: ExecutionMode::Main,
                paused: false,
            },
        );
        Ok(Self {
            objects: hiraku_script::ObjectHeap::default(),
            linked,
            program,
            entry,
            module_globals: BTreeMap::new(),
            executions,
            next_execution: 1,
            globals: BTreeMap::new(),
        })
    }

    pub fn restore(
        code: impl Into<super::StoryProgram>,
        snapshot: ExecutionRuntimeSnapshot,
    ) -> Result<Self, ExecutionRuntimeError> {
        let (program, entry) = code.into().link()?;
        let linked = program.modules[entry.0 as usize].clone();
        if linked.fingerprint != snapshot.program
            || program
                .modules
                .iter()
                .map(|module| module.fingerprint.clone())
                .collect::<Vec<_>>()
                != snapshot.modules
        {
            return Err(VmError::ProgramFingerprintMismatch.into());
        }
        let executions = snapshot
            .executions
            .into_iter()
            .map(|(id, state)| {
                let code = program
                    .modules
                    .get(state.module.0 as usize)
                    .ok_or(VmError::ProgramFingerprintMismatch)?
                    .bytecode
                    .clone();
                let callers = state
                    .callers
                    .into_iter()
                    .map(|(module, snapshot)| {
                        let code = program
                            .modules
                            .get(module.0 as usize)
                            .ok_or(VmError::ProgramFingerprintMismatch)?
                            .bytecode
                            .clone();
                        Ok((module, Vm::restore(code, snapshot)?))
                    })
                    .collect::<Result<Vec<_>, VmError>>()?;
                Vm::restore(code, state.vm).map(|vm| {
                    (
                        id,
                        ExecutionState {
                            vm,
                            module: state.module,
                            callers,
                            mode: state.mode,
                            paused: state.paused,
                        },
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        let globals = snapshot
            .module_globals
            .clone()
            .into_iter()
            .map(|(key, value)| Ok((key, snapshot.objects.export(&value)?)))
            .collect::<Result<_, hiraku_script::VmError>>()?;
        Ok(Self {
            linked,
            program,
            entry,
            module_globals: snapshot.module_globals,
            executions,
            next_execution: snapshot.next_execution,
            objects: snapshot.objects,
            globals,
        })
    }

    pub fn snapshot(&self) -> ExecutionRuntimeSnapshot {
        ExecutionRuntimeSnapshot {
            objects: self.objects.clone(),
            program: self.linked.fingerprint.clone(),
            modules: self
                .program
                .modules
                .iter()
                .map(|module| module.fingerprint.clone())
                .collect(),
            module_globals: self.module_globals.clone(),
            next_execution: self.next_execution,
            executions: self
                .executions
                .iter()
                .map(|(id, state)| {
                    (
                        *id,
                        ExecutionSnapshot {
                            vm: state.vm.snapshot(),
                            module: state.module,
                            callers: state
                                .callers
                                .iter()
                                .map(|(module, vm)| (*module, vm.snapshot()))
                                .collect(),
                            mode: state.mode,
                            paused: state.paused,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Drop a child continuation. External effects remain owned by the host.
    pub fn cancel_child(&mut self, execution: ExecutionId) -> bool {
        !execution.is_main() && self.executions.remove(&execution).is_some()
    }

    pub fn spawn(
        &mut self,
        closure: &Value,
        mode: ExecutionMode,
    ) -> Result<ExecutionId, ExecutionRuntimeError> {
        if mode == ExecutionMode::Main {
            return Err(ExecutionRuntimeError::InvalidChildMode);
        }
        let execution = ExecutionId(self.next_execution);
        self.next_execution = self
            .next_execution
            .checked_add(1)
            .expect("story execution identifier space must not be exhausted");
        // Import portable captures into the execution-owned heap before creating
        // the child. All story executions must address the same object table.
        let closure = self.objects.import(closure.clone());
        let module = match &closure {
            Value::Closure {
                module: Some(module),
                ..
            }
            | Value::Function {
                module: Some(module),
                ..
            } => hiraku_script::ModuleId(*module),
            _ => self.entry,
        };
        let code = self
            .program
            .modules
            .get(module.0 as usize)
            .ok_or(VmError::ProgramFingerprintMismatch)?
            .bytecode
            .clone();
        let mut vm = Vm::from_callable(code.clone(), &closure, Vec::new())?;
        vm.set_global_values(values_from_globals(&code, &self.module_globals))?;
        self.executions.insert(
            execution,
            ExecutionState {
                vm,
                module,
                callers: Vec::new(),
                mode,
                paused: false,
            },
        );
        Ok(execution)
    }

    /// Advances the root execution first, then the first ready child.
    #[cfg(test)]
    pub fn step(&mut self) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        self.step_with_budget(&mut 10_000)
    }

    pub fn step_with_budget(
        &mut self,
        budget: &mut u32,
    ) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        if self.executions.contains_key(&ExecutionId::MAIN)
            && let Some(event) = self.step_execution(ExecutionId::MAIN, budget)?
        {
            return Ok(Some(event));
        }
        self.step_children_with_budget(budget)
    }

    /// Advances children while the story policy keeps the root host-blocked.
    #[cfg(test)]
    pub fn step_children(&mut self) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        self.step_children_with_budget(&mut 10_000)
    }

    pub fn step_children_with_budget(
        &mut self,
        budget: &mut u32,
    ) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        let ready = self
            .executions
            .iter()
            .filter(|(id, state)| !id.is_main() && !state.paused)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for execution in ready {
            if let Some(event) = self.step_execution(execution, budget)? {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }

    pub(super) fn step_execution(
        &mut self,
        execution: ExecutionId,
        budget: &mut u32,
    ) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .vm
            .swap_objects(&mut self.objects);
        let result = self.step_execution_inner(execution, budget);
        if let Some(state) = self.executions.get_mut(&execution) {
            state.vm.swap_objects(&mut self.objects);
        }
        self.refresh_globals();
        let mut event = result?;
        if let Some(ExecutionEvent::Call { call, .. }) = &mut event {
            for value in call.receiver.iter_mut().chain(
                call.arguments
                    .iter_mut()
                    .map(|argument| &mut argument.value),
            ) {
                // Scheduled story closures remain in this execution's heap.
                if !matches!(value, Value::Closure { .. } | Value::Function { .. }) {
                    *value = self.objects.export(value)?;
                }
            }
        }
        Ok(event)
    }

    fn step_execution_inner(
        &mut self,
        execution: ExecutionId,
        budget: &mut u32,
    ) -> Result<Option<ExecutionEvent>, ExecutionRuntimeError> {
        loop {
            let event = {
                let state = self
                    .executions
                    .get_mut(&execution)
                    .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                if state.paused {
                    return Ok(None);
                }
                state.vm.set_module(state.module);
                state.vm.set_global_values(values_from_globals(
                    state.vm.bytecode(),
                    &self.module_globals,
                ))?;
                state.vm.step_with_budget(budget).map_err(|mut error| {
                    if let VmError::Panic { frames, .. } = &mut error {
                        for (_, caller) in state.callers.iter().rev() {
                            frames.extend(caller.stack_trace());
                        }
                    }
                    error
                })?
            };
            let Some(event) = event else {
                return Ok(None);
            };

            return match event {
                VmEvent::Invoke {
                    callable,
                    arguments,
                } => {
                    self.capture_globals(execution)?;
                    let state = self
                        .executions
                        .get_mut(&execution)
                        .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                    let arguments = arguments
                        .into_iter()
                        .map(|mut argument| {
                            argument.value = hiraku_script::linked_vm::bind_value_module(
                                argument.value,
                                state.module,
                            );
                            argument
                        })
                        .collect();
                    state.vm.swap_objects(&mut self.objects);
                    let prepared = hiraku_script::linked_vm::prepare_invocation(
                        &self.program,
                        callable,
                        arguments,
                        &mut self.objects,
                    );
                    state.vm.swap_objects(&mut self.objects);
                    match prepared? {
                        hiraku_script::linked_vm::PreparedInvocation::Script {
                            module,
                            vm: mut callee,
                        } => {
                            callee.set_read_only_globals(state.vm.read_only_globals().clone());
                            callee.set_global_values(values_from_globals(
                                callee.bytecode(),
                                &self.module_globals,
                            ))?;
                            state.vm.swap_objects(&mut self.objects);
                            callee.swap_objects(&mut self.objects);
                            let caller = std::mem::replace(&mut state.vm, callee);
                            state.callers.push((state.module, caller));
                            state.module = module;
                            continue;
                        }
                        hiraku_script::linked_vm::PreparedInvocation::Native(mut call) => {
                            evaluate_call_templates(&mut call, |text| {
                                state
                                    .vm
                                    .eval_template_value_with(text, |source| Ok(source.to_owned()))
                            })?;
                            Ok(Some(ExecutionEvent::Call { execution, call }))
                        }
                    }
                }
                VmEvent::BudgetExhausted => {
                    self.capture_globals(execution)?;
                    Ok(None)
                }
                VmEvent::Call(mut call) => {
                    self.capture_globals(execution)?;
                    let state = self
                        .executions
                        .get_mut(&execution)
                        .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                    let module = &self.program.modules[state.module.0 as usize];
                    let target = module
                        .resolve(call.function)
                        .ok_or(ExecutionRuntimeError::UnlinkedCall(call.function))?;
                    let bind =
                        |value| hiraku_script::linked_vm::bind_value_module(value, state.module);
                    let mut call = match target {
                        LinkedFunction::Native(builtin) => BuiltinCall {
                            builtin,
                            receiver: call.receiver.map(bind),
                            arguments: call
                                .arguments
                                .into_iter()
                                .map(|mut arg| {
                                    arg.value = bind(arg.value);
                                    arg
                                })
                                .collect(),
                        },
                        LinkedFunction::Script { module, function } => {
                            if call.receiver.is_some() {
                                return Err(VmError::TypeMismatch(
                                    "external script function cannot have a native receiver",
                                )
                                .into());
                            }
                            let code = self.program.modules[module.0 as usize].bytecode.clone();
                            call.resolve_literal_arguments(
                                &code.functions[function as usize].signature,
                            );
                            let mut callee = Vm::from_function(
                                code.clone(),
                                function,
                                call.arguments
                                    .into_iter()
                                    .map(|arg| bind(arg.value))
                                    .collect(),
                            )?;
                            callee.set_type_bindings(call.type_bindings);
                            callee.set_global_values(values_from_globals(
                                &code,
                                &self.module_globals,
                            ))?;
                            // The current VM owns the shared heap during this step.
                            state.vm.swap_objects(&mut self.objects);
                            callee.swap_objects(&mut self.objects);
                            let caller = std::mem::replace(&mut state.vm, callee);
                            state.callers.push((state.module, caller));
                            state.module = module;
                            continue;
                        }
                    };
                    let state = self
                        .executions
                        .get(&execution)
                        .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                    evaluate_call_templates(&mut call, |text| {
                        state
                            .vm
                            .eval_template_value_with(text, |source| Ok(source.to_owned()))
                    })?;
                    Ok(Some(ExecutionEvent::Call { execution, call }))
                }
                VmEvent::Statement(value) => {
                    let value = {
                        let state = self
                            .executions
                            .get_mut(&execution)
                            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                        evaluate_statement_template(&mut state.vm, value)?
                    };
                    self.capture_globals(execution)?;
                    Ok(Some(ExecutionEvent::Statement { execution, value }))
                }
                VmEvent::Completed(value) => {
                    self.capture_globals(execution)?;
                    let state = self
                        .executions
                        .get_mut(&execution)
                        .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                    if let Some((module, mut caller)) = state.callers.pop() {
                        let value =
                            hiraku_script::linked_vm::bind_value_module(value, state.module);
                        state.vm.swap_objects(&mut self.objects);
                        caller.swap_objects(&mut self.objects);
                        caller.resume(value)?;
                        state.vm = caller;
                        state.module = module;
                        continue;
                    }
                    let mut state = self
                        .executions
                        .remove(&execution)
                        .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                    state.vm.swap_objects(&mut self.objects);
                    Ok(Some(ExecutionEvent::Completed { execution, value }))
                }
            };
        }
    }

    pub fn resume(
        &mut self,
        execution: ExecutionId,
        value: Value,
    ) -> Result<(), ExecutionRuntimeError> {
        let vm = &mut self
            .executions
            .get_mut(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .vm;
        vm.swap_objects(&mut self.objects);
        let result = vm.resume(value);
        vm.swap_objects(&mut self.objects);
        result.map_err(Into::into)
    }

    pub fn pause(&mut self, execution: ExecutionId) -> Result<(), ExecutionRuntimeError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .paused = true;
        Ok(())
    }

    pub fn unpause(&mut self, execution: ExecutionId) -> Result<(), ExecutionRuntimeError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .paused = false;
        Ok(())
    }

    pub fn mode(&self, execution: ExecutionId) -> Option<ExecutionMode> {
        self.executions.get(&execution).map(|state| state.mode)
    }

    pub fn is_waiting_for_host(&self, execution: ExecutionId) -> bool {
        self.executions
            .get(&execution)
            .is_some_and(|state| matches!(state.vm.status(), VmStatus::WaitingForHost))
    }

    pub fn set_globals(&mut self, globals: BTreeMap<String, Value>) {
        for (name, value) in globals {
            let old = self
                .module_globals
                .get(&name)
                .cloned()
                .unwrap_or(Value::Uninitialized);
            let value = if self
                .objects
                .export(&old)
                .is_ok_and(|current| current == value)
            {
                old
            } else {
                self.objects
                    .update(&old, value)
                    .expect("host globals refer to live objects")
            };
            self.module_globals.insert(name, value);
        }
        for state in self.executions.values_mut() {
            state
                .vm
                .set_global_values(values_from_globals(
                    state.vm.bytecode(),
                    &self.module_globals,
                ))
                .expect("compiled global frame shape must match its bytecode");
        }
        self.refresh_globals();
    }

    pub fn globals(&self) -> &BTreeMap<String, Value> {
        &self.globals
    }

    pub fn export_value(&self, value: &Value) -> Result<Value, ExecutionRuntimeError> {
        Ok(self.objects.export(value)?)
    }

    pub fn collect_objects_if_due(
        &mut self,
        host_roots: &[&Value],
    ) -> Result<(), ExecutionRuntimeError> {
        if self.objects.collection_due() {
            let roots: Vec<_> = self
                .executions
                .values()
                .flat_map(|state| {
                    state
                        .vm
                        .object_roots()
                        .chain(state.callers.iter().flat_map(|(_, vm)| vm.object_roots()))
                })
                .collect();
            self.objects.collect(
                roots
                    .iter()
                    .chain(self.module_globals.values())
                    .chain(host_roots.iter().copied()),
            )?;
        }
        Ok(())
    }

    fn capture_globals(&mut self, execution: ExecutionId) -> Result<(), ExecutionRuntimeError> {
        let vm = &self
            .executions
            .get(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .vm;
        self.module_globals
            .extend(globals_from_values(vm.bytecode(), &vm.globals()));
        Ok(())
    }

    fn refresh_globals(&mut self) {
        self.globals = self
            .module_globals
            .clone()
            .into_iter()
            .map(|(key, value)| (key, self.objects.export(&value).unwrap_or(value)))
            .collect();
    }
}

#[derive(Debug, Error)]
pub enum ExecutionRuntimeError {
    #[error("HKS callable dispatch failed: {0}")]
    Callable(#[from] hiraku_script::linked_vm::LinkedVmError),
    #[error("HKS VM failed: {0}")]
    Vm(VmError),
    #[error("unknown story execution {0}")]
    UnknownExecution(ExecutionId),
    #[error("the root execution mode cannot be used for a closure")]
    InvalidChildMode,
    #[error("HKS bytecode link failed: {0:?}")]
    Link(Vec<hiraku_script::LinkError>),
    #[error("HKS call references an unlinked symbol {0:?}")]
    UnlinkedCall(hiraku_script::SymbolId),
    #[error("HKS string template failed: {0}")]
    Template(#[from] TemplateError),
}

impl From<VmError> for ExecutionRuntimeError {
    fn from(error: VmError) -> Self {
        Self::Vm(error)
    }
}

fn evaluate_statement_template(
    vm: &mut Vm,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::TextTemplate(text) => Ok(StatementValue::String(vm.eval_template(&text)?)),
        StatementValue::Value(_) => Ok(StatementValue::Commit),
        value => Ok(value),
    }
}

fn evaluate_call_templates(
    call: &mut BuiltinCall,
    mut evaluate: impl FnMut(&hiraku_script::runtime::TemplateValue) -> Result<String, TemplateError>,
) -> Result<(), TemplateError> {
    if let Some(Value::TextTemplate(text)) = &mut call.receiver {
        *text = evaluate(text)?.into();
    }
    for argument in &mut call.arguments {
        if let Value::TextTemplate(text) = &mut argument.value {
            *text = evaluate(text)?.into();
        }
    }
    Ok(())
}

fn values_from_globals(bytecode: &Bytecode, globals: &BTreeMap<String, Value>) -> Vec<Value> {
    bytecode
        .globals
        .iter()
        .map(|symbol| {
            bytecode
                .symbols
                .resolve(*symbol)
                .and_then(|name| globals.get(name))
                .cloned()
                .unwrap_or(Value::Uninitialized)
        })
        .collect()
}

fn globals_from_values(bytecode: &Bytecode, values: &[Value]) -> BTreeMap<String, Value> {
    bytecode
        .globals
        .iter()
        .zip(values)
        .filter_map(|(symbol, value)| {
            (value != &Value::Uninitialized).then(|| {
                bytecode
                    .symbols
                    .resolve(*symbol)
                    .map(|name| (name.to_string(), value.clone()))
            })?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_callback_executes_in_its_owner_and_restores_host_wait() {
        let mut natives = hiraku_script::native::NativeRegistry::<Vec<i64>>::new();
        natives
            .register_fn("record", |output: &mut Vec<i64>, value: i64| {
                output.push(value);
                Ok(())
            })
            .expect("native");
        let project = hiraku_script::compile_project([
            ("library.hks", "let decoy: () -> Int = { 99 }\nglobal fn apply(callback: () -> Int) -> Int { callback() }"),
            ("main.hks", "let unused = 0\nlet callback: () -> Int = { record(5); 7 }\nrecord(apply(callback))"),
        ].into_iter().map(|(path, source)| hiraku_script::ScriptSource {
            path: path.into(), source: source.into(), namespace: None,
        }).collect(), &natives.manifest()).expect("project");
        let program = crate::script::StoryProgram::Project {
            entry: project.paths["main.hks"],
            program: project.program,
        };
        let mut runtime = ExecutionRuntime::new(program.clone()).expect("runtime");
        let mut output = Vec::new();
        for _ in 0..100 {
            match runtime.step().expect("execute callback") {
                Some(ExecutionEvent::Call { execution, call }) => {
                    let bytes = hiraku_script::hson::to_vec(&runtime.snapshot()).expect("snapshot");
                    runtime = ExecutionRuntime::restore(
                        program.clone(),
                        hiraku_script::hson::from_slice(&bytes).expect("decode"),
                    )
                    .expect("restore");
                    let value = natives.call(&mut output, &call).expect("native");
                    runtime.resume(execution, value).expect("resume");
                }
                Some(ExecutionEvent::Completed { .. }) => {
                    assert_eq!(output, [5, 7]);
                    return;
                }
                _ => {}
            }
        }
        panic!("expected completion");
    }

    #[test]
    fn extension_static_identity_survives_serialized_restore() {
        let bytecode = crate::script::compile_story_bytecode(
            "static_test.hks",
            r#"
                struct Counter { value: Int }
                extend Counter { let shared: Counter = .{ value: 1 } }
                let first = Counter.shared
                first.value = 42
                let second = Counter.shared
                if second.value != 42 { panic("static was recreated") }
                second.value = 7
                if first.value != 7 { panic("static lost its alias") }
            "#,
        )
        .expect("static story compiles");
        let mut runtime = ExecutionRuntime::new(bytecode.clone()).expect("runtime initializes");
        for _ in 0..500 {
            match runtime.step().expect("static story executes") {
                Some(ExecutionEvent::Completed { .. }) => return,
                Some(ExecutionEvent::Call { .. }) => panic!("unexpected host call"),
                _ => {}
            }
            let encoded = hiraku_script::hson::to_string(&runtime.snapshot())
                .expect("static state serializes");
            let snapshot =
                hiraku_script::hson::from_str(&encoded).expect("static state deserializes");
            runtime = ExecutionRuntime::restore(bytecode.clone(), snapshot)
                .expect("static state restores");
        }
        panic!("static story did not complete");
    }

    #[test]
    fn child_execution_shares_captured_records_after_snapshot_restore() {
        check_child_record_identity(false);
    }

    #[test]
    fn host_global_updates_preserve_child_aliases() {
        check_child_record_identity(true);
    }

    #[test]
    fn restore_requires_identical_recompiled_code() {
        let compile = |source| {
            crate::script::compile_story_bytecode("entry.hks", source).expect("fixture compiles")
        };
        let original = compile("log(\"alice\")");
        let runtime = ExecutionRuntime::new(original.clone()).expect("runtime starts");
        let snapshot = runtime.snapshot();
        let encoded = hiraku_script::hson::to_string(&snapshot).expect("snapshot serializes");
        assert!(
            !encoded.contains("instructions"),
            "save must not contain executable code"
        );
        assert!(ExecutionRuntime::restore(original.clone(), snapshot.clone()).is_ok());
        assert!(ExecutionRuntime::restore(compile("log(\"bob\")"), snapshot.clone()).is_err());
        let mut changed_layout = original;
        changed_layout
            .instructions
            .push(hiraku_script::vm::Instruction::Halt);
        assert!(
            ExecutionRuntime::restore(changed_layout, snapshot).is_err(),
            "matching source hash alone is insufficient"
        );
    }

    fn check_child_record_identity(update_from_host: bool) {
        let bytecode = crate::script::compile_story_bytecode(
            "reference_test.hks",
            r#"
            type Player = .{ score: Int }
            global var player = Player.{ score: 1 }
            let alias = player
            par { alias.score += 2 }
        "#,
        )
        .expect("reference story compiles");
        let mut runtime = ExecutionRuntime::new(bytecode.clone()).expect("runtime initializes");
        let closure = loop {
            if let Some(ExecutionEvent::Call { call, .. }) = runtime.step().expect("root advances")
            {
                break call.arguments[0].value.clone();
            }
        };
        let child = runtime
            .spawn(&closure, ExecutionMode::Interactive)
            .expect("child starts");
        if update_from_host {
            let mut globals = runtime.globals().clone();
            let Value::Typed { value, .. } = globals.get_mut("player").expect("player exists")
            else {
                panic!("expected Player")
            };
            let Value::Map(fields) = value.as_mut() else {
                panic!("expected fields")
            };
            fields.insert("score".into(), Value::Int(10));
            runtime.set_globals(globals);
        }
        let encoded =
            hiraku_script::hson::to_string(&runtime.snapshot()).expect("runtime serializes");
        let snapshot = hiraku_script::hson::from_str(&encoded).expect("runtime deserializes");
        let mut runtime = ExecutionRuntime::restore(bytecode, snapshot).expect("runtime restores");
        loop {
            if matches!(runtime.step_children().expect("child advances"), Some(ExecutionEvent::Completed { execution, .. }) if execution == child)
            {
                break;
            }
        }
        let Value::Typed { value, .. } = &runtime.globals()["player"] else {
            panic!("expected Player")
        };
        let Value::Map(fields) = value.as_ref() else {
            panic!("expected fields")
        };
        assert_eq!(
            fields["score"],
            Value::Int(if update_from_host { 12 } else { 3 })
        );
    }
}
