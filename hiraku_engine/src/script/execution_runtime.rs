//! Unified execution model for a linked story program.
//!
//! Every running VM, including the root program and engine-created closures,
//! lives in the same execution table and crosses the same host event boundary.

use std::{collections::BTreeMap, fmt};

use hiraku_script::{
    BuiltinCall, Bytecode, LinkedBytecode, LinkedFunction, StatementValue, SymbolCall,
    TemplateError, Value, Vm, VmError, VmEvent, VmSnapshot, VmStatus, link_bytecode,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::script::capabilities::story_manifest;

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
    Sequence,
    Parallel,
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
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ExecutionSnapshot {
    vm: VmSnapshot,
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRuntimeSnapshot {
    objects: hiraku_script::ObjectHeap,
    /// Exact executing bytecode. Restore relinks this against the current
    /// native registry instead of recompiling source and trusting offsets.
    pub program: Bytecode,
    next_execution: u64,
    executions: BTreeMap<ExecutionId, ExecutionSnapshot>,
    shared_globals: Vec<Value>,
}

pub struct ExecutionRuntime {
    objects: hiraku_script::ObjectHeap,
    linked: LinkedBytecode,
    executions: BTreeMap<ExecutionId, ExecutionState>,
    next_execution: u64,
    shared_globals: Vec<Value>,
    globals: BTreeMap<String, Value>,
}

impl ExecutionRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, ExecutionRuntimeError> {
        let linked =
            link_bytecode(bytecode, &story_manifest()).map_err(ExecutionRuntimeError::Link)?;
        let bytecode = linked.bytecode.clone();
        let shared_globals = vec![Value::Uninitialized; bytecode.globals.len()];
        let mut executions = BTreeMap::new();
        executions.insert(
            ExecutionId::MAIN,
            ExecutionState {
                vm: Vm::new(bytecode)?,
                mode: ExecutionMode::Main,
                paused: false,
            },
        );
        Ok(Self {
            objects: hiraku_script::ObjectHeap::default(),
            linked,
            executions,
            next_execution: 1,
            shared_globals,
            globals: BTreeMap::new(),
        })
    }

    pub fn restore(
        _bytecode: Bytecode,
        snapshot: ExecutionRuntimeSnapshot,
    ) -> Result<Self, ExecutionRuntimeError> {
        let bytecode = snapshot.program;
        let linked =
            link_bytecode(bytecode, &story_manifest()).map_err(ExecutionRuntimeError::Link)?;
        let bytecode = linked.bytecode.clone();
        let executions = snapshot
            .executions
            .into_iter()
            .map(|(id, state)| {
                Vm::restore(bytecode.clone(), state.vm).map(|vm| {
                    (
                        id,
                        ExecutionState {
                            vm,
                            mode: state.mode,
                            paused: state.paused,
                        },
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        let globals = globals_from_values(&bytecode, &snapshot.shared_globals)
            .into_iter()
            .map(|(key, value)| Ok((key, snapshot.objects.export(&value)?)))
            .collect::<Result<_, hiraku_script::VmError>>()?;
        Ok(Self {
            linked,
            executions,
            next_execution: snapshot.next_execution,
            objects: snapshot.objects,
            shared_globals: snapshot.shared_globals,
            globals,
        })
    }

    pub fn snapshot(&self) -> ExecutionRuntimeSnapshot {
        ExecutionRuntimeSnapshot {
            objects: self.objects.clone(),
            program: self.linked.bytecode.as_ref().clone(),
            next_execution: self.next_execution,
            executions: self
                .executions
                .iter()
                .map(|(id, state)| {
                    (
                        *id,
                        ExecutionSnapshot {
                            vm: state.vm.snapshot(),
                            mode: state.mode,
                            paused: state.paused,
                        },
                    )
                })
                .collect(),
            shared_globals: self.shared_globals.clone(),
        }
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
        let mut vm = Vm::from_callable(self.linked.bytecode.clone(), &closure, Vec::new())?;
        vm.set_global_values(self.shared_globals.clone())?;
        self.executions.insert(
            execution,
            ExecutionState {
                vm,
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

    fn step_execution(
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
        let event = {
            let state = self
                .executions
                .get_mut(&execution)
                .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
            if state.paused {
                return Ok(None);
            }
            state.vm.set_global_values(self.shared_globals.clone())?;
            state.vm.step_with_budget(budget)?
        };
        let Some(event) = event else {
            return Ok(None);
        };

        match event {
            VmEvent::BudgetExhausted => {
                self.capture_globals(execution)?;
                Ok(None)
            }
            VmEvent::Call(call) => {
                self.capture_globals(execution)?;
                let mut call = self.link_call(call)?;
                let state = self
                    .executions
                    .get(&execution)
                    .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                evaluate_call_templates(&mut call, |text| state.vm.eval_template(text))?;
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
                let mut state = self
                    .executions
                    .remove(&execution)
                    .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?;
                self.shared_globals = state.vm.globals().to_vec();
                state.vm.swap_objects(&mut self.objects);
                Ok(Some(ExecutionEvent::Completed { execution, value }))
            }
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
        let incoming = values_from_globals(&self.linked.bytecode, &globals);
        let changed = self
            .shared_globals
            .iter()
            .zip(incoming)
            .map(|(old, new)| {
                let same = self.objects.export(old).is_ok_and(|value| value == new);
                (old.clone(), new, same)
            })
            .collect::<Vec<_>>();
        self.shared_globals = changed
            .into_iter()
            .map(|(old, new, same)| {
                if same {
                    old
                } else {
                    self.objects
                        .update(&old, new)
                        .expect("host global updates target valid objects")
                }
            })
            .collect();
        for state in self.executions.values_mut() {
            state
                .vm
                .set_global_values(self.shared_globals.clone())
                .expect("compiled global frame shape must match its bytecode");
        }
        self.globals = globals;
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
            self.objects.collect(
                self.executions
                    .values()
                    .flat_map(|state| state.vm.object_roots())
                    .chain(self.shared_globals.iter())
                    .chain(host_roots.iter().copied()),
            )?;
        }
        Ok(())
    }

    fn capture_globals(&mut self, execution: ExecutionId) -> Result<(), ExecutionRuntimeError> {
        self.shared_globals = self
            .executions
            .get(&execution)
            .ok_or(ExecutionRuntimeError::UnknownExecution(execution))?
            .vm
            .globals()
            .to_vec();
        Ok(())
    }

    fn refresh_globals(&mut self) {
        self.globals = globals_from_values(&self.linked.bytecode, &self.shared_globals)
            .into_iter()
            .map(|(key, value)| (key, self.objects.export(&value).unwrap_or(value)))
            .collect();
    }

    fn link_call(&self, call: SymbolCall) -> Result<BuiltinCall, ExecutionRuntimeError> {
        let Some(LinkedFunction::Native(builtin)) = self.linked.resolve(call.function) else {
            return Err(ExecutionRuntimeError::UnlinkedCall(call.function));
        };
        Ok(BuiltinCall {
            builtin,
            receiver: call.receiver,
            arguments: call.arguments,
        })
    }
}

#[derive(Debug, Error)]
pub enum ExecutionRuntimeError {
    #[error("HKS VM failed: {0:?}")]
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
    mut evaluate: impl FnMut(&str) -> Result<String, TemplateError>,
) -> Result<(), TemplateError> {
    if let Some(Value::TextTemplate(text)) = &mut call.receiver {
        *text = evaluate(text)?;
    }
    for argument in &mut call.arguments {
        if let Value::TextTemplate(text) = &mut argument.value {
            *text = evaluate(text)?;
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
    fn child_execution_shares_captured_records_after_snapshot_restore() {
        check_child_record_identity(false);
    }

    #[test]
    fn host_global_updates_preserve_child_aliases() {
        check_child_record_identity(true);
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
            .spawn(&closure, ExecutionMode::Parallel)
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
            fields.insert("score".into(), Value::Number(10.0));
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
            Value::Number(if update_from_host { 12.0 } else { 3.0 })
        );
    }
}
