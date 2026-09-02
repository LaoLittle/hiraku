//! Direct whole-program HKS execution state.
//!
//! It owns the generic VM and task scheduler while ECS systems own waits and effects.

use std::collections::{BTreeMap, VecDeque};

use hiraku_script::StatementValue;
use hiraku_script::TemplateError;
use hiraku_script::{
    BuiltinCall, Bytecode, LinkedBytecode, LinkedFunction, SymbolCall, Value, Vm, VmError, VmEvent,
    VmSnapshot, link_bytecode,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::task_runtime::{
    ExecutionMode, TaskEvent, TaskScheduler, TaskSchedulerError, TaskSchedulerSnapshot,
};
use crate::script::capabilities::{
    CharacterCapabilityError, StoryCallOutcome, StoryControl, StoryEffect, StoryNativeHost,
    StoryNativeHostSnapshot, StoryTaskKind, StoryWait, story_manifest,
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
    vm: Vm,
    scheduler: TaskScheduler,
    linked: LinkedBytecode,
    globals: BTreeMap<String, Value>,
}

/// Engine-facing whole-story driver. It translates generic VM boundaries into
/// story effects without introducing a second executable representation.
pub struct StoryRuntime {
    bytecode: HksRuntime,
    host: StoryNativeHost,
    pending: VecDeque<StoryRuntimeEvent>,
    active_task_effects: BTreeMap<u64, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<u64, Value>,
    waiting_task: Option<u64>,
    waiting_interactive_task: Option<u64>,
    choice: Option<ChoiceState>,
    blocked: bool,
    blocked_wait: Option<StoryWait>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ClosureValue(Value);

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
        arguments: Vec<Value>,
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
    active_task_effects: BTreeMap<u64, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<u64, Value>,
    waiting_task: Option<u64>,
    waiting_interactive_task: Option<u64>,
    choice: Option<ChoiceState>,
    blocked: bool,
    #[serde(default)]
    blocked_wait: Option<StoryWait>,
}

impl StoryRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, StoryRuntimeError> {
        Ok(Self {
            bytecode: HksRuntime::new(bytecode)?,
            host: StoryNativeHost::new(),
            pending: VecDeque::new(),
            active_task_effects: BTreeMap::new(),
            deferred_task_completions: BTreeMap::new(),
            waiting_task: None,
            waiting_interactive_task: None,
            choice: None,
            blocked: false,
            blocked_wait: None,
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
            deferred_task_completions: self.deferred_task_completions.clone(),
            waiting_task: self.waiting_task,
            waiting_interactive_task: self.waiting_interactive_task,
            choice: self.choice.clone(),
            blocked: self.blocked,
            blocked_wait: self.blocked_wait.clone(),
        })
    }

    pub fn restore(
        bytecode: Bytecode,
        mut snapshot: StoryRuntimeSnapshot,
    ) -> Result<Self, StoryRuntimeError> {
        // Voice playback is transient output rather than durable story state.
        // The scheduler has already consumed the native call and only keeps an
        // active effect so seq/wait can observe its completion. Loading must
        // complete that effect without emitting PlayVoice again.
        let mut completed_voice_tasks = Vec::new();
        for (task, effects) in &mut snapshot.active_task_effects {
            let had_voice = effects
                .iter()
                .any(|effect| matches!(effect, StoryEffect::PlayVoice { .. }));
            effects.retain(|effect| !matches!(effect, StoryEffect::PlayVoice { .. }));
            if had_voice && effects.is_empty() {
                completed_voice_tasks.push(*task);
            }
        }
        snapshot
            .active_task_effects
            .retain(|_, effects| !effects.is_empty());
        let pending = snapshot
            .active_task_effects
            .iter()
            .flat_map(|(task, effects)| {
                effects.iter().map(|effect| StoryRuntimeEvent::TaskEffect {
                    task: *task,
                    effect: effect.clone(),
                })
            })
            .collect();
        let mut runtime = Self {
            bytecode: HksRuntime::restore(bytecode, snapshot.bytecode)?,
            host: StoryNativeHost::restore(snapshot.host),
            pending,
            active_task_effects: snapshot.active_task_effects,
            deferred_task_completions: snapshot.deferred_task_completions,
            waiting_task: snapshot.waiting_task,
            waiting_interactive_task: snapshot.waiting_interactive_task,
            choice: snapshot.choice,
            blocked: snapshot.blocked,
            blocked_wait: snapshot.blocked_wait,
        };
        for task in completed_voice_tasks {
            runtime.finish_task_effects(task)?;
        }
        Ok(runtime)
    }

    pub fn set_globals(&mut self, globals: std::collections::BTreeMap<String, Value>) {
        self.bytecode.set_globals(globals);
    }

    pub fn globals(&self) -> &std::collections::BTreeMap<String, Value> {
        self.bytecode.globals()
    }

    pub(crate) fn enqueue_event(&mut self, event: StoryRuntimeEvent) {
        self.pending.push_back(event);
    }

    /// Reconstructs the host-visible boundary represented by a restored VM.
    /// The engine must not infer every blocked state as dialogue input: a
    /// waiting choice needs its prompt and options mounted again.
    pub fn restored_boundary_event(&self) -> Option<StoryRuntimeEvent> {
        if !self.blocked {
            return None;
        }
        match &self.choice {
            Some(ChoiceState::AwaitingSelection { prompt, options }) => {
                Some(StoryRuntimeEvent::Choice {
                    prompt: prompt.clone(),
                    options: options.iter().map(|option| option.label.clone()).collect(),
                })
            }
            _ => Some(StoryRuntimeEvent::Wait(
                self.blocked_wait
                    .clone()
                    .unwrap_or(StoryWait::DialogueAdvance),
            )),
        }
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
                .spawn_closure(&option, ExecutionMode::Interactive)?;
            self.choice = Some(ChoiceState::RunningBranch { task, selected });
            self.blocked = false;
            self.blocked_wait = None;
            return Ok(());
        }
        if let Some(task) = self.waiting_interactive_task.take() {
            self.blocked = false;
            self.blocked_wait = None;
            self.bytecode.unpause_task(task)?;
            return Ok(());
        }
        self.blocked = false;
        self.blocked_wait = None;
        if self.bytecode.main_waiting_for_host() {
            self.bytecode.resume_main(value)?;
        }
        Ok(())
    }

    /// Returns whether the story currently owns a host-side wait boundary.
    ///
    /// ECS completions can arrive after navigation or state restoration has
    /// invalidated their request. Callers must use this boundary state to
    /// discard such late completions instead of treating them as VM failures.
    pub fn is_waiting_for_host_response(&self) -> bool {
        self.blocked
    }

    pub fn resume_task(&mut self, task: u64) -> Result<(), StoryRuntimeError> {
        let effects = self
            .active_task_effects
            .get_mut(&task)
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        effects
            .pop()
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        if effects.is_empty() {
            self.active_task_effects.remove(&task);
            self.finish_task_effects(task)?;
        }
        Ok(())
    }

    fn finish_task_effects(&mut self, task: u64) -> Result<(), StoryRuntimeError> {
        if self.bytecode.task_mode(task) == Some(ExecutionMode::Sequence) {
            let _ = self.bytecode.unpause_task(task);
        }
        if let Some(value) = self.deferred_task_completions.remove(&task) {
            if self.waiting_task == Some(task) {
                self.waiting_task = None;
                self.bytecode.resume_main(value)?;
            }
        }
        Ok(())
    }

    pub fn step(&mut self) -> Result<Option<StoryRuntimeEvent>, StoryRuntimeError> {
        if let Some(event) = self.pending.pop_front() {
            self.mark_host_boundary(&event);
            return Ok(Some(event));
        }
        if self.blocked {
            loop {
                let Some(event) = self.bytecode.step_task()? else {
                    return Ok(None);
                };
                if let Some(event) = self.handle_task_event(event)? {
                    return Ok(Some(event));
                }
            }
        }
        loop {
            let Some(event) = self.bytecode.step()? else {
                return Ok(None);
            };
            match event {
                HksRuntimeEvent::Call(call) => match self.host.call(&call)? {
                    StoryCallOutcome::Return(value) => self.bytecode.resume_main(value)?,
                    StoryCallOutcome::Control(StoryControl::SpawnTask { kind, closure }) => {
                        let mode = match kind {
                            StoryTaskKind::Sequence => ExecutionMode::Sequence,
                            StoryTaskKind::Parallel => ExecutionMode::Parallel,
                        };
                        let task = self.bytecode.spawn_closure(&ClosureValue(closure), mode)?;
                        self.bytecode.resume_main(Value::Task(task))?;
                    }
                    StoryCallOutcome::Control(StoryControl::BeginChoice { prompt, closure }) => {
                        let builder_task = self
                            .bytecode
                            .spawn_closure(&ClosureValue(closure), ExecutionMode::Interactive)?;
                        self.choice = Some(ChoiceState::Collecting {
                            builder_task,
                            prompt,
                            options: Vec::new(),
                        });
                    }
                    StoryCallOutcome::Control(StoryControl::OpenUi { path, arguments }) => {
                        self.blocked = true;
                        return Ok(Some(StoryRuntimeEvent::OpenUi { path, arguments }));
                    }
                    StoryCallOutcome::Control(StoryControl::WaitTask { task }) => {
                        self.waiting_task = Some(task);
                    }
                    StoryCallOutcome::Control(control @ StoryControl::AddChoiceOption { .. }) => {
                        return Err(StoryRuntimeError::UnexpectedMainControl(control));
                    }
                },
                HksRuntimeEvent::Statement(statement) => {
                    self.host.handle_statement(&statement)?;
                    self.enqueue_host_boundaries();
                    if let Some(event) = self.pending.pop_front() {
                        self.mark_host_boundary(&event);
                        return Ok(Some(event));
                    }
                }
                event @ (HksRuntimeEvent::TaskCall { .. }
                | HksRuntimeEvent::TaskStatement { .. }
                | HksRuntimeEvent::TaskCompleted { .. }) => {
                    if let Some(event) = self.handle_task_event(event)? {
                        self.mark_host_boundary(&event);
                        return Ok(Some(event));
                    }
                }
                HksRuntimeEvent::Completed(value) => {
                    return Ok(Some(StoryRuntimeEvent::Completed(value)));
                }
            }
        }
    }

    fn mark_host_boundary(&mut self, event: &StoryRuntimeEvent) {
        if matches!(
            event,
            StoryRuntimeEvent::Wait(_)
                | StoryRuntimeEvent::OpenUi { .. }
                | StoryRuntimeEvent::Choice { .. }
        ) {
            self.blocked = true;
        }
        if let StoryRuntimeEvent::Wait(wait) = event {
            self.blocked_wait = Some(wait.clone());
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

    fn handle_task_event(
        &mut self,
        event: HksRuntimeEvent,
    ) -> Result<Option<StoryRuntimeEvent>, StoryRuntimeError> {
        match event {
            HksRuntimeEvent::TaskCall { task, call } => match self.host.call(&call)? {
                StoryCallOutcome::Return(value) => {
                    self.bytecode.resume_task(task, value)?;
                }
                StoryCallOutcome::Control(StoryControl::AddChoiceOption { label, closure }) => {
                    let Some(ChoiceState::Collecting { options, .. }) = &mut self.choice else {
                        return Err(StoryRuntimeError::InvalidChoice);
                    };
                    options.push(ChoiceOption {
                        label,
                        body: ClosureValue(closure),
                    });
                    self.bytecode.resume_task(task, Value::Unit)?;
                }
                StoryCallOutcome::Control(control) => {
                    return Err(StoryRuntimeError::UnsupportedTaskControl(control));
                }
            },
            HksRuntimeEvent::TaskStatement { task, value } => {
                self.host.handle_statement(&value)?;
                self.enqueue_task_boundaries(task)?;
                return Ok(self.pending.pop_front());
            }
            HksRuntimeEvent::TaskCompleted { task, value } => {
                if self.active_task_effects.contains_key(&task) {
                    self.deferred_task_completions.insert(task, value);
                    return Ok(None);
                }
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
                    return Ok(None);
                }
                if self.waiting_task == Some(task) {
                    self.waiting_task = None;
                    self.bytecode.resume_main(value)?;
                }
            }
            _ => unreachable!("task handler requires a task VM event"),
        }
        Ok(None)
    }

    fn enqueue_task_boundaries(&mut self, task: u64) -> Result<(), StoryRuntimeError> {
        for effect in self.host.drain_effects() {
            if matches!(effect, StoryEffect::PlayVoice { .. }) {
                self.active_task_effects
                    .entry(task)
                    .or_default()
                    .push(effect.clone());
                self.pending
                    .push_back(StoryRuntimeEvent::TaskEffect { task, effect });
            } else {
                self.pending.push_back(StoryRuntimeEvent::Effect(effect));
            }
        }
        let wait = self.host.take_wait();
        let task_mode = self.bytecode.task_mode(task);
        if matches!(wait, Some(StoryWait::Movie { .. }))
            && task_mode != Some(ExecutionMode::Interactive)
        {
            return Err(StoryRuntimeError::UnsupportedTaskWait(
                wait.expect("the movie wait was matched"),
            ));
        }
        let has_wait = wait.is_some();
        if let Some(wait) = wait
            && task_mode == Some(ExecutionMode::Interactive)
        {
            self.bytecode.pause_task(task)?;
            self.waiting_interactive_task = Some(task);
            self.pending.push_back(StoryRuntimeEvent::Wait(wait));
        }
        if task_mode == Some(ExecutionMode::Sequence)
            && has_wait
            && self.active_task_effects.contains_key(&task)
        {
            self.bytecode.pause_task(task)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HksRuntimeSnapshot {
    /// Exact executing bytecode. Restore relinks this against the current
    /// native registry instead of relying on source recompilation offsets.
    pub program: Bytecode,
    pub vm: VmSnapshot,
    pub scheduler: TaskSchedulerSnapshot,
}

impl HksRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, HksRuntimeError> {
        let linked =
            link_bytecode(bytecode.clone(), &story_manifest()).map_err(HksRuntimeError::Link)?;
        Ok(Self {
            vm: Vm::new(bytecode.clone())?,
            scheduler: TaskScheduler::new(bytecode),
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
        mode: ExecutionMode,
    ) -> Result<u64, HksRuntimeError> {
        self.scheduler
            .set_global_values(self.vm.globals().to_vec())?;
        Ok(self.scheduler.spawn(&closure.0, mode)?)
    }

    pub fn restore(
        _bytecode: Bytecode,
        snapshot: HksRuntimeSnapshot,
    ) -> Result<Self, HksRuntimeError> {
        let bytecode = snapshot.program;
        let linked =
            link_bytecode(bytecode.clone(), &story_manifest()).map_err(HksRuntimeError::Link)?;
        let vm = Vm::restore(bytecode.clone(), snapshot.vm)?;
        let globals = globals_from_values(&bytecode, vm.globals());
        Ok(Self {
            vm,
            scheduler: TaskScheduler::restore(bytecode, snapshot.scheduler)?,
            linked,
            globals,
        })
    }

    /// Advances either the main program or the first ready task to the next host boundary.
    pub fn step(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        if let Some(event) = self.vm.step()? {
            return match event {
                VmEvent::Call(call) => {
                    let mut call = self.link_call(call)?;
                    evaluate_call_templates(&mut call, |text| self.vm.eval_template(text))?;
                    Ok(Some(HksRuntimeEvent::Call(call)))
                }
                VmEvent::Statement(value) => {
                    let value = evaluate_statement_template(&mut self.vm, value)?;
                    self.refresh_globals();
                    self.scheduler
                        .set_global_values(self.vm.globals().to_vec())?;
                    Ok(Some(HksRuntimeEvent::Statement(value)))
                }
                VmEvent::Completed(value) => Ok(Some(HksRuntimeEvent::Completed(value))),
            };
        }

        self.step_task()
    }

    /// Advances only scheduled child tasks, leaving the main VM untouched.
    pub fn step_task(&mut self) -> Result<Option<HksRuntimeEvent>, HksRuntimeError> {
        match self.scheduler.step()? {
            Some(TaskEvent::Call { task, call }) => {
                let mut call = self.link_call(call)?;
                evaluate_call_templates(&mut call, |text| {
                    self.scheduler.eval_template(task, text)
                })?;
                Ok(Some(HksRuntimeEvent::TaskCall { task, call }))
            }
            Some(TaskEvent::Statement { task, value }) => {
                let value = evaluate_task_statement_template(&mut self.scheduler, task, value)?;
                self.vm
                    .set_global_values(self.scheduler.global_values().to_vec())?;
                self.refresh_globals();
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
        matches!(self.vm.status(), hiraku_script::VmStatus::WaitingForHost)
    }

    pub fn resume_task(&mut self, task: u64, value: Value) -> Result<(), HksRuntimeError> {
        self.scheduler.resume(task, value)?;
        Ok(())
    }

    pub fn pause_task(&mut self, task: u64) -> Result<(), HksRuntimeError> {
        self.scheduler.pause(task)?;
        Ok(())
    }

    pub fn unpause_task(&mut self, task: u64) -> Result<(), HksRuntimeError> {
        self.scheduler.unpause(task)?;
        Ok(())
    }

    fn task_mode(&self, task: u64) -> Option<ExecutionMode> {
        self.scheduler.mode(task)
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
    Vm(VmError),
    #[error("HKS task scheduler failed: {0}")]
    Task(#[from] TaskSchedulerError),
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
    #[error("choice requires a string prompt and a list of string options")]
    InvalidChoice,
    #[error("story control {0:?} cannot be issued by the main program")]
    UnexpectedMainControl(StoryControl),
    #[error("story control {0:?} is not supported inside a task closure")]
    UnsupportedTaskControl(StoryControl),
    #[error(
        "story wait {0:?} is not supported inside seq/par; call it from the main story or an interactive choice branch"
    )]
    UnsupportedTaskWait(StoryWait),
    #[error("story runtime is not waiting for a host response")]
    NotBlocked,
    #[error("story runtime snapshot requires an empty effect queue")]
    NotAtSnapshotBoundary,
    #[error("task {0} has no pending host effect")]
    UnknownTaskEffect(u64),
}

fn evaluate_statement_template(
    vm: &mut Vm,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(vm.eval_template(&text)?)),
        // Story expression values are implementation details (for example an
        // Actor handle). At the public statement boundary they mean commit.
        StatementValue::Value(_) => Ok(StatementValue::Commit),
        value => Ok(value),
    }
}

fn evaluate_task_statement_template(
    scheduler: &mut TaskScheduler,
    task: u64,
    value: StatementValue,
) -> Result<StatementValue, TemplateError> {
    match value {
        StatementValue::String(text) => Ok(StatementValue::String(
            scheduler.eval_template(task, &text)?,
        )),
        StatementValue::Value(_) => Ok(StatementValue::Commit),
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

impl From<VmError> for HksRuntimeError {
    fn from(error: VmError) -> Self {
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
        let mut runtime = HksRuntime::new(bytecode).expect("script runtime must initialize");
        let Some(HksRuntimeEvent::Call(call)) = runtime.step().expect("runtime must advance")
        else {
            panic!("expected a native call")
        };
        assert_eq!(
            call.builtin,
            story_manifest().resolve("log").expect("log registration")
        );
        runtime
            .resume_main(Value::Unit)
            .expect("host result must resume the main VM");
    }

    #[test]
    fn ui_roles_are_engine_effects_and_ui_open_is_a_selector_call() {
        let bytecode = compile_story_bytecode(
            "ui_roles.hks",
            concat!(
                "ui.set(\"dialogue\", \"ui/dialogue.ui.hks\")\n",
                "ui.mount(\"clock\", \"ui/clock.ui.hks\")\n",
                "ui.unmount(\"clock\")\n",
                "ui.open(\"dialogue\", \"Alice\", 3)",
            ),
        )
        .expect("UI role APIs must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story runtime must initialize");
        assert_eq!(
            runtime.step().expect("ui.set must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::SetUiRole {
                role: "dialogue".to_string(),
                component: "ui/dialogue.ui.hks".to_string(),
            }))
        );
        assert_eq!(
            runtime.step().expect("ui.mount must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::MountUiOverlay {
                name: "clock".to_string(),
                component: "ui/clock.ui.hks".to_string(),
            }))
        );
        assert_eq!(
            runtime.step().expect("ui.unmount must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::UnmountUiOverlay {
                name: "clock".to_string(),
            }))
        );
        assert_eq!(
            runtime.step().expect("ui.open must run"),
            Some(StoryRuntimeEvent::OpenUi {
                path: "dialogue".to_string(),
                arguments: vec![Value::String("Alice".to_string()), Value::Number(3.0)],
            })
        );
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
            .resume_main(Value::Unit)
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
        let mut runtime = HksRuntime::new(bytecode).expect("script runtime must initialize");
        let mut host = StoryNativeHost::new();

        loop {
            match runtime.step().expect("runtime must advance") {
                Some(HksRuntimeEvent::Call(call)) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
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
    fn fluent_bgm_and_actor_focus_commit_as_typed_effects() {
        let bytecode = compile_story_bytecode(
            "fluent.story.hks",
            r#"
                bgm("music/theme").volume(0.75).fadeIn(600)
                char("alice").focus()
                char("bob").focus(false)
                camera().blur(2)
                camera(.canvas)
                    .offset(10, 20, 30)
                    .rotation(1, 2, 3)
                    .zoom(1.25)
                    .projection(.perspective)
                    .time(0.5)
                    .easing(.easeOut)
            "#,
        )
        .expect("fluent engine APIs must compile");
        let mut runtime = HksRuntime::new(bytecode).expect("script runtime must initialize");
        let mut host = StoryNativeHost::new();

        loop {
            match runtime.step().expect("runtime must advance") {
                Some(HksRuntimeEvent::Call(call)) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
                    runtime
                        .resume_main(value)
                        .expect("native result must resume the VM");
                }
                Some(HksRuntimeEvent::Statement(value)) => host
                    .handle_statement(&value)
                    .expect("statement commit must succeed"),
                Some(HksRuntimeEvent::Completed(_)) => break,
                Some(event) => panic!("unexpected runtime event: {event:?}"),
                None => panic!("runtime stopped before completion"),
            }
        }

        let effects = host.drain_effects();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::PlayBgm { path, volume, fade_in_ms: Some(600) }
                if path == "music/theme" && (*volume - 0.75).abs() < f32::EPSILON
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::ShowCharacter { actor_id, focused: true, .. } if actor_id == "alice"
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::ShowCharacter { actor_id, focused: false, .. } if actor_id == "bob"
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::SetCamera {
                blur: Some(blur),
                scope: crate::script::CameraEffectScope::World,
                ..
            } if (*blur - 2.0).abs() < f32::EPSILON
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::SetCamera {
                zoom: Some(zoom),
                offset: Some([10.0, 20.0, 30.0]),
                rotation: Some([1.0, 2.0, 3.0]),
                projection: Some(crate::script::CameraProjectionMode::Perspective),
                duration_ms: 500,
                ease,
                scope: crate::script::CameraEffectScope::Canvas,
                ..
            } if (*zoom - 1.25).abs() < f32::EPSILON && ease == "easeOut"
        )));
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
            .resume(Value::Unit)
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
                "after choice"
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
        assert_eq!(
            runtime.step().expect("selected branch must wait for input"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
        runtime
            .resume(Value::Unit)
            .expect("branch dialogue must resume independently of the main VM");
        assert!(matches!(
            runtime.step().expect("main story continues after the branch"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "after choice"
        ));
    }

    #[test]
    fn movie_wait_inside_a_choice_branch_resumes_that_branch() {
        let bytecode = compile_story_bytecode(
            "choice-movie.story.hks",
            r#"
                choice {
                    option("Play movie") {
                        movie("opening")
                        "after movie"
                    }
                }
                "after choice"
            "#,
        )
        .expect("choice movie story must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("story driver must initialize");
        assert!(matches!(
            runtime.step().expect("choice must suspend"),
            Some(StoryRuntimeEvent::Choice { .. })
        ));
        runtime
            .resume(Value::Number(0.0))
            .expect("choice response must start the selected branch");
        assert_eq!(
            runtime.step().expect("movie must suspend its branch"),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "opening".into(),
            }))
        );
        assert!(runtime.is_waiting_for_host_response());
        runtime
            .resume(Value::Unit)
            .expect("movie completion must resume the selected branch");
        assert!(matches!(
            runtime.step().expect("branch dialogue must run after movie"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. }))
                if text == "after movie"
        ));
        assert_eq!(
            runtime.step().expect("branch dialogue must await input"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
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
        assert_eq!(
            restored.restored_boundary_event(),
            Some(StoryRuntimeEvent::Choice {
                prompt: String::new(),
                options: vec!["Route A".into(), "Route B".into()],
            })
        );
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
    fn a_blocked_movie_wait_survives_snapshot_restore() {
        let bytecode = compile_story_bytecode(
            "movie-save.hks",
            "movie(\"movies/opening.mkv\")\n\"after movie\"",
        )
        .expect("movie story must compile");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime must initialize");
        assert_eq!(
            runtime.step().expect("movie must suspend"),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "movies/opening.mkv".into(),
            }))
        );
        let snapshot = runtime.snapshot().expect("movie wait must be saveable");
        let restored = StoryRuntime::restore(bytecode, snapshot).expect("movie wait must restore");
        assert_eq!(
            restored.restored_boundary_event(),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "movies/opening.mkv".into(),
            }))
        );
    }

    #[test]
    fn parallel_tasks_continue_while_the_main_story_waits_for_input() {
        let bytecode = compile_story_bytecode(
            "parallel.story.hks",
            r#"
                par {
                    voice("voice/first")
                    voice("voice/second")
                }
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
        assert!(matches!(
            runtime.step().expect("second parallel voice must start without waiting"),
            Some(StoryRuntimeEvent::TaskEffect {
                task: second_task,
                effect: StoryEffect::PlayVoice { ref path, .. },
            }) if second_task == task && path == "voice/second"
        ));
        runtime
            .resume_task(task)
            .expect("one parallel audio completion must be recorded");
        runtime
            .resume_task(task)
            .expect("the other parallel audio completion must be recorded");
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
                    "first line"
                    voice("voice/second")
                    "second line"
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
        assert!(matches!(
            runtime.step().expect("the first line must be displayed immediately"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. })) if text == "first line"
        ));
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
    fn snapshot_completes_an_in_flight_voice_without_replaying_it() {
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
        loop {
            match restored.step().expect("restored story must continue") {
                Some(StoryRuntimeEvent::TaskEffect {
                    effect: StoryEffect::PlayVoice { .. },
                    ..
                }) => panic!("loading must not replay an in-flight voice"),
                Some(StoryRuntimeEvent::Completed(_)) => break,
                Some(_) => {}
                None => panic!("restored story stopped before completing"),
            }
        }
    }

    #[test]
    fn representative_inline_stories_compile_as_whole_programs() {
        for (path, source) in [
            ("<bootstrap>", r#"story.goto("chapter.hks")"#),
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
