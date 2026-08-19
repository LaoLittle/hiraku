//! Direct whole-program HKS execution state.
//!
//! This runtime is deliberately independent from the transitional story IR. It
//! owns the generic VM and task scheduler while ECS systems own waits and effects.

use hiraku_script::hks::vm::{
    BuiltinCall, Bytecode, TaskEvent, TaskScheduler, TaskSchedulerError, Value, Vm, VmError,
    VmEvent,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum HksRuntimeEvent {
    Call(BuiltinCall),
    TaskCall { task: u64, call: BuiltinCall },
    StatementCommit,
    TaskStatementCommit { task: u64 },
    TaskCompleted { task: u64, value: Value },
    Completed(Value),
}

pub struct HksRuntime {
    vm: Vm,
    scheduler: TaskScheduler,
}

impl HksRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, HksRuntimeError> {
        Ok(Self {
            vm: Vm::new(bytecode.clone())?,
            scheduler: TaskScheduler::new(bytecode)?,
        })
    }

    /// Advances either the main program or the first ready task to the next host boundary.
    pub fn step(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        if let Some(event) = self.vm.step()? {
            return match event {
                VmEvent::Call(call) => Ok(Some(HksRuntimeEvent::Call(call))),
                VmEvent::StatementCommit => Ok(Some(HksRuntimeEvent::StatementCommit)),
                VmEvent::SpawnTask(request) => {
                    let task = self.scheduler.spawn(request.task)?;
                    self.vm.resume(Value::Task(task))?;
                    Ok(Some(HksRuntimeEvent::StatementCommit))
                }
                VmEvent::Completed(value) => Ok(Some(HksRuntimeEvent::Completed(value))),
            };
        }

        match self.scheduler.step()? {
            Some(TaskEvent::Call { task, call }) => {
                Ok(Some(HksRuntimeEvent::TaskCall { task, call }))
            }
            Some(TaskEvent::StatementCommit { task }) => {
                Ok(Some(HksRuntimeEvent::TaskStatementCommit { task }))
            }
            Some(TaskEvent::Completed { task, value }) => {
                Ok(Some(HksRuntimeEvent::TaskCompleted { task, value }))
            }
            None => Ok(None),
        }
    }

    pub fn resume_main(&mut self, value: Value) -> Result<(), HksRuntimeError> {
        self.vm.resume(value)?;
        Ok(())
    }

    pub fn resume_task(&mut self, task: u64, value: Value) -> Result<(), HksRuntimeError> {
        self.scheduler.resume(task, value)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum HksRuntimeError {
    #[error("HKS VM failed: {0:?}")]
    Vm(VmError),
    #[error("HKS task scheduler failed: {0:?}")]
    Scheduler(TaskSchedulerError),
}

impl From<VmError> for HksRuntimeError {
    fn from(error: VmError) -> Self {
        Self::Vm(error)
    }
}

impl From<TaskSchedulerError> for HksRuntimeError {
    fn from(error: TaskSchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hks_capabilities::{StoryEffect, StoryNativeHost, compile_story_bytecode};
    use hiraku_script::hks::vm::BuiltinId;

    #[test]
    fn whole_program_runtime_yields_native_calls_without_ir() {
        let bytecode = compile_story_bytecode("test.story.hks", "log(\"hello\")")
            .expect("whole HKS story must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("direct HKS runtime must initialize");
        let Some(HksRuntimeEvent::Call(call)) = runtime.step().expect("runtime must advance")
        else {
            panic!("expected a native call")
        };
        assert_eq!(call.builtin, BuiltinId(10));
        runtime
            .resume_main(Value::Null)
            .expect("host result must resume the main VM");
    }

    #[test]
    fn direct_runtime_dispatches_native_calls_at_statement_boundaries() {
        let bytecode =
            compile_story_bytecode("test.story.hks", r#"char("Alice").e("happy").at("right")"#)
                .expect("character story must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("direct HKS runtime must initialize");
        let mut host = StoryNativeHost::new();

        loop {
            match runtime.step().expect("runtime must advance") {
                Some(HksRuntimeEvent::Call(call)) => {
                    let value = host.call(&call).expect("native call must succeed");
                    runtime
                        .resume_main(value)
                        .expect("native result must resume the VM");
                }
                Some(HksRuntimeEvent::StatementCommit) => {
                    host.commit_statement()
                        .expect("statement commit must flush actor state");
                }
                Some(HksRuntimeEvent::Completed(_)) => break,
                Some(event) => panic!("unexpected runtime event: {event:?}"),
                None => panic!("runtime stopped before completion"),
            }
        }

        assert!(matches!(
            host.drain_effects().as_slice(),
            [StoryEffect::ShowCharacter {
                actor_id,
                expressions,
                position,
                ..
            }] if actor_id == "Alice" && expressions == &["happy"] && position == &[600.0, 0.0]
        ));
    }

    #[test]
    fn all_shipped_stories_compile_as_whole_hks_programs() {
        for (path, source) in [
            (
                "startup.story.hks",
                include_str!("../../../../manosabars/assets/main_hdp_contents/startup.story.hks"),
            ),
            (
                "system.story.hks",
                include_str!("../../../../manosabars/assets/main_hdp_contents/system.story.hks"),
            ),
            (
                "scripts/new_game.story.hks",
                include_str!(
                    "../../../../manosabars/assets/main_hdp_contents/scripts/new_game.story.hks"
                ),
            ),
            (
                "scripts/gallery.story.hks",
                include_str!(
                    "../../../../manosabars/assets/main_hdp_contents/scripts/gallery.story.hks"
                ),
            ),
            (
                "scripts/settings.story.hks",
                include_str!(
                    "../../../../manosabars/assets/main_hdp_contents/scripts/settings.story.hks"
                ),
            ),
        ] {
            compile_story_bytecode(path, source).unwrap_or_else(|error| {
                panic!("`{path}` failed whole-program compilation: {error}")
            });
        }
    }
}
