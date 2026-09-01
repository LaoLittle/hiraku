//! Engine-owned scheduling for save-safe script closures.
//!
//! The language VM exposes resumable closures but has no task semantics.

use std::collections::BTreeMap;

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

#[derive(Debug, Error)]
pub enum TaskSchedulerError {
    #[error("VM failed: {0:?}")]
    Vm(VmError),
    #[error("unknown story task {0}")]
    UnknownTask(u64),
}

impl From<VmError> for TaskSchedulerError {
    fn from(value: VmError) -> Self {
        Self::Vm(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskEvent {
    Call { task: u64, call: SymbolCall },
    Statement { task: u64, value: StatementValue },
    Completed { task: u64, value: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TaskSnapshot {
    vm: VmSnapshot,
    mode: ExecutionMode,
    paused: bool,
}

struct TaskState {
    vm: Vm,
    mode: ExecutionMode,
    paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSchedulerSnapshot {
    next_task: u64,
    tasks: BTreeMap<u64, TaskSnapshot>,
    globals: Vec<Value>,
}

pub struct TaskScheduler {
    bytecode: Bytecode,
    next_task: u64,
    tasks: BTreeMap<u64, TaskState>,
    globals: Vec<Value>,
}

impl TaskScheduler {
    pub fn new(bytecode: Bytecode) -> Self {
        Self {
            globals: vec![Value::Uninitialized; bytecode.globals.len()],
            bytecode,
            next_task: 1,
            tasks: BTreeMap::new(),
        }
    }

    pub fn spawn(
        &mut self,
        closure: &Value,
        mode: ExecutionMode,
    ) -> Result<u64, TaskSchedulerError> {
        let task = self.next_task;
        self.next_task += 1;
        let mut vm = Vm::from_callable(self.bytecode.clone(), closure, Vec::new())?;
        vm.set_global_values(self.globals.clone())?;
        self.tasks.insert(
            task,
            TaskState {
                vm,
                mode,
                paused: false,
            },
        );
        Ok(task)
    }

    pub fn step(&mut self) -> Result<Option<TaskEvent>, TaskSchedulerError> {
        for task in self.tasks.keys().copied().collect::<Vec<_>>() {
            let Some(state) = self.tasks.get_mut(&task) else {
                continue;
            };
            if state.paused {
                continue;
            }
            match state.vm.step()? {
                Some(VmEvent::Call(call)) => {
                    return Ok(Some(TaskEvent::Call { task, call }));
                }
                Some(VmEvent::Statement(value)) => {
                    return Ok(Some(TaskEvent::Statement { task, value }));
                }
                Some(VmEvent::Completed(value)) => {
                    self.globals = state.vm.globals().to_vec();
                    self.tasks.remove(&task);
                    return Ok(Some(TaskEvent::Completed { task, value }));
                }
                None => {}
            }
        }
        Ok(None)
    }

    pub fn resume(&mut self, task: u64, value: Value) -> Result<(), TaskSchedulerError> {
        self.tasks
            .get_mut(&task)
            .ok_or(TaskSchedulerError::UnknownTask(task))?
            .vm
            .resume(value)?;
        Ok(())
    }

    pub fn pause(&mut self, task: u64) -> Result<(), TaskSchedulerError> {
        self.tasks
            .get_mut(&task)
            .ok_or(TaskSchedulerError::UnknownTask(task))?
            .paused = true;
        Ok(())
    }

    pub fn unpause(&mut self, task: u64) -> Result<(), TaskSchedulerError> {
        self.tasks
            .get_mut(&task)
            .ok_or(TaskSchedulerError::UnknownTask(task))?
            .paused = false;
        Ok(())
    }

    pub fn mode(&self, task: u64) -> Option<ExecutionMode> {
        self.tasks.get(&task).map(|state| state.mode)
    }

    pub fn eval_template(&self, task: u64, template: &str) -> Result<String, TemplateError> {
        self.tasks
            .get(&task)
            .ok_or_else(|| TemplateError::UnknownPath(format!("task {task}")))?
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

    pub fn snapshot(&self) -> TaskSchedulerSnapshot {
        TaskSchedulerSnapshot {
            next_task: self.next_task,
            globals: self.globals.clone(),
            tasks: self
                .tasks
                .iter()
                .map(|(id, state)| {
                    (
                        *id,
                        TaskSnapshot {
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
        snapshot: TaskSchedulerSnapshot,
    ) -> Result<Self, TaskSchedulerError> {
        let tasks = snapshot
            .tasks
            .into_iter()
            .map(|(id, task)| {
                Vm::restore(bytecode.clone(), task.vm).map(|vm| {
                    (
                        id,
                        TaskState {
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
            next_task: snapshot.next_task,
            tasks,
            globals: snapshot.globals,
        })
    }
}
