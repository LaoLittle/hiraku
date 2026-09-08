//! Engine-facing story policy built on the generic execution runtime.
//!
//! It owns story capabilities and wait policy while ECS systems own effects.

use std::collections::{BTreeMap, VecDeque};

use hiraku_script::{Bytecode, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::execution_runtime::{
    ExecutionEvent, ExecutionId, ExecutionMode, ExecutionRuntime, ExecutionRuntimeError,
    ExecutionRuntimeSnapshot,
};
use crate::script::capabilities::{
    CharacterCapabilityError, StoryCallOutcome, StoryControl, StoryEffect, StoryNativeHost,
    StoryNativeHostSnapshot, StoryTaskKind, StoryWait,
};

/// Engine-facing whole-story driver. It translates generic VM boundaries into
/// story effects without introducing a second executable representation.
pub struct StoryRuntime {
    execution: ExecutionRuntime,
    host: StoryNativeHost,
    pending: VecDeque<StoryRuntimeEvent>,
    active_task_effects: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<ExecutionId, Value>,
    deferred_dialogue: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    completed_groups: std::collections::BTreeSet<ExecutionId>,
    waiting_task: Option<ExecutionId>,
    waiting_interactive_task: Option<ExecutionId>,
    awaiting_effects: std::collections::BTreeSet<ExecutionId>,
    choice: Option<ChoiceState>,
    blocked: bool,
    terminated: bool,
    blocked_wait: Option<StoryWait>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ChoiceOption {
    label: String,
    enabled: bool,
    body: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum ChoiceState {
    Collecting {
        builder_task: ExecutionId,
        prompt: String,
        options: Vec<ChoiceOption>,
    },
    AwaitingSelection {
        prompt: String,
        options: Vec<ChoiceOption>,
    },
    RunningBranch {
        task: ExecutionId,
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
        enabled: Vec<bool>,
    },
    TaskEffect {
        task: ExecutionId,
        effect: StoryEffect,
    },
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoryRuntimeSnapshot {
    awaiting_effects: std::collections::BTreeSet<ExecutionId>,
    execution: ExecutionRuntimeSnapshot,
    host: StoryNativeHostSnapshot,
    active_task_effects: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<ExecutionId, Value>,
    #[serde(default)]
    deferred_dialogue: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    #[serde(default)]
    completed_groups: std::collections::BTreeSet<ExecutionId>,
    waiting_task: Option<ExecutionId>,
    waiting_interactive_task: Option<ExecutionId>,
    choice: Option<ChoiceState>,
    blocked: bool,
    terminated: bool,
    #[serde(default)]
    blocked_wait: Option<StoryWait>,
}

impl StoryRuntime {
    pub fn new(bytecode: Bytecode) -> Result<Self, StoryRuntimeError> {
        Ok(Self {
            execution: ExecutionRuntime::new(bytecode)?,
            host: StoryNativeHost::new(),
            pending: VecDeque::new(),
            active_task_effects: BTreeMap::new(),
            deferred_task_completions: BTreeMap::new(),
            deferred_dialogue: BTreeMap::new(),
            completed_groups: Default::default(),
            waiting_task: None,
            waiting_interactive_task: None,
            awaiting_effects: Default::default(),
            choice: None,
            blocked: false,
            terminated: false,
            blocked_wait: None,
        })
    }

    pub fn snapshot(&self) -> Result<StoryRuntimeSnapshot, StoryRuntimeError> {
        if !self.pending.is_empty() {
            return Err(StoryRuntimeError::NotAtSnapshotBoundary);
        }
        Ok(StoryRuntimeSnapshot {
            awaiting_effects: self.awaiting_effects.clone(),
            execution: self.execution.snapshot(),
            host: self.host.snapshot(),
            active_task_effects: self.active_task_effects.clone(),
            deferred_task_completions: self.deferred_task_completions.clone(),
            deferred_dialogue: self.deferred_dialogue.clone(),
            completed_groups: self.completed_groups.clone(),
            waiting_task: self.waiting_task,
            waiting_interactive_task: self.waiting_interactive_task,
            choice: self.choice.clone(),
            blocked: self.blocked,
            terminated: self.terminated,
            blocked_wait: self.blocked_wait.clone(),
        })
    }

    pub fn restore(
        bytecode: Bytecode,
        mut snapshot: StoryRuntimeSnapshot,
    ) -> Result<Self, StoryRuntimeError> {
        // Voice playback is transient output rather than durable story state.
        // The execution has already consumed the native call and only keeps an
        // active effect so seq/wait can observe its completion. Loading must
        // complete that effect without emitting PlayVoice again.
        let mut completed_voice_tasks = Vec::new();
        for (task, effects) in &mut snapshot.active_task_effects {
            let had_voice = effects.iter().any(|effect| {
                matches!(
                    effect,
                    StoryEffect::PlayVoice { .. } | StoryEffect::PlaySfx { .. }
                )
            });
            effects.retain(|effect| {
                !matches!(
                    effect,
                    StoryEffect::PlayVoice { .. } | StoryEffect::PlaySfx { .. }
                )
            });
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
            awaiting_effects: snapshot.awaiting_effects,
            execution: ExecutionRuntime::restore(bytecode, snapshot.execution)?,
            host: StoryNativeHost::restore(snapshot.host),
            pending,
            active_task_effects: snapshot.active_task_effects,
            deferred_task_completions: snapshot.deferred_task_completions,
            deferred_dialogue: snapshot.deferred_dialogue,
            completed_groups: snapshot.completed_groups,
            waiting_task: snapshot.waiting_task,
            waiting_interactive_task: snapshot.waiting_interactive_task,
            choice: snapshot.choice,
            blocked: snapshot.blocked,
            terminated: snapshot.terminated,
            blocked_wait: snapshot.blocked_wait,
        };
        for task in completed_voice_tasks {
            runtime.finish_task_effects(task)?;
        }
        Ok(runtime)
    }

    pub fn set_globals(&mut self, globals: std::collections::BTreeMap<String, Value>) {
        self.execution.set_globals(globals);
    }

    pub fn globals(&self) -> &std::collections::BTreeMap<String, Value> {
        self.execution.globals()
    }

    pub(crate) fn voice_playback_mode(&self, task: ExecutionId) -> super::VoicePlaybackMode {
        match self.execution.mode(task) {
            Some(ExecutionMode::Parallel | ExecutionMode::Sequence) => {
                super::VoicePlaybackMode::Concurrent
            }
            _ => super::VoicePlaybackMode::Exclusive,
        }
    }

    /// Native handles and their retained state belong to the story session,
    /// not to an individual called file. VM wait state is deliberately not copied.
    pub(crate) fn inherit_native_state(&mut self, previous: &Self, reset_presentation: bool) {
        self.host = StoryNativeHost::restore(previous.host.snapshot());
        if reset_presentation {
            self.host.reset_presentation();
        }
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
                    enabled: options.iter().map(|option| option.enabled).collect(),
                })
            }
            _ => Some(StoryRuntimeEvent::Wait(
                self.blocked_wait
                    .clone()
                    .unwrap_or(StoryWait::DialogueAdvance),
            )),
        }
    }

    pub fn accepts_choice_response(&self, value: &Value) -> bool {
        let Some(ChoiceState::AwaitingSelection { options, .. }) = &self.choice else {
            return true;
        };
        let Value::Number(index) = value else {
            return false;
        };
        index.is_finite()
            && *index >= 0.0
            && index.fract() == 0.0
            && options
                .get(*index as usize)
                .is_some_and(|option| option.enabled)
    }

    pub fn resume(&mut self, value: Value) -> Result<(), StoryRuntimeError> {
        if self.terminated {
            return Err(StoryRuntimeError::Terminated);
        }
        if !self.blocked {
            return Err(StoryRuntimeError::NotBlocked);
        }
        if !self.accepts_choice_response(&value) {
            return Ok(());
        }
        if let Some(ChoiceState::AwaitingSelection { options, .. }) = &self.choice {
            let Value::Number(selected) = value else {
                return Err(StoryRuntimeError::InvalidChoice);
            };
            let selected = selected as usize;
            if options.get(selected).is_some_and(|option| !option.enabled) {
                return Ok(());
            }
            let closure = options
                .get(selected)
                .ok_or(StoryRuntimeError::InvalidChoice)?
                .body
                .clone();
            let task = self.execution.spawn(&closure, ExecutionMode::Interactive)?;
            self.choice = Some(ChoiceState::RunningBranch { task, selected });
            self.blocked = false;
            self.blocked_wait = None;
            return Ok(());
        }
        if let Some(task) = self.waiting_interactive_task.take() {
            self.blocked = false;
            self.blocked_wait = None;
            self.execution.unpause(task)?;
            return Ok(());
        }
        self.blocked = false;
        self.blocked_wait = None;
        if self.execution.is_waiting_for_host(ExecutionId::MAIN) {
            self.execution.resume(ExecutionId::MAIN, value)?;
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

    /// A movie owns interaction until its matching host completion arrives.
    pub fn blocks_ui_input(&self) -> bool {
        self.blocked && matches!(self.blocked_wait, Some(StoryWait::Movie { .. }))
    }

    /// Completes the dispatched effect, not the most recently submitted effect.
    /// Identical outstanding effects are interchangeable; distinct effects must
    /// remain associated with their own ECS completion request across snapshots.
    pub fn complete_task_effect(
        &mut self,
        task: ExecutionId,
        completed: &StoryEffect,
    ) -> Result<(), StoryRuntimeError> {
        let effects = self
            .active_task_effects
            .get_mut(&task)
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        let index = effects
            .iter()
            .position(|effect| effect == completed)
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        effects.remove(index);
        if effects.is_empty() {
            self.active_task_effects.remove(&task);
            self.finish_task_effects(task)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn resume_task(&mut self, task: ExecutionId) -> Result<(), StoryRuntimeError> {
        let effect = self
            .active_task_effects
            .get(&task)
            .and_then(|effects| effects.first())
            .cloned()
            .ok_or(StoryRuntimeError::UnknownTaskEffect(task))?;
        self.complete_task_effect(task, &effect)
    }

    fn finish_task_effects(&mut self, task: ExecutionId) -> Result<(), StoryRuntimeError> {
        if let Some(dialogue) = self.deferred_dialogue.remove(&task) {
            for effect in dialogue {
                self.active_task_effects
                    .entry(task)
                    .or_default()
                    .push(effect.clone());
                self.pending
                    .push_back(StoryRuntimeEvent::TaskEffect { task, effect });
            }
            return Ok(());
        }
        if self.awaiting_effects.remove(&task)
            || self.execution.mode(task) == Some(ExecutionMode::Sequence)
        {
            let _ = self.execution.unpause(task);
        }
        if let Some(value) = self.deferred_task_completions.remove(&task) {
            self.completed_groups.insert(task);
            if self.waiting_task == Some(task) {
                self.waiting_task = None;
                self.execution.resume(ExecutionId::MAIN, value)?;
            }
        }
        Ok(())
    }

    pub fn step(&mut self) -> Result<Option<StoryRuntimeEvent>, StoryRuntimeError> {
        if self.terminated {
            return Ok(None);
        }
        // Choice branches can outlive their builder VM. They and deferred
        // completions are explicit host roots, not incidental register liveness.
        let mut roots = self.deferred_task_completions.values().collect::<Vec<_>>();
        if let Some(
            ChoiceState::Collecting { options, .. }
            | ChoiceState::AwaitingSelection { options, .. },
        ) = &self.choice
        {
            roots.extend(options.iter().map(|option| &option.body));
        }
        for event in &self.pending {
            match event {
                StoryRuntimeEvent::OpenUi { arguments, .. } => roots.extend(arguments),
                StoryRuntimeEvent::Completed(value) => roots.push(value),
                _ => {}
            }
        }
        self.execution.collect_objects_if_due(&roots)?;
        let mut budget = 10_000;
        if let Some(event) = self.pending.pop_front() {
            self.mark_host_boundary(&event);
            return Ok(Some(event));
        }
        if self.blocked {
            loop {
                let Some(event) = self.execution.step_children_with_budget(&mut budget)? else {
                    return Ok(None);
                };
                if let Some(event) = self.handle_task_event(event)? {
                    return Ok(Some(event));
                }
            }
        }
        loop {
            let Some(event) = self.execution.step_with_budget(&mut budget)? else {
                return Ok(None);
            };
            match event {
                ExecutionEvent::Call { execution, call } if execution.is_main() => {
                    match self.host.call(&call)? {
                        StoryCallOutcome::Control(StoryControl::Navigate(request)) => {
                            self.terminated = true;
                            return Ok(Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(
                                request,
                            ))));
                        }
                        StoryCallOutcome::Return(value) => {
                            self.execution.resume(ExecutionId::MAIN, value)?
                        }
                        StoryCallOutcome::Control(StoryControl::SpawnTask { kind, closure }) => {
                            let mode = match kind {
                                StoryTaskKind::Sequence => ExecutionMode::Sequence,
                                StoryTaskKind::Parallel => ExecutionMode::Parallel,
                            };
                            let task = self.execution.spawn(&closure, mode)?;
                            self.execution
                                .resume(ExecutionId::MAIN, Value::Task(task.task_handle()))?;
                        }
                        StoryCallOutcome::Control(StoryControl::BeginChoice {
                            prompt,
                            closure,
                        }) => {
                            let builder_task =
                                self.execution.spawn(&closure, ExecutionMode::Interactive)?;
                            self.choice = Some(ChoiceState::Collecting {
                                builder_task,
                                prompt,
                                options: Vec::new(),
                            });
                        }
                        StoryCallOutcome::Control(StoryControl::OpenUi { path, arguments }) => {
                            let arguments = arguments
                                .iter()
                                .map(|value| self.execution.export_value(value))
                                .collect::<Result<_, _>>()?;
                            self.blocked = true;
                            return Ok(Some(StoryRuntimeEvent::OpenUi { path, arguments }));
                        }
                        StoryCallOutcome::Control(StoryControl::WaitTask { task }) => {
                            let task = ExecutionId::from_task_handle(task);
                            if self.completed_groups.contains(&task) {
                                self.execution.resume(ExecutionId::MAIN, Value::Unit)?;
                            } else {
                                self.waiting_task = Some(task);
                            }
                        }
                        StoryCallOutcome::Control(
                            control @ (StoryControl::AddChoiceOption { .. }
                            | StoryControl::EnableChoiceOption { .. }),
                        ) => {
                            return Err(StoryRuntimeError::UnexpectedMainControl(control));
                        }
                    }
                }
                ExecutionEvent::Statement { execution, value } if execution.is_main() => {
                    let statement = value;
                    self.host.handle_statement(&statement)?;
                    self.enqueue_host_boundaries()?;
                    if let Some(event) = self.pending.pop_front() {
                        self.mark_host_boundary(&event);
                        return Ok(Some(event));
                    }
                }
                event @ (ExecutionEvent::Call { .. }
                | ExecutionEvent::Statement { .. }
                | ExecutionEvent::Completed { .. })
                    if !event.execution().is_main() =>
                {
                    if let Some(event) = self.handle_task_event(event)? {
                        self.mark_host_boundary(&event);
                        return Ok(Some(event));
                    }
                }
                ExecutionEvent::Completed { execution, value } if execution.is_main() => {
                    return Ok(Some(StoryRuntimeEvent::Completed(value)));
                }
                _ => unreachable!("execution event guard must classify main or child execution"),
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

    fn enqueue_host_boundaries(&mut self) -> Result<(), StoryRuntimeError> {
        let explicit = self.host.take_animation_await();
        if explicit {
            let task = ExecutionId::MAIN;
            for effect in self.host.drain_effects() {
                if animation_effect(&effect) {
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
            if self.active_task_effects.contains_key(&task) {
                self.awaiting_effects.insert(task);
                self.execution.pause(task)?;
            }
        } else {
            self.pending.extend(
                self.host
                    .drain_effects()
                    .into_iter()
                    .map(|effect| match effect {
                        StoryEffect::Delay { duration_ms } => {
                            StoryRuntimeEvent::Wait(StoryWait::Delay { duration_ms })
                        }
                        effect => StoryRuntimeEvent::Effect(effect),
                    }),
            );
        }
        if let Some(wait) = self.host.take_wait() {
            self.pending.push_back(StoryRuntimeEvent::Wait(wait));
        }
        Ok(())
    }

    fn handle_task_event(
        &mut self,
        event: ExecutionEvent,
    ) -> Result<Option<StoryRuntimeEvent>, StoryRuntimeError> {
        match event {
            ExecutionEvent::Call {
                execution: task,
                call,
            } => match self.host.call(&call)? {
                StoryCallOutcome::Control(StoryControl::Navigate(request)) => {
                    self.terminated = true;
                    return Ok(Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(
                        request,
                    ))));
                }
                StoryCallOutcome::Return(value) => {
                    self.execution.resume(task, value)?;
                }
                StoryCallOutcome::Control(StoryControl::AddChoiceOption { label, closure }) => {
                    let Some(ChoiceState::Collecting {
                        options,
                        builder_task,
                        ..
                    }) = &mut self.choice
                    else {
                        return Err(StoryRuntimeError::InvalidChoice);
                    };
                    if *builder_task != task || task.task_handle() > u32::MAX as u64 {
                        return Err(StoryRuntimeError::InvalidChoice);
                    }
                    let id = (task.task_handle() << 32) | options.len() as u64;
                    options.push(ChoiceOption {
                        label,
                        enabled: true,
                        body: closure,
                    });
                    self.execution.resume(
                        task,
                        Value::Handle {
                            type_id: super::capabilities::CHOICE_OPTION_HANDLE_TYPE,
                            id,
                        },
                    )?;
                }
                StoryCallOutcome::Control(StoryControl::EnableChoiceOption { id, enabled }) => {
                    let Some(ChoiceState::Collecting {
                        options,
                        builder_task,
                        ..
                    }) = &mut self.choice
                    else {
                        return Err(StoryRuntimeError::InvalidChoice);
                    };
                    if *builder_task != task || id >> 32 != task.task_handle() {
                        return Err(StoryRuntimeError::InvalidChoice);
                    }
                    let option = options
                        .get_mut((id & u32::MAX as u64) as usize)
                        .ok_or(StoryRuntimeError::InvalidChoice)?;
                    option.enabled = enabled;
                    self.execution.resume(
                        task,
                        Value::Handle {
                            type_id: super::capabilities::CHOICE_OPTION_HANDLE_TYPE,
                            id,
                        },
                    )?;
                }
                StoryCallOutcome::Control(control) => {
                    return Err(StoryRuntimeError::UnsupportedTaskControl(control));
                }
            },
            ExecutionEvent::Statement {
                execution: task,
                value,
            } => {
                self.host.handle_statement(&value)?;
                self.enqueue_task_boundaries(task)?;
                return Ok(self.pending.pop_front());
            }
            ExecutionEvent::Completed {
                execution: task,
                value,
            } => {
                if self.active_task_effects.contains_key(&task) {
                    self.deferred_task_completions.insert(task, value);
                    return Ok(None);
                }
                self.completed_groups.insert(task);
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
                    let enabled = options.iter().map(|option| option.enabled).collect();
                    self.choice = Some(ChoiceState::AwaitingSelection {
                        prompt: prompt.clone(),
                        options,
                    });
                    self.blocked = true;
                    return Ok(Some(StoryRuntimeEvent::Choice {
                        enabled,
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
                    self.execution
                        .resume(ExecutionId::MAIN, Value::Number(selected as f64))?;
                    return Ok(None);
                }
                if self.waiting_task == Some(task) {
                    self.waiting_task = None;
                    self.execution.resume(ExecutionId::MAIN, value)?;
                }
            }
        }
        Ok(None)
    }

    fn enqueue_task_boundaries(&mut self, task: ExecutionId) -> Result<(), StoryRuntimeError> {
        let explicit = self.host.take_animation_await();
        let mut interactive_delay = None;
        let task_mode = self.execution.mode(task);
        for effect in self.host.drain_effects() {
            let dialogue = matches!(
                effect,
                StoryEffect::Say { .. } | StoryEffect::ContinueDialogue { .. }
            );
            if dialogue && task_mode == Some(ExecutionMode::Parallel) {
                bevy::log::warn!(
                    "say/narrate and dialogue continuation are not allowed in par; statement skipped"
                );
                continue;
            }
            if dialogue
                && task_mode == Some(ExecutionMode::Sequence)
                && self.active_task_effects.contains_key(&task)
            {
                self.deferred_dialogue.entry(task).or_default().push(effect);
                continue;
            }
            if let StoryEffect::Delay { duration_ms } = effect
                && task_mode == Some(ExecutionMode::Interactive)
            {
                interactive_delay = Some(StoryWait::Delay { duration_ms });
                continue;
            }
            if matches!(
                effect,
                StoryEffect::PlayVoice { .. }
                    | StoryEffect::PlaySfx { .. }
                    | StoryEffect::Delay { .. }
            ) || ((explicit || task_mode != Some(ExecutionMode::Interactive))
                && animation_effect(&effect))
                || (dialogue && task_mode == Some(ExecutionMode::Sequence))
            {
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
        let wait = self.host.take_wait().or(interactive_delay);
        if matches!(wait, Some(StoryWait::Movie { .. }))
            && task_mode != Some(ExecutionMode::Interactive)
        {
            return Err(StoryRuntimeError::UnsupportedTaskWait(
                wait.expect("an interactive-only wait was matched"),
            ));
        }
        if let Some(wait) = wait
            && task_mode == Some(ExecutionMode::Interactive)
        {
            self.execution.pause(task)?;
            self.waiting_interactive_task = Some(task);
            self.pending.push_back(StoryRuntimeEvent::Wait(wait));
        }
        if (explicit || task_mode == Some(ExecutionMode::Sequence))
            && self.active_task_effects.contains_key(&task)
        {
            self.awaiting_effects.insert(task);
            self.execution.pause(task)?;
        }
        Ok(())
    }
}

fn animation_effect(effect: &StoryEffect) -> bool {
    matches!(
        effect,
        StoryEffect::SetCamera { .. }
            | StoryEffect::SetBackground { .. }
            | StoryEffect::ShowCharacter { .. }
            | StoryEffect::HideCharacter { .. }
            | StoryEffect::ActorMotion { .. }
            | StoryEffect::SetCurtain { .. }
            | StoryEffect::Picture(_)
            | StoryEffect::PlayBgm { .. }
            | StoryEffect::PlayVoice { .. }
            | StoryEffect::PlaySfx { .. }
    )
}

#[derive(Debug, Error)]
pub enum StoryRuntimeError {
    #[error("story execution has terminated; it cannot accept a host response")]
    Terminated,
    #[error(transparent)]
    Bytecode(#[from] ExecutionRuntimeError),
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
    UnknownTaskEffect(ExecutionId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::capabilities::{
        StoryEffect, StoryNativeHost, compile_story_bytecode, story_manifest,
    };

    #[test]
    fn out_of_order_completions_preserve_the_correct_snapshot_effects() {
        let bytecode = compile_story_bytecode(
            "test.hks",
            r#"
            let group = par { voice("voice/alice") voice("voice/bob") }
            group.await()
        "#,
        )
        .expect("parallel voices compile");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime initializes");
        let Some(StoryRuntimeEvent::TaskEffect {
            task,
            effect: first,
        }) = runtime.step().expect("first voice")
        else {
            panic!("expected first voice");
        };
        let Some(StoryRuntimeEvent::TaskEffect { effect: second, .. }) =
            runtime.step().expect("second voice")
        else {
            panic!("expected second voice");
        };
        assert_eq!(runtime.step().expect("group waits"), None);
        runtime
            .complete_task_effect(task, &first)
            .expect("first voice finishes first");
        let snapshot = runtime.snapshot().expect("snapshot pending second voice");
        assert_eq!(snapshot.active_task_effects[&task], vec![second]);
        assert!(
            runtime.complete_task_effect(task, &first).is_err(),
            "duplicate completion is rejected"
        );
        let mut restored =
            StoryRuntime::restore(bytecode, snapshot).expect("restore skips transient voice");
        assert!(matches!(
            restored.step().expect("join finishes"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
    }

    #[test]
    fn restore_preserves_sequence_dialogue_deferred_by_voice() {
        let bytecode = compile_story_bytecode(
            "test.hks",
            r#"
            let group = seq { voice("voice/alice") "after voice" }
            group.await()
        "#,
        )
        .expect("sequence compiles");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime initializes");
        assert!(matches!(
            runtime.step().expect("voice starts"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { .. },
                ..
            })
        ));
        assert_eq!(runtime.step().expect("dialogue is deferred"), None);
        let mut restored = StoryRuntime::restore(bytecode, runtime.snapshot().expect("snapshot"))
            .expect("restore");
        let Some(StoryRuntimeEvent::TaskEffect {
            task,
            effect: StoryEffect::Say { text, .. },
        }) = restored.step().expect("deferred dialogue")
        else {
            panic!("restore must emit deferred dialogue without replaying voice");
        };
        assert_eq!(text, "after voice");
        assert_eq!(restored.step().expect("wait for reveal"), None);
        restored.resume_task(task).expect("reveal completes");
        assert!(matches!(
            restored.step().expect("join finishes"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
    }

    #[test]
    fn script_handoff_reuses_session_globals_without_running_initializers() {
        let source = r#"
            fn initialScore() -> Int { log("initialized") 1 }
            global var score: Int = initialScore()
            score += 1
        "#;
        let bytecode = compile_story_bytecode("chapter.hks", source).expect("script compiles");
        let mut first = StoryRuntime::new(bytecode.clone()).expect("first entry");
        assert!(matches!(
            first.step().expect("initialize"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Log(_)))
        ));
        assert!(matches!(
            first.step().expect("complete"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
        assert_eq!(first.globals().get("score"), Some(&Value::Number(2.0)));
        let mut next = StoryRuntime::new(bytecode.clone()).expect("next entry");
        next.inherit_native_state(&first, false);
        next.set_globals(first.globals().clone());
        assert!(matches!(
            next.step().expect("skip initializer"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
        assert_eq!(next.globals().get("score"), Some(&Value::Number(3.0)));
        let mut reset = StoryRuntime::new(bytecode).expect("new session");
        assert!(matches!(
            reset.step().expect("initialize again"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Log(_)))
        ));
    }

    #[test]
    fn terminal_navigation_cancels_the_source_including_child_executions() {
        let bytecode = compile_story_bytecode(
            "entry.hks",
            r#"
            let handle = par { story.goto("next.hks") log("unreachable child") }
            wait(handle)
            log("unreachable parent")
        "#,
        )
        .expect("terminal child call compiles");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime initializes");
        assert!(matches!(
            runtime.step().expect("child jumps"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(_)))
        ));
        assert_eq!(runtime.step().expect("source stops"), None);
        assert!(matches!(
            runtime.resume(Value::Unit),
            Err(StoryRuntimeError::Terminated)
        ));
        let mut restored = StoryRuntime::restore(bytecode, runtime.snapshot().expect("snapshot"))
            .expect("restore");
        assert_eq!(
            restored.step().expect("restored source remains stopped"),
            None
        );
    }
    use hiraku_script::StatementValue;

    #[test]
    fn whole_program_runtime_yields_native_calls_without_ir() {
        let bytecode = compile_story_bytecode("test.story.hks", "log(\"hello\")")
            .expect("whole HKS story must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("script runtime must initialize");
        let Some(ExecutionEvent::Call { execution, call }) =
            runtime.step().expect("runtime must advance")
        else {
            panic!("expected a native call")
        };
        assert_eq!(execution, ExecutionId::MAIN);
        assert_eq!(
            call.builtin,
            story_manifest().resolve("log").expect("log registration")
        );
        runtime
            .resume(ExecutionId::MAIN, Value::Unit)
            .expect("host result must resume the main VM");
    }

    #[test]
    fn story_panic_is_formatted_with_its_source_location() {
        let source = "\npanic(\"alice failed\")";
        let bytecode = compile_story_bytecode("entry.hks", source).expect("panic compiles");
        let mut runtime = StoryRuntime::new(bytecode).expect("runtime starts");
        let report = runtime
            .step()
            .expect_err("panic reaches the host")
            .to_string();
        assert!(report.contains("HKS-PANIC"));
        assert!(report.contains("entry.hks:2:"));
        assert!(report.contains("alice failed"));
        assert!(!report.contains("RuntimeSource"));
    }

    #[test]
    fn choice_enable_is_evaluated_once_and_restored() {
        let source = r#"
            global var affection: Int = 1
            choice {
                option("Locked") { panic("disabled branch executed") }.enable(affection > 2)
                option("Available") { log("bob") }.enable(false).enable(true)
                affection = 10
            }
        "#;
        let bytecode =
            compile_story_bytecode("choice.hks", source).expect("fluent option compiles");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime starts");
        let event = runtime.step().expect("choice is built");
        assert!(
            matches!(event, Some(StoryRuntimeEvent::Choice { enabled, .. }) if enabled == [false, true])
        );
        let snapshot = runtime.snapshot().expect("choice is saveable");
        let mut runtime = StoryRuntime::restore(bytecode, snapshot).expect("choice restores");
        assert!(
            matches!(runtime.restored_boundary_event(), Some(StoryRuntimeEvent::Choice { enabled, .. }) if enabled == [false, true])
        );
        assert!(!runtime.accepts_choice_response(&Value::Number(0.0)));
        assert!(!runtime.accepts_choice_response(&Value::Number(0.5)));
        runtime
            .resume(Value::Number(0.0))
            .expect("disabled selection is ignored");
        assert!(runtime.step().expect("still waiting").is_none());
        runtime
            .resume(Value::Number(1.0))
            .expect("enabled branch starts");
        assert!(
            matches!(runtime.step().expect("branch executes"), Some(StoryRuntimeEvent::Effect(StoryEffect::Log(message))) if message == "bob")
        );
    }

    #[test]
    fn child_closures_use_the_same_execution_event_protocol() {
        let bytecode = compile_story_bytecode("execution.hks", "par { log(\"child\") }")
            .expect("task story must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("runtime must initialize");
        let mut host = StoryNativeHost::new();
        let Some(ExecutionEvent::Call {
            execution: ExecutionId::MAIN,
            call,
        }) = runtime.step().expect("root execution must advance")
        else {
            panic!("expected the root par call")
        };
        let StoryCallOutcome::Control(StoryControl::SpawnTask { kind, closure }) =
            host.call(&call).expect("par must create a child execution")
        else {
            panic!("expected a task spawn control")
        };
        assert_eq!(kind, StoryTaskKind::Parallel);
        let child = runtime
            .spawn(&closure, ExecutionMode::Parallel)
            .expect("child execution must spawn");
        runtime
            .resume(ExecutionId::MAIN, Value::Task(child.task_handle()))
            .expect("task handle must resume the root execution");

        let Some(ExecutionEvent::Call { execution, .. }) = runtime
            .step_children()
            .expect("child execution must advance")
        else {
            panic!("expected the child log call")
        };
        assert_eq!(execution, child);
        assert!(!execution.is_main());
    }

    #[test]
    fn raw_ui_result_requires_and_obeys_a_concrete_cast() {
        let source = "global var answer: Int = 0\nlet result = ui.open_any(\"form\") as! .{ a: Int }\nanswer = result.a";
        for value in [Value::Number(1.0), Value::String("alice".into())] {
            let code =
                compile_story_bytecode("memory://result.hks", source).expect("raw API compiles");
            let mut runtime = StoryRuntime::new(code).expect("runtime");
            assert!(matches!(
                runtime.step().expect("open UI"),
                Some(StoryRuntimeEvent::OpenUi { .. })
            ));
            runtime
                .resume(Value::Map(BTreeMap::from([("a".into(), value.clone())])))
                .expect("host result accepted");
            if matches!(value, Value::String(_)) {
                assert!(
                    runtime.step().is_err(),
                    "invalid data must fail the explicit cast"
                );
            } else {
                while runtime.step().expect("valid result").is_some() {}
                assert_eq!(runtime.globals().get("answer"), Some(&Value::Number(1.0)));
            }
        }
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
        let mut runtime = ExecutionRuntime::new(bytecode.clone()).expect("runtime must initialize");
        assert!(matches!(
            runtime.step().expect("runtime must advance"),
            Some(ExecutionEvent::Call { .. })
        ));
        let snapshot = runtime.snapshot();
        let mut restored =
            ExecutionRuntime::restore(bytecode, snapshot).expect("snapshot must restore");
        restored
            .resume(ExecutionId::MAIN, Value::Unit)
            .expect("restored host call must resume");
        assert!(matches!(
            restored.step().expect("runtime must reach statement"),
            Some(ExecutionEvent::Statement {
                value: StatementValue::Commit,
                ..
            })
        ));
        assert!(matches!(
            restored.step().expect("runtime must reach string hook"),
            Some(ExecutionEvent::Statement {
                value: StatementValue::String(text),
                ..
            }) if text == "after"
        ));
    }

    #[test]
    fn called_file_preserves_global_actor_identity() {
        let mut caller = StoryRuntime::new(
            compile_story_bytecode(
                "memory://caller.hks",
                "global let alice = char(\"alice\")\nglobal let bob = char(\"bob\")",
            )
            .expect("caller compiles"),
        )
        .expect("caller starts");
        assert!(matches!(
            caller.step().expect("declarations execute"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
        let bob = caller
            .globals()
            .get("bob")
            .expect("global identity exists")
            .clone();
        let mut callee = StoryRuntime::new(
            compile_story_bytecode(
                "memory://callee.hks",
                "global let bob = char(\"bob\")\nbob: \"Hello\"",
            )
            .expect("callee compiles"),
        )
        .expect("callee starts");
        callee.inherit_native_state(&caller, false);
        callee.set_globals(caller.globals().clone());
        assert_eq!(
            callee.step().expect("dialogue executes"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say {
                speaker: "bob".into(),
                text: "Hello".into()
            },))
        );
        assert_eq!(callee.globals().get("bob"), Some(&bob));
    }

    #[test]
    fn whole_program_runtime_evaluates_dialogue_templates_from_globals() {
        let bytecode = compile_story_bytecode(
            "template.story.hks",
            "global var player = .{ name: \"alice\" }\n\"Hi, ${player.name}\"",
        )
        .expect("template story must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("runtime must initialize");
        assert!(matches!(
            runtime.step().expect("global declaration must run"),
            Some(ExecutionEvent::Statement {
                value: StatementValue::Commit,
                ..
            })
        ));
        assert!(matches!(
            runtime.step().expect("dialogue statement must run"),
            Some(ExecutionEvent::Statement {
                value: StatementValue::String(text),
                ..
            }) if text == "Hi, alice"
        ));
    }

    #[test]
    fn direct_runtime_dispatches_native_calls_at_statement_boundaries() {
        let bytecode = compile_story_bytecode(
            "test.story.hks",
            r#"char("alice").e("happy").at(.right).show()"#,
        )
        .expect("character story must compile");
        let mut runtime = ExecutionRuntime::new(bytecode).expect("script runtime must initialize");
        let mut host = StoryNativeHost::new();

        loop {
            match runtime.step().expect("runtime must advance") {
                Some(ExecutionEvent::Call { call, .. }) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
                    runtime
                        .resume(ExecutionId::MAIN, value)
                        .expect("native result must resume the VM");
                }
                Some(ExecutionEvent::Statement {
                    value: StatementValue::Commit,
                    ..
                }) => {
                    host.commit_statement()
                        .expect("statement commit must flush actor state");
                }
                Some(ExecutionEvent::Statement { value, .. }) => {
                    panic!("unexpected statement boundary: {value:?}")
                }
                Some(ExecutionEvent::Completed { .. }) => break,
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
                char("alice").focus().show()
                char("bob").focus(false).show()
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
        let mut runtime = ExecutionRuntime::new(bytecode).expect("script runtime must initialize");
        let mut host = StoryNativeHost::new();

        loop {
            match runtime.step().expect("runtime must advance") {
                Some(ExecutionEvent::Call { call, .. }) => {
                    let value = host
                        .call(&call)
                        .expect("native call must succeed")
                        .into_return_value()
                        .expect("ordinary native call must return a value");
                    runtime
                        .resume(ExecutionId::MAIN, value)
                        .expect("native result must resume the VM");
                }
                Some(ExecutionEvent::Statement { value, .. }) => host
                    .handle_statement(&value)
                    .expect("statement commit must succeed"),
                Some(ExecutionEvent::Completed { .. }) => break,
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
                global var player = .{ name: "alice" }
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
    fn collection_keeps_choice_captures_after_the_builder_completes() {
        let bytecode = compile_story_bytecode(
            "choice_gc.hks",
            r#"
            choice("Select") {
                let alice = .{ name: "alice" }
                option("A") { "${alice.name}" }
                var index = 0
                while index < 1100 {
                    let unused = .{ score: index }
                    index += 1
                }
            }
        "#,
        )
        .expect("choice compiles");
        let mut runtime = StoryRuntime::new(bytecode).expect("runtime initializes");
        let mut reached_choice = false;
        for _ in 0..100 {
            if matches!(
                runtime.step().expect("builder executes"),
                Some(StoryRuntimeEvent::Choice { .. })
            ) {
                reached_choice = true;
                break;
            }
        }
        assert!(reached_choice);
        assert_eq!(runtime.step().expect("blocked choice can collect"), None);
        runtime
            .resume(Value::Number(0.0))
            .expect("selection resumes");
        assert!(
            matches!(runtime.step().expect("captured object survives collection"), Some(StoryRuntimeEvent::Effect(StoryEffect::Say { text, .. })) if text == "alice")
        );
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
                enabled: vec![true, true],
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
                enabled: vec![true, true],
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
        assert!(runtime.blocks_ui_input());
        let mut restored =
            StoryRuntime::restore(bytecode, snapshot).expect("movie wait must restore");
        assert!(restored.blocks_ui_input());
        assert_eq!(
            restored.restored_boundary_event(),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "movies/opening.mkv".into(),
            }))
        );
        restored.resume(Value::Unit).expect("movie completion");
        assert!(!restored.blocks_ui_input());
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
        assert_eq!(
            runtime
                .step()
                .expect("line must wait for the preceding voice"),
            None
        );
        runtime
            .resume_task(first)
            .expect("first audio completion must resume the task");
        assert!(
            matches!(runtime.step().expect("line follows voice completion"),
            Some(StoryRuntimeEvent::TaskEffect { effect: StoryEffect::Say { ref text, .. }, .. }) if text == "first line")
        );
        assert_eq!(
            runtime
                .step()
                .expect("line reveal must complete before next voice"),
            None
        );
        runtime
            .resume_task(first)
            .expect("line reveal completes automatically");
        assert!(matches!(
            runtime.step().expect("second voice must follow completion"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { ref path, .. },
                ..
            }) if path == "voice/second"
        ));
    }

    #[test]
    fn parallel_dialogue_is_skipped_but_other_effects_and_join_still_run() {
        let code = compile_story_bytecode(
            "parallel.hks",
            r#"
            let group = par {
                "not shown"
                narrate("also not shown")
                char("alice"): "not shown either"
                ...: "not appended"
                voice("voice/alice")
            }
            group.await()
            group.await()
            "after group"
        "#,
        )
        .expect("parallel story compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime starts");
        let task = match runtime.step().expect("parallel voice starts") {
            Some(StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::PlayVoice { .. },
            }) => task,
            other => panic!("dialogue must not be emitted: {other:?}"),
        };
        assert!(runtime.step().expect("group waits for voice").is_none());
        runtime.resume_task(task).expect("voice completes");
        assert!(matches!(runtime.step().expect("both joins complete"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { ref text, .. })) if text == "after group"));
    }

    #[test]
    fn sequence_waits_for_camera_and_voice_before_emitting_dialogue() {
        let code = compile_story_bytecode(
            "sequence.hks",
            r#"
            let group = seq {
                camera().zoom(1.2).time(1)
                voice("voice/alice")
                "after effects"
            }
            group.await()
        "#,
        )
        .expect("sequence story compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime starts");
        let task = match runtime.step().expect("camera starts") {
            Some(StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::SetCamera { .. },
            }) => task,
            other => panic!("expected tracked camera: {other:?}"),
        };
        assert!(runtime.step().expect("voice waits for camera").is_none());
        runtime.resume_task(task).expect("camera completes");
        assert!(matches!(
            runtime.step().expect("voice starts after camera"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { .. },
                ..
            })
        ));
        assert!(runtime.step().expect("dialogue waits").is_none());
        runtime.resume_task(task).expect("voice completes");
        assert!(matches!(
            runtime.step().expect("dialogue now appears"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::Say { .. },
                ..
            })
        ));
        assert!(
            runtime
                .step()
                .expect("group includes dialogue reveal")
                .is_none()
        );
        runtime
            .resume_task(task)
            .expect("dialogue reveal completes");
        assert!(matches!(
            runtime.step().expect("group finishes"),
            Some(StoryRuntimeEvent::Completed(_))
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
                voices.await()
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
                    var count = 0
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
