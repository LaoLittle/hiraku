//! Direct whole-program HKS execution state.
//!
//! This runtime is deliberately independent from the transitional story IR. It
//! owns the generic VM and task scheduler while ECS systems own waits and effects.

use hiraku_script::StatementValue;
use hiraku_script::TemplateError;
use hiraku_script::vm::{
    BuiltinCall, Bytecode, TaskEvent, TaskScheduler, TaskSchedulerError, TaskSchedulerSnapshot,
    Value, Vm, VmError, VmEvent, VmSnapshot,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum HksRuntimeEvent {
    Call(BuiltinCall),
    TaskCall { task: u64, call: BuiltinCall },
    Statement(StatementValue),
    TaskStatement { task: u64, value: StatementValue },
    TaskCompleted { task: u64, value: Value },
    Completed(Value),
}

pub struct HksRuntime {
    vm: Vm,
    scheduler: TaskScheduler,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HksRuntimeSnapshot {
    pub vm: VmSnapshot,
    pub scheduler: TaskSchedulerSnapshot,
}

impl HksRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, HksRuntimeError> {
        Ok(Self {
            vm: Vm::new(bytecode.clone())?,
            scheduler: TaskScheduler::new(bytecode)?,
        })
    }

    pub fn snapshot(&self) -> HksRuntimeSnapshot {
        HksRuntimeSnapshot {
            vm: self.vm.snapshot(),
            scheduler: self.scheduler.snapshot(),
        }
    }

    pub fn restore(
        bytecode: Bytecode,
        snapshot: HksRuntimeSnapshot,
    ) -> Result<Self, HksRuntimeError> {
        Ok(Self {
            vm: Vm::restore(bytecode.clone(), snapshot.vm)?,
            scheduler: TaskScheduler::restore(bytecode, snapshot.scheduler)?,
        })
    }

    /// Advances either the main program or the first ready task to the next host boundary.
    pub fn step(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        if let Some(event) = self.vm.step()? {
            return match event {
                VmEvent::Call(mut call) => {
                    evaluate_call_templates(&mut call, |text| self.vm.eval_template(text))?;
                    Ok(Some(HksRuntimeEvent::Call(call)))
                }
                VmEvent::Statement(value) => {
                    let value = evaluate_statement_template(&mut self.vm, value)?;
                    self.scheduler.set_globals(self.vm.globals().clone());
                    Ok(Some(HksRuntimeEvent::Statement(value)))
                }
                VmEvent::SpawnTask(request) => {
                    let task = self.scheduler.spawn(request.task)?;
                    self.vm.resume(Value::Task(task))?;
                    Ok(Some(HksRuntimeEvent::Statement(StatementValue::Commit)))
                }
                VmEvent::Completed(value) => Ok(Some(HksRuntimeEvent::Completed(value))),
            };
        }

        match self.scheduler.step()? {
            Some(TaskEvent::Call { task, mut call }) => {
                evaluate_call_templates(&mut call, |text| self.scheduler.eval_template(text))?;
                Ok(Some(HksRuntimeEvent::TaskCall { task, call }))
            }
            Some(TaskEvent::Statement { task, value }) => {
                let value = evaluate_task_statement_template(&mut self.scheduler, value)?;
                self.vm.set_globals(self.scheduler.globals().clone());
                Ok(Some(HksRuntimeEvent::TaskStatement { task, value }))
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

    pub fn set_globals(&mut self, globals: std::collections::BTreeMap<String, Value>) {
        self.vm.set_globals(globals.clone());
        self.scheduler.set_globals(globals);
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
    #[error("HKS string template failed: {0}")]
    Template(#[from] TemplateError),
}

fn evaluate_statement_template(
    vm: &mut Vm,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(vm.eval_template(&text)?)),
        value => Ok(value),
    }
}

fn evaluate_task_statement_template(
    scheduler: &mut TaskScheduler,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(scheduler.eval_template(&text)?)),
        value => Ok(value),
    }
}

fn evaluate_call_templates(
    call: &mut BuiltinCall,
    mut evaluate: impl FnMut(&str) -> Result<String, TemplateError>,
) -> Result<(), TemplateError> {
    if let Some(Value::String(text)) = &mut call.receiver {
        *text = evaluate(text)?;
    }
    for argument in &mut call.arguments {
        if let Value::String(text) = &mut argument.value {
            *text = evaluate(text)?;
        }
    }
    Ok(())
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
    use crate::script::capabilities::{
        StoryEffect, StoryNativeHost, compile_story_bytecode, story_manifest,
    };

    #[test]
    fn whole_program_runtime_yields_native_calls_without_ir() {
        let bytecode = compile_story_bytecode("test.story.hks", "log(\"hello\")")
            .expect("whole HKS story must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("direct HKS runtime must initialize");
        let Some(HksRuntimeEvent::Call(call)) = runtime.step().expect("runtime must advance")
        else {
            panic!("expected a native call")
        };
        assert_eq!(
            call.builtin,
            story_manifest().resolve("log").expect("log registration")
        );
        runtime
            .resume_main(Value::Null)
            .expect("host result must resume the main VM");
    }

    #[test]
    fn whole_program_runtime_restores_at_a_host_boundary() {
        let bytecode = compile_story_bytecode("restore.story.hks", "log(\"before\")\n\"after\"")
            .expect("whole HKS story must compile");
        let mut runtime = HksRuntime::new(bytecode.clone()).expect("runtime must initialize");
        assert!(matches!(
            runtime.step().expect("runtime must advance"),
            Some(HksRuntimeEvent::Call(_))
        ));
        let snapshot = runtime.snapshot();
        let mut restored = HksRuntime::restore(bytecode, snapshot).expect("snapshot must restore");
        restored
            .resume_main(Value::Null)
            .expect("restored host call must resume");
        assert_eq!(
            restored.step().expect("runtime must reach statement"),
            Some(HksRuntimeEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().expect("runtime must reach string hook"),
            Some(HksRuntimeEvent::Statement(StatementValue::String(
                "after".to_string()
            )))
        );
    }

    #[test]
    fn whole_program_runtime_evaluates_dialogue_templates_from_globals() {
        let bytecode = compile_story_bytecode(
            "template.story.hks",
            "global player = .{ name: \"Alice\" }\n\"Hi, ${player.name}\"",
        )
        .expect("template story must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("runtime must initialize");
        assert!(matches!(
            runtime.step().expect("global declaration must run"),
            Some(HksRuntimeEvent::Statement(StatementValue::Commit))
        ));
        assert_eq!(
            runtime.step().expect("dialogue statement must run"),
            Some(HksRuntimeEvent::Statement(StatementValue::String(
                "Hi, Alice".to_string()
            )))
        );
    }

    #[test]
    fn direct_runtime_dispatches_native_calls_at_statement_boundaries() {
        let bytecode =
            compile_story_bytecode("test.story.hks", r#"char("Alice").e("happy").at(.right)"#)
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
                Some(HksRuntimeEvent::Statement(StatementValue::Commit)) => {
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
            }] if actor_id == "Alice" && expressions == &["happy"] && position == &[600.0, -200.0]
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
