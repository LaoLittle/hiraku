//! Engine-owned scheduling for save-safe script closures.
//!
//! The language VM exposes resumable closures but has no task semantics.

use std::{collections::BTreeMap, fmt};

use hiraku_script::{
    Bytecode, StatementValue, SymbolCall, TemplateError, Value, Vm, VmError, VmEvent, VmSnapshot,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    Interactive,
    Sequence,
    Parallel,
}

/// Stable identity of a VM execution within one story runtime.
///
/// The root program always owns [`ExecutionId::MAIN`]. Engine-created closure
/// executions use monotonically increasing identifiers so snapshots and host
/// responses never depend on collection order or VM addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExecutionId(u64);

impl ExecutionId {
    pub const MAIN: Self = Self(0);

    fn child(value: u64) -> Self {
        debug_assert_ne!(value, 0);
        Self(value)
    }

    pub const fn is_main(self) -> bool {
        self.0 == Self::MAIN.0
    }

    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
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

#[derive(Debug, Error)]
pub enum ExecutionSchedulerError {
    #[error("VM failed: {0:?}")]
    Vm(VmError),
    #[error("unknown story execution {0}")]
    UnknownExecution(ExecutionId),
}

impl From<VmError> for ExecutionSchedulerError {
    fn from(value: VmError) -> Self {
        Self::Vm(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RawExecutionEvent {
    Call {
        execution: ExecutionId,
        call: SymbolCall,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ExecutionSnapshot {
    vm: VmSnapshot,
    mode: ExecutionMode,
    paused: bool,
}

struct ExecutionState {
    vm: Vm,
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionSchedulerSnapshot {
    next_execution: u64,
    executions: BTreeMap<ExecutionId, ExecutionSnapshot>,
    globals: Vec<Value>,
}

pub struct ChildExecutionScheduler {
    bytecode: Bytecode,
    next_execution: u64,
    executions: BTreeMap<ExecutionId, ExecutionState>,
    globals: Vec<Value>,
}

impl ChildExecutionScheduler {
    pub fn new(bytecode: Bytecode) -> Self {
        Self {
            globals: vec![Value::Uninitialized; bytecode.globals.len()],
            bytecode,
            next_execution: 1,
            executions: BTreeMap::new(),
        }
    }

    pub fn spawn(
        &mut self,
        closure: &Value,
        mode: ExecutionMode,
    ) -> Result<ExecutionId, ExecutionSchedulerError> {
        let execution = ExecutionId::child(self.next_execution);
        self.next_execution = self
            .next_execution
            .checked_add(1)
            .expect("story execution identifier space must not be exhausted");
        let mut vm = Vm::from_callable(self.bytecode.clone(), closure, Vec::new())?;
        vm.set_global_values(self.globals.clone())?;
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

    pub fn step(&mut self) -> Result<Option<RawExecutionEvent>, ExecutionSchedulerError> {
        for execution in self.executions.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.executions.get_mut(&execution) else {
                continue;
            };
            if state.paused {
                continue;
            }
            match state.vm.step()? {
                Some(VmEvent::Call(call)) => {
                    return Ok(Some(RawExecutionEvent::Call { execution, call }));
                }
                Some(VmEvent::Statement(value)) => {
                    return Ok(Some(RawExecutionEvent::Statement { execution, value }));
                }
                Some(VmEvent::Completed(value)) => {
                    self.globals = state.vm.globals().to_vec();
                    self.executions.remove(&execution);
                    return Ok(Some(RawExecutionEvent::Completed { execution, value }));
                }
                None => {}
            }
        }
        Ok(None)
    }

    pub fn resume(
        &mut self,
        execution: ExecutionId,
        value: Value,
    ) -> Result<(), ExecutionSchedulerError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionSchedulerError::UnknownExecution(execution))?
            .vm
            .resume(value)?;
        Ok(())
    }

    pub fn pause(&mut self, execution: ExecutionId) -> Result<(), ExecutionSchedulerError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionSchedulerError::UnknownExecution(execution))?
            .paused = true;
        Ok(())
    }

    pub fn unpause(&mut self, execution: ExecutionId) -> Result<(), ExecutionSchedulerError> {
        self.executions
            .get_mut(&execution)
            .ok_or(ExecutionSchedulerError::UnknownExecution(execution))?
            .paused = false;
        Ok(())
    }

    pub fn mode(&self, execution: ExecutionId) -> Option<ExecutionMode> {
        self.executions.get(&execution).map(|state| state.mode)
    }

    pub fn eval_template(
        &self,
        execution: ExecutionId,
        template: &str,
    ) -> Result<String, TemplateError> {
        self.executions
            .get(&execution)
            .ok_or_else(|| TemplateError::UnknownPath(format!("execution {execution}")))?
            .vm
            .eval_template(template)
    }

    pub fn set_global_values(&mut self, globals: Vec<Value>) -> Result<(), VmError> {
        if globals.len() != self.bytecode.globals.len() {
            return Err(VmError::FrameShapeMismatch);
        }
        self.globals = globals;
        Ok(())
    }

    pub fn global_values(&self) -> &[Value] {
        &self.globals
    }

    pub fn snapshot(&self) -> ExecutionSchedulerSnapshot {
        ExecutionSchedulerSnapshot {
            next_execution: self.next_execution,
            globals: self.globals.clone(),
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
        }
    }

    pub fn restore(
        bytecode: Bytecode,
        snapshot: ExecutionSchedulerSnapshot,
    ) -> Result<Self, ExecutionSchedulerError> {
        let executions = snapshot
            .executions
            .into_iter()
            .map(|(id, task)| {
                Vm::restore(bytecode.clone(), task.vm).map(|vm| {
                    (
                        id,
                        ExecutionState {
                            vm,
                            mode: task.mode,
                            paused: task.paused,
                        },
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            bytecode,
            next_execution: snapshot.next_execution,
            executions,
            globals: snapshot.globals,
        })
    }
}
