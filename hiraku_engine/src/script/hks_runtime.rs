//! Direct whole-program HKS execution state.
//!
//! It owns the generic VM and task scheduler while ECS systems own waits and effects.

use std::collections::{BTreeMap, VecDeque};

use hiraku_script::StatementValue;
use hiraku_script::TemplateError;
use hiraku_script::{
    BuiltinCall, LinkedBytecode, LinkedFunction, RegisterBytecode, RegisterTaskEvent,
    RegisterTaskMode, RegisterTaskScheduler, RegisterTaskSchedulerSnapshot, RegisterVm,
    RegisterVmError, RegisterVmEvent, RegisterVmSnapshot, SymbolCall, Value,
    link_register_bytecode,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::script::capabilities::{
    CharacterCapabilityError, StoryEffect, StoryNativeHost, StoryNativeHostSnapshot, StoryWait,
    story_manifest,
};

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
    vm: RegisterVm,
    scheduler: RegisterTaskScheduler,
    linked: LinkedBytecode,
    globals: BTreeMap<String, Value>,
}

/// Engine-facing whole-story driver. It translates generic VM boundaries into
/// story effects without introducing a second executable representation.
pub struct StoryRuntime {
    bytecode: HksRuntime,
    host: StoryNativeHost,
    pending: VecDeque<StoryRuntimeEvent>,
    active_task_effects: BTreeMap<u64, (StoryEffect, Value)>,
    waiting_task: Option<u64>,
    choice: Option<ChoiceState>,
    blocked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ClosureValue {
    region: u32,
    statements: Vec<u32>,
    captures: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ChoiceOption {
    label: String,
    body: ClosureValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum ChoiceState {
    Collecting {
        builder_task: u64,
        prompt: String,
        options: Vec<ChoiceOption>,
    },
    AwaitingSelection {
        prompt: String,
        options: Vec<ChoiceOption>,
    },
    RunningBranch {
        task: u64,
        selected: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoryRuntimeEvent {
    Effect(StoryEffect),
    Wait(StoryWait),
    OpenUi {
        path: String,
    },
    Choice {
        prompt: String,
        options: Vec<String>,
    },
    TaskEffect {
        task: u64,
        effect: StoryEffect,
    },
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoryRuntimeSnapshot {
    bytecode: HksRuntimeSnapshot,
    host: StoryNativeHostSnapshot,
    active_task_effects: BTreeMap<u64, (StoryEffect, Value)>,
    waiting_task: Option<u64>,
    choice: Option<ChoiceState>,
    blocked: bool,
}

impl StoryRuntime {
    pub fn new(bytecode: RegisterBytecode) -> Result<Self, StoryRuntimeError> {
        Ok(Self {
            bytecode: HksRuntime::new(bytecode)?,
            host: StoryNativeHost::new(),
            pending: VecDeque::new(),
            active_task_effects: BTreeMap::new(),
            waiting_task: None,
            choice: None,
            blocked: false,
        })
    }

    pub fn snapshot(&self) -> Result<StoryRuntimeSnapshot, StoryRuntimeError> {
        if !self.pending.is_empty() {
            return Err(StoryRuntimeError::NotAtSnapshotBoundary);
        }
        Ok(StoryRuntimeSnapshot {
            bytecode: self.bytecode.snapshot(),
            host: self.host.snapshot(),
            active_task_effects: self.active_task_effects.clone(),
            waiting_task: self.waiting_task,
            choice: self.choice.clone(),
            blocked: self.blocked,
        })
    }

    pub fn restore(
        bytecode: RegisterBytecode,
        snapshot: StoryRuntimeSnapshot,
    ) -> Result<Self, StoryRuntimeError> {
        let pending = snapshot
            .active_task_effects
            .iter()
            .map(|(task, (effect, _))| StoryRuntimeEvent::TaskEffect {
                task: *task,
                effect: effect.clone(),
            })
            .collect();
        Ok(Self {
            bytecode: HksRuntime::restore(bytecode, snapshot.bytecode)?,
            host: StoryNativeHost::restore(snapshot.host),
            pending,
            active_task_effects: snapshot.active_task_effects,
            waiting_task: snapshot.waiting_task,
            choice: snapshot.choice,
            blocked: snapshot.blocked,
        })
    }

    pub fn set_globals(&mut self, globals: std::collections::BTreeMap<String, Value>) {
        self.bytecode.set_globals(globals);
    }

    pub fn globals(&self) -> &std::collections::BTreeMap<String, Value> {
        self.bytecode.globals()
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    pub fn resume(&mut self, value: Value) -> Result<(), StoryRuntimeError> {
        if !self.blocked {
            return Err(StoryRuntimeError::NotBlocked);
        }
        if let Some(ChoiceState::AwaitingSelection { options, .. }) = &self.choice {
            let Value::Number(selected) = value else {
                return Err(StoryRuntimeError::InvalidChoice);
            };
            let selected = selected as usize;
            let option = options
                .get(selected)
                .ok_or(StoryRuntimeError::InvalidChoice)?
                .body
                .clone();
            let task = self
                .bytecode
                .spawn_closure(&option, RegisterTaskMode::Sequence)?;
            self.choice = Some(ChoiceState::RunningBranch { task, selected });
            self.blocked = false;
            return Ok(());
        }
        self.blocked = false;
        if self.bytecode.main_waiting_for_host() {
            self.bytecode.resume_main(value)?;
        }
        Ok(())
    }

    pub fn resume_task(&mut self, task: u64) -> Result<(), StoryRuntimeError> {
        let (_, value) = self
            .active_task_effects
            .remove(&task)
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        self.bytecode.resume_task(task, value)?;
        Ok(())
    }

    pub fn step(&mut self) -> Result<Option<StoryRuntimeEvent>, StoryRuntimeError> {
        if let Some(event) = self.pending.pop_front() {
            if matches!(
                event,
                StoryRuntimeEvent::Wait(_)
                    | StoryRuntimeEvent::OpenUi { .. }
                    | StoryRuntimeEvent::Choice { .. }
            ) {
                self.blocked = true;
            }
            return Ok(Some(event));
        }
        if self.blocked {
            loop {
                let Some(event) = self.bytecode.step_task()? else {
                    return Ok(None);
                };
                match event {
                    HksRuntimeEvent::TaskCall { task, call } => {
                        let value = self.host.call(&call)?;
                        if call.builtin == story_manifest().resolve("voice").expect("voice builtin")
                        {
                            let mut effects = self.host.drain_effects();
                            let effect =
                                effects.pop().ok_or(StoryRuntimeError::MissingTaskEffect)?;
                            if !effects.is_empty() {
                                return Err(StoryRuntimeError::AmbiguousTaskEffect);
                            }
                            self.active_task_effects
                                .insert(task, (effect.clone(), value));
                            return Ok(Some(StoryRuntimeEvent::TaskEffect { task, effect }));
                        }
                        self.bytecode.resume_task(task, value)?;
                    }
                    HksRuntimeEvent::TaskStatement { value, .. } => {
                        self.host.handle_statement(&value)?;
                        self.enqueue_host_boundaries();
                        if self.host.take_wait().is_some()
                            || self
                                .pending
                                .iter()
                                .any(|event| matches!(event, StoryRuntimeEvent::Wait(_)))
                        {
                            return Err(StoryRuntimeError::SuspendingTaskCapability);
                        }
                        if let Some(event) = self.pending.pop_front() {
                            return Ok(Some(event));
                        }
                    }
                    HksRuntimeEvent::TaskCompleted { .. } => {}
                    _ => unreachable!("task-only stepping cannot yield a main VM event"),
                }
            }
        }
        loop {
            let Some(event) = self.bytecode.step()? else {
                return Ok(None);
            };
            match event {
                HksRuntimeEvent::Call(call) => {
                    let task_mode = if call.builtin
                        == story_manifest().resolve("seq").expect("seq builtin")
                    {
                        Some(RegisterTaskMode::Sequence)
                    } else if call.builtin == story_manifest().resolve("par").expect("par builtin")
                    {
                        Some(RegisterTaskMode::Parallel)
                    } else {
                        None
                    };
                    if let Some(mode) = task_mode {
                        let Some(closure) = call
                            .arguments
                            .first()
                            .and_then(|argument| closure_value(&argument.value))
                        else {
                            return Err(StoryRuntimeError::InvalidTaskClosure);
                        };
                        let task = self.bytecode.spawn_closure(&closure, mode)?;
                        self.bytecode.resume_main(Value::Task(task))?;
                        continue;
                    }
                    if call.builtin == story_manifest().resolve("choice").expect("choice builtin") {
                        let closure =
                            closure_argument(&call).ok_or(StoryRuntimeError::InvalidChoice)?;
                        let prompt = call
                            .arguments
                            .iter()
                            .find_map(|argument| match &argument.value {
                                Value::String(prompt) => Some(prompt.clone()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        let builder_task = self
                            .bytecode
                            .spawn_closure(&closure, RegisterTaskMode::Sequence)?;
                        self.choice = Some(ChoiceState::Collecting {
                            builder_task,
                            prompt,
                            options: Vec::new(),
                        });
                        continue;
                    }
                    if call.builtin == story_manifest().resolve("openUi").expect("openUi builtin") {
                        let Some(Value::String(path)) =
                            call.arguments.first().map(|arg| &arg.value)
                        else {
                            return Err(StoryRuntimeError::InvalidOpenUi);
                        };
                        self.blocked = true;
                        return Ok(Some(StoryRuntimeEvent::OpenUi { path: path.clone() }));
                    }
                    if call.builtin == story_manifest().resolve("wait").expect("wait builtin") {
                        let Some(Value::Task(task)) = call.arguments.first().map(|arg| &arg.value)
                        else {
                            return Err(StoryRuntimeError::InvalidTaskHandle);
                        };
                        self.waiting_task = Some(*task);
                        continue;
                    }
                    let value = self.host.call(&call)?;
                    self.bytecode.resume_main(value)?;
                }
                HksRuntimeEvent::Statement(statement) => {
                    self.host.handle_statement(&statement)?;
                    self.enqueue_host_boundaries();
                    if let Some(event) = self.pending.pop_front() {
                        if matches!(event, StoryRuntimeEvent::Wait(_)) {
                            self.blocked = true;
                        }
                        return Ok(Some(event));
                    }
                }
                HksRuntimeEvent::TaskCall { task, call } => {
                    if call.builtin == story_manifest().resolve("option").expect("option builtin") {
                        let Some(ChoiceState::Collecting { options, .. }) = &mut self.choice else {
                            return Err(StoryRuntimeError::InvalidChoice);
                        };
                        let Some(Value::String(label)) =
                            call.arguments.first().map(|arg| &arg.value)
                        else {
                            return Err(StoryRuntimeError::InvalidChoice);
                        };
                        let body =
                            closure_argument(&call).ok_or(StoryRuntimeError::InvalidChoice)?;
                        options.push(ChoiceOption {
                            label: label.clone(),
                            body,
                        });
                        self.bytecode.resume_task(task, Value::Null)?;
                        continue;
                    }
                    let value = self.host.call(&call)?;
                    if call.builtin == story_manifest().resolve("voice").expect("voice builtin") {
                        let mut effects = self.host.drain_effects();
                        let effect = effects.pop().ok_or(StoryRuntimeError::MissingTaskEffect)?;
                        if !effects.is_empty() {
                            return Err(StoryRuntimeError::AmbiguousTaskEffect);
                        }
                        self.active_task_effects
                            .insert(task, (effect.clone(), value));
                        return Ok(Some(StoryRuntimeEvent::TaskEffect { task, effect }));
                    }
                    self.bytecode.resume_task(task, value)?;
                }
                HksRuntimeEvent::TaskStatement { value, .. } => {
                    self.host.handle_statement(&value)?;
                    self.enqueue_host_boundaries();
                    if let Some(event) = self.pending.pop_front() {
                        return Ok(Some(event));
                    }
                }
                HksRuntimeEvent::TaskCompleted { task, value } => {
                    if let Some(ChoiceState::Collecting {
                        builder_task,
                        prompt,
                        options,
                    }) = &self.choice
                        && *builder_task == task
                    {
                        let prompt = prompt.clone();
                        let options = options.clone();
                        let labels = options.iter().map(|option| option.label.clone()).collect();
                        self.choice = Some(ChoiceState::AwaitingSelection {
                            prompt: prompt.clone(),
                            options,
                        });
                        self.blocked = true;
                        return Ok(Some(StoryRuntimeEvent::Choice {
                            prompt,
                            options: labels,
                        }));
                    }
                    if let Some(ChoiceState::RunningBranch {
                        task: branch,
                        selected,
                    }) = self.choice
                        && branch == task
                    {
                        self.choice = None;
                        self.bytecode.resume_main(Value::Number(selected as f64))?;
                        continue;
                    }
                    if self.waiting_task == Some(task) {
                        self.waiting_task = None;
                        self.bytecode.resume_main(value)?;
                    }
                }
                HksRuntimeEvent::Completed(value) => {
                    return Ok(Some(StoryRuntimeEvent::Completed(value)));
                }
            }
        }
    }

    fn enqueue_host_boundaries(&mut self) {
        self.pending.extend(
            self.host
                .drain_effects()
                .into_iter()
                .map(StoryRuntimeEvent::Effect),
        );
        if let Some(wait) = self.host.take_wait() {
            self.pending.push_back(StoryRuntimeEvent::Wait(wait));
        }
    }
}

fn closure_argument(call: &BuiltinCall) -> Option<ClosureValue> {
    call.arguments
        .iter()
        .find_map(|argument| closure_value(&argument.value))
}

fn closure_value(value: &Value) -> Option<ClosureValue> {
    match value {
        Value::RegisterClosure {
            region,
            statements,
            captures,
        } => Some(ClosureValue {
            region: *region,
            statements: statements.clone(),
            captures: captures.clone(),
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HksRuntimeSnapshot {
    /// Exact executing bytecode. Restore relinks this against the current
    /// native registry instead of relying on source recompilation offsets.
    pub program: RegisterBytecode,
    pub vm: RegisterVmSnapshot,
    pub scheduler: RegisterTaskSchedulerSnapshot,
}

impl HksRuntime {
    pub fn new(bytecode: RegisterBytecode) -> Result<Self, HksRuntimeError> {
        let linked = link_register_bytecode(bytecode.clone(), &story_manifest())
            .map_err(HksRuntimeError::Link)?;
        Ok(Self {
            vm: RegisterVm::new(bytecode.clone())?,
            scheduler: RegisterTaskScheduler::new(bytecode),
            linked,
            globals: BTreeMap::new(),
        })
    }

    pub fn snapshot(&self) -> HksRuntimeSnapshot {
        HksRuntimeSnapshot {
            program: self.linked.bytecode.clone(),
            vm: self.vm.snapshot(),
            scheduler: self.scheduler.snapshot(),
        }
    }

    fn spawn_closure(
        &mut self,
        closure: &ClosureValue,
        mode: RegisterTaskMode,
    ) -> Result<u64, HksRuntimeError> {
        self.scheduler
            .set_global_values(self.vm.globals().to_vec())?;
        Ok(self.scheduler.spawn_closure(
            &Value::RegisterClosure {
                region: closure.region,
                statements: closure.statements.clone(),
                captures: closure.captures.clone(),
            },
            mode,
        )?)
    }

    pub fn restore(
        _bytecode: RegisterBytecode,
        snapshot: HksRuntimeSnapshot,
    ) -> Result<Self, HksRuntimeError> {
        let bytecode = snapshot.program;
        let linked = link_register_bytecode(bytecode.clone(), &story_manifest())
            .map_err(HksRuntimeError::Link)?;
        let vm = RegisterVm::restore(bytecode.clone(), snapshot.vm)?;
        let globals = globals_from_values(&bytecode, vm.globals());
        Ok(Self {
            vm,
            scheduler: RegisterTaskScheduler::restore(bytecode, snapshot.scheduler)?,
            linked,
            globals,
        })
    }

    /// Advances either the main program or the first ready task to the next host boundary.
    pub fn step(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        if let Some(event) = self.vm.step()? {
            return match event {
                RegisterVmEvent::Call(call) => {
                    let mut call = self.link_call(call)?;
                    evaluate_call_templates(&mut call, |text| self.vm.eval_template(text))?;
                    Ok(Some(HksRuntimeEvent::Call(call)))
                }
                RegisterVmEvent::Statement(value) => {
                    let value = evaluate_statement_template(&mut self.vm, value)?;
                    self.refresh_globals();
                    self.scheduler
                        .set_global_values(self.vm.globals().to_vec())?;
                    Ok(Some(HksRuntimeEvent::Statement(value)))
                }
                RegisterVmEvent::Completed(value) => Ok(Some(HksRuntimeEvent::Completed(value))),
            };
        }

        self.step_task()
    }

    /// Advances only scheduled child tasks, leaving the main VM untouched.
    pub fn step_task(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        match self.scheduler.step()? {
            Some(RegisterTaskEvent::Call { task, call }) => {
                let mut call = self.link_call(call)?;
                evaluate_call_templates(&mut call, |text| {
                    self.scheduler.eval_template(task, text)
                })?;
                Ok(Some(HksRuntimeEvent::TaskCall { task, call }))
            }
            Some(RegisterTaskEvent::Statement { task, value }) => {
                let value = evaluate_task_statement_template(&mut self.scheduler, task, value)?;
                self.vm
                    .set_global_values(self.scheduler.global_values().to_vec())?;
                self.refresh_globals();
                Ok(Some(HksRuntimeEvent::TaskStatement { task, value }))
            }
            Some(RegisterTaskEvent::Completed { task, value }) => {
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
        let values = values_from_globals(&self.linked.bytecode, &globals);
        self.vm
            .set_global_values(values.clone())
            .expect("compiled global frame shape must match its bytecode");
        self.scheduler
            .set_global_values(values)
            .expect("compiled task global frame shape must match its bytecode");
        self.globals = globals;
    }

    pub fn globals(&self) -> &std::collections::BTreeMap<String, Value> {
        &self.globals
    }

    fn main_waiting_for_host(&self) -> bool {
        matches!(
            self.vm.status(),
            hiraku_script::RegisterVmStatus::WaitingForHost
        )
    }

    pub fn resume_task(&mut self, task: u64, value: Value) -> Result<(), HksRuntimeError> {
        self.scheduler.resume(task, value)?;
        Ok(())
    }

    fn link_call(&self, call: SymbolCall) -> Result<BuiltinCall, HksRuntimeError> {
        let Some(LinkedFunction::Native(builtin)) = self.linked.resolve(call.function) else {
            return Err(HksRuntimeError::UnlinkedCall(call.function));
        };
        Ok(BuiltinCall {
            builtin,
            receiver: call.receiver,
            arguments: call.arguments,
        })
    }

    fn refresh_globals(&mut self) {
        self.globals = globals_from_values(&self.linked.bytecode, self.vm.globals());
    }
}

#[derive(Debug, Error)]
pub enum HksRuntimeError {
    #[error("HKS VM failed: {0:?}")]
    Vm(RegisterVmError),
    #[error("HKS bytecode link failed: {0:?}")]
    Link(Vec<hiraku_script::LinkError>),
    #[error("HKS call references an unlinked symbol {0:?}")]
    UnlinkedCall(hiraku_script::SymbolId),
    #[error("HKS string template failed: {0}")]
    Template(#[from] TemplateError),
}

#[derive(Debug, Error)]
pub enum StoryRuntimeError {
    #[error(transparent)]
    Bytecode(#[from] HksRuntimeError),
    #[error(transparent)]
    Capability(#[from] CharacterCapabilityError),
    #[error("openUi requires a string path")]
    InvalidOpenUi,
    #[error("choice requires a string prompt and a list of string options")]
    InvalidChoice,
    #[error("wait requires a task handle")]
    InvalidTaskHandle,
    #[error("seq/par require a trailing closure")]
    InvalidTaskClosure,
    #[error("story runtime is not waiting for a host response")]
    NotBlocked,
    #[error("story runtime snapshot requires an empty effect queue")]
    NotAtSnapshotBoundary,
    #[error("capabilities which suspend for host input are not supported inside seq/par")]
    SuspendingTaskCapability,
    #[error("task {0} has no pending host effect")]
    UnknownTaskEffect(u64),
    #[error("task native call did not produce an effect")]
    MissingTaskEffect,
    #[error("task native call produced more than one effect")]
    AmbiguousTaskEffect,
}

fn evaluate_statement_template(
    vm: &mut RegisterVm,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(vm.eval_template(&text)?)),
        value => Ok(value),
    }
}

fn evaluate_task_statement_template(
    scheduler: &mut RegisterTaskScheduler,
    task: u64,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(
            scheduler.eval_template(task, &text)?,
        )),
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

fn values_from_globals(
    bytecode: &RegisterBytecode,
    globals: &BTreeMap<String, Value>,
) -> Vec<Value> {
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

fn globals_from_values(bytecode: &RegisterBytecode, values: &[Value]) -> BTreeMap<String, Value> {
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

impl From<RegisterVmError> for HksRuntimeError {
    fn from(error: RegisterVmError) -> Self {
        Self::Vm(error)
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
            "global player = .{ name: \"alice\" }\n\"Hi, ${player.name}\"",
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
                "Hi, alice".to_string()
            )))
        );
    }

    #[test]
    fn direct_runtime_dispatches_native_calls_at_statement_boundaries() {
        let bytecode =
            compile_story_bytecode("test.story.hks", r#"char("alice").e("happy").at(.right)"#)
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
            }] if actor_id == "alice" && expressions == &["happy"] && position == &[600.0, -200.0]
        ));
    }

    #[test]
    fn story_driver_does_not_prefetch_past_dialogue_waits() {
        let bytecode = compile_story_bytecode(
            "driver.story.hks",
            r#"
                global player = .{ name: "alice" }
                "Hi, ${player.name}"
                "after"
            "#,
        )
        .expect("driver story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        assert!(matches!(
            runtime.step().expect("first effect must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "Hi, alice"
        ));
        assert_eq!(
            runtime
                .step()
                .expect("dialogue wait must follow the effect"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
        assert_eq!(
            runtime.step().expect("blocked runtime must stay idle"),
            None
        );
        runtime
            .resume(Value::Null)
            .expect("dialogue wait must resume");
        assert!(matches!(
            runtime.step().expect("second effect must run after resume"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "after"
        ));
    }

    #[test]
    fn choice_blocks_suspend_and_resume_into_the_selected_branch() {
        let bytecode = compile_story_bytecode(
            "choice.story.hks",
            r#"
                choice("Select") {
                    option("Route A") { "selected A" }
                    option("Route B") { "selected B" }
                }
            "#,
        )
        .expect("choice story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        assert_eq!(
            runtime.step().expect("choice must suspend"),
            Some(StoryRuntimeEvent::Choice {
                prompt: "Select".into(),
                options: vec!["Route A".into(), "Route B".into()],
            })
        );
        assert_eq!(runtime.step().expect("choice remains blocked"), None);
        runtime
            .resume(Value::Number(1.0))
            .expect("choice response resumes the VM");
        assert!(matches!(
            runtime.step().expect("selected branch runs"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "selected B"
        ));
    }

    #[test]
    fn choice_selection_and_captured_branch_survive_snapshot_restore() {
        let bytecode = compile_story_bytecode(
            "choice-save.story.hks",
            r#"
                let greeting = "restored"
                choice {
                    option("Route A") { "ignored" }
                    option("Route B") { "${greeting}" }
                }
            "#,
        )
        .expect("choice story must compile");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime must initialize");
        let event = runtime.step().expect("choice must suspend");
        assert!(
            matches!(
                event,
                Some(StoryRuntimeEvent::Choice { ref options, .. })
                    if options == &["Route A", "Route B"]
            ),
            "unexpected choice event: {event:?}"
        );

        let snapshot = runtime.snapshot().expect("waiting choice must be saveable");
        let mut restored = StoryRuntime::restore(bytecode, snapshot).expect("choice must restore");
        restored
            .resume(Value::Number(1.0))
            .expect("restored selection must start its branch");
        assert!(matches!(
            restored.step().expect("captured branch must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "restored"
        ));
    }

    #[test]
    fn parallel_tasks_continue_while_the_main_story_waits_for_input() {
        let bytecode = compile_story_bytecode(
            "parallel.story.hks",
            r#"
                par { voice("voice/first") }
                "dialogue"
            "#,
        )
        .expect("parallel story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        assert!(matches!(
            runtime.step().expect("dialogue effect must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
        assert_eq!(
            runtime.step().expect("dialogue must block the main VM"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
        let task = match runtime.step().expect("parallel voice must keep advancing") {
            Some(StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::PlayVoice { ref path, .. },
            }) if path == "voice/first" => task,
            event => panic!("unexpected task event: {event:?}"),
        };
        runtime
            .resume_task(task)
            .expect("audio completion must resume its task");
        assert_eq!(
            runtime.step().expect("finished task must become idle"),
            None
        );
    }

    #[test]
    fn sequence_voice_waits_for_each_host_completion() {
        let bytecode = compile_story_bytecode(
            "sequence.story.hks",
            r#"
                seq {
                    voice("voice/first")
                    voice("voice/second")
                }
                "dialogue"
            "#,
        )
        .expect("sequence story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        assert!(matches!(
            runtime.step().expect("dialogue effect must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
        assert!(matches!(
            runtime.step().expect("dialogue must block"),
            Some(StoryRuntimeEvent::Wait(_))
        ));
        let first = match runtime.step().expect("first voice must start") {
            Some(StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::PlayVoice { ref path, .. },
            }) if path == "voice/first" => task,
            event => panic!("unexpected first sequence event: {event:?}"),
        };
        assert_eq!(
            runtime.step().expect("sequence must remain suspended"),
            None
        );
        runtime
            .resume_task(first)
            .expect("first audio completion must resume the task");
        assert!(matches!(
            runtime.step().expect("second voice must follow completion"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { ref path, .. },
                ..
            }) if path == "voice/second"
        ));
    }

    #[test]
    fn wait_handle_resumes_the_main_vm_after_task_completion() {
        let bytecode = compile_story_bytecode(
            "wait.story.hks",
            r#"
                let voices = seq {
                    voice("voice/first")
                    voice("voice/second")
                }
                wait(voices)
                "after voices"
            "#,
        )
        .expect("wait story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        for expected in ["voice/first", "voice/second"] {
            let task = match runtime.step().expect("voice task must advance") {
                Some(StoryRuntimeEvent::TaskEffect {
                    task,
                    effect: StoryEffect::PlayVoice { ref path, .. },
                }) if path == expected => task,
                event => panic!("unexpected wait task event: {event:?}"),
            };
            runtime
                .resume_task(task)
                .expect("audio completion must resume sequence");
        }
        assert!(matches!(
            runtime.step().expect("main VM must resume after the task"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "after voices"
        ));
    }

    #[test]
    fn snapshot_replays_an_in_flight_task_effect() {
        let bytecode = compile_story_bytecode(
            "task-save.story.hks",
            r#"
                let voiceTask = seq { voice("voice/saved") }
                wait(voiceTask)
            "#,
        )
        .expect("task save story must compile");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime must initialize");
        assert!(matches!(
            runtime.step().expect("voice effect must start"),
            Some(StoryRuntimeEvent::TaskEffect { .. })
        ));
        let snapshot = runtime
            .snapshot()
            .expect("an externally waiting task must be saveable");
        let mut restored =
            StoryRuntime::restore(bytecode, snapshot).expect("snapshot must restore");
        assert!(matches!(
            restored.step().expect("restored effect must be replayed"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { ref path, .. },
                ..
            }) if path == "voice/saved"
        ));
    }

    #[test]
    fn representative_inline_stories_compile_as_whole_programs() {
        for (path, source) in [
            ("<bootstrap>", r#"gotoScript("chapter.hks")"#),
            (
                "<dialogue>",
                r#"
                    let alice = char("alice")
                    alice.at(.center).scale(0.5).e("happy")
                    alice: "Hello"
                    ...: " again"
                    "Narration"
                "#,
            ),
            (
                "<control-flow>",
                r#"
                    let count = 0
                    while count < 2 {
                        "Iteration ${count}"
                        count += 1
                    }
                    if count == 2 { log("done") }
                "#,
            ),
            (
                "<tasks>",
                r#"
                    let voices = par {
                        voice("voice/alice/first")
                        voice("voice/bob/second")
                    }
                    wait(voices)
                "#,
            ),
        ] {
            compile_story_bytecode(path, source).unwrap_or_else(|error| {
                panic!("`{path}` failed whole-program compilation: {error}")
            });
        }
    }
}
