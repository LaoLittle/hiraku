//! Engine-facing story policy built on the generic execution runtime.
//!
//! It owns story capabilities and wait policy while ECS systems own effects.

use std::collections::{BTreeMap, VecDeque};

use super::animation_plan::{AnimationPlan, PlanMode};
use hiraku_script::Value;
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
    pub(crate) preload_calls: bool,
    plans: BTreeMap<ExecutionId, AnimationPlan>,
    execution: ExecutionRuntime,
    host: StoryNativeHost,
    pending: VecDeque<StoryRuntimeEvent>,
    active_task_effects: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<ExecutionId, Value>,
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
    parameters: Option<crate::state::StoredValue>,
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
    RandomInt {
        min: i64,
        max: i64,
    },
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
        parameters: Vec<Option<crate::state::StoredValue>>,
    },
    TaskEffect {
        task: ExecutionId,
        effect: StoryEffect,
    },
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoryRuntimeSnapshot {
    preload_calls: bool,
    plans: BTreeMap<ExecutionId, AnimationPlan>,
    awaiting_effects: std::collections::BTreeSet<ExecutionId>,
    execution: ExecutionRuntimeSnapshot,
    host: StoryNativeHostSnapshot,
    active_task_effects: BTreeMap<ExecutionId, Vec<StoryEffect>>,
    deferred_task_completions: BTreeMap<ExecutionId, Value>,
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
    fn build_plan(
        &mut self,
        kind: StoryTaskKind,
        closure: &Value,
    ) -> Result<ExecutionId, StoryRuntimeError> {
        let mode = match kind {
            StoryTaskKind::Sequence => PlanMode::Sequence,
            StoryTaskKind::Parallel => PlanMode::Parallel,
        };
        let task = self.execution.spawn(closure, ExecutionMode::Interactive)?;
        let mut plan = AnimationPlan::new(mode);
        let result = (|| {
            let mut budget = 1_000_000;
            loop {
                let event = self
                    .execution
                    .step_execution(task, &mut budget)?
                    .ok_or(StoryRuntimeError::PlanBuildBudgetExceeded)?;
                match event {
                    ExecutionEvent::Call { call, .. } => match self.host.call(&call)? {
                        StoryCallOutcome::Return(value) => self.execution.resume(task, value)?,
                        StoryCallOutcome::Control(StoryControl::Navigate(request)) => {
                            plan.record(vec![StoryEffect::Navigate(request)], true);
                            self.execution.cancel_child(task);
                            break;
                        }
                        StoryCallOutcome::Control(control) => {
                            return Err(StoryRuntimeError::UnsupportedTaskControl(control));
                        }
                    },
                    ExecutionEvent::Statement { value, .. } => {
                        self.host.handle_statement(&value)?;
                        let barrier = self.host.take_animation_await();
                        if let Some(wait @ StoryWait::Movie { .. }) = self.host.take_wait() {
                            return Err(StoryRuntimeError::UnsupportedTaskWait(wait));
                        }
                        let effects = self
                            .host
                            .drain_effects()
                            .into_iter()
                            .filter(|effect| {
                                if mode == PlanMode::Parallel
                                    && matches!(
                                        effect,
                                        StoryEffect::Say { .. }
                                            | StoryEffect::ContinueDialogue { .. }
                                    )
                                {
                                    bevy::log::warn!(
                                        "say/narrate are not allowed in par; statement skipped"
                                    );
                                    false
                                } else {
                                    true
                                }
                            })
                            .collect();
                        plan.record(effects, barrier);
                    }
                    ExecutionEvent::Completed { .. } => break,
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.execution.cancel_child(task);
            return Err(error);
        }
        self.plans.insert(task, plan);
        self.advance_plan(task)?;
        Ok(task)
    }

    fn advance_plan(&mut self, task: ExecutionId) -> Result<(), StoryRuntimeError> {
        loop {
            let batch = self.plans.get_mut(&task).and_then(AnimationPlan::next);
            let Some(batch) = batch else {
                self.plans.remove(&task);
                self.completed_groups.insert(task);
                if self.waiting_task == Some(task) {
                    self.waiting_task = None;
                    self.execution.resume(ExecutionId::MAIN, Value::Unit)?;
                }
                return Ok(());
            };
            for effect in batch {
                if matches!(effect, StoryEffect::Navigate(_)) {
                    self.terminated = true;
                    self.plans.clear();
                    self.active_task_effects.clear();
                    self.pending.push_back(StoryRuntimeEvent::Effect(effect));
                    return Ok(());
                }
                if animation_effect(&effect)
                    || matches!(
                        effect,
                        StoryEffect::Say { .. } | StoryEffect::ContinueDialogue { .. }
                    )
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
            if self.active_task_effects.contains_key(&task) {
                return Ok(());
            }
        }
    }

    pub(crate) fn has_executions(&self) -> bool {
        self.execution.has_executions() || !self.plans.is_empty()
    }
    pub(crate) fn resource_positions(&self) -> Vec<(String, usize)> {
        self.execution.resource_positions()
    }
    pub fn program_for_path(&self, path: &str) -> Option<super::StoryProgram> {
        self.execution.program_for_path(path)
    }

    pub fn new(bytecode: impl Into<super::StoryProgram>) -> Result<Self, StoryRuntimeError> {
        Ok(Self {
            preload_calls: true,
            plans: BTreeMap::new(),
            execution: ExecutionRuntime::new(bytecode)?,
            host: StoryNativeHost::new(),
            pending: VecDeque::new(),
            active_task_effects: BTreeMap::new(),
            deferred_task_completions: BTreeMap::new(),
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
            preload_calls: self.preload_calls,
            plans: self.plans.clone(),
            awaiting_effects: self.awaiting_effects.clone(),
            execution: self.execution.snapshot(),
            host: self.host.snapshot(),
            active_task_effects: self.active_task_effects.clone(),
            deferred_task_completions: self.deferred_task_completions.clone(),
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
        bytecode: impl Into<super::StoryProgram>,
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
                    StoryEffect::PlayVoice { .. }
                        | StoryEffect::PlaySfx { .. }
                        | StoryEffect::PlaySfxChannel { .. }
                )
            });
            effects.retain(|effect| {
                !matches!(
                    effect,
                    StoryEffect::PlayVoice { .. }
                        | StoryEffect::PlaySfx { .. }
                        | StoryEffect::PlaySfxChannel { .. }
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
            preload_calls: snapshot.preload_calls,
            plans: snapshot.plans,
            awaiting_effects: snapshot.awaiting_effects,
            execution: ExecutionRuntime::restore(bytecode, snapshot.execution)?,
            host: StoryNativeHost::restore(snapshot.host),
            pending,
            active_task_effects: snapshot.active_task_effects,
            deferred_task_completions: snapshot.deferred_task_completions,
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
        if self.plans.contains_key(&task) {
            return super::VoicePlaybackMode::Concurrent;
        }
        super::VoicePlaybackMode::Exclusive
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
                    parameters: options
                        .iter()
                        .map(|option| option.parameters.clone())
                        .collect(),
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
        let Value::Int(index) = value else {
            return false;
        };
        *index >= 0
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
            let Value::Int(selected) = value else {
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
        // Hiding/replacing an actor cancels its old animation continuation,
        // not just the current ECS tween. Never let the next offset target a
        // hidden actor or a newly shown incarnation of the same display.
        if let Some(plan) = self.plans.get(&task)
            && plan.mode == PlanMode::Sequence
            && let StoryEffect::ActorMotion {
                actor_id, revision, ..
            } = completed
            && !self.host.actor_motion_is_current(
                actor_id,
                plan.motion_revision(actor_id).unwrap_or(*revision),
            )
        {
            self.execution.cancel_child(task);
            self.plans.remove(&task);
            self.deferred_task_completions.insert(task, Value::Unit);
        }
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
        if self.plans.contains_key(&task) {
            return Ok(());
        }
        if self.awaiting_effects.remove(&task) {
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
        let ready = self
            .plans
            .keys()
            .copied()
            .filter(|task| !self.active_task_effects.contains_key(task))
            .collect::<Vec<_>>();
        for task in ready {
            self.advance_plan(task)?;
        }
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
                            let task = self.build_plan(kind, &closure)?;
                            self.execution
                                .resume(ExecutionId::MAIN, Value::Task(task.task_handle()))?;
                            if let Some(event) = self.pending.pop_front() {
                                self.mark_host_boundary(&event);
                                return Ok(Some(event));
                            }
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
                        StoryCallOutcome::Control(StoryControl::RandomInt { min, max }) => {
                            self.blocked = true;
                            return Ok(Some(StoryRuntimeEvent::RandomInt { min, max }));
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
                            | StoryControl::SetChoiceParameters { .. }
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
                | StoryRuntimeEvent::RandomInt { .. }
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
                StoryCallOutcome::Control(StoryControl::Navigate(mut request)) => {
                    if matches!(self.choice, Some(ChoiceState::RunningBranch { task: branch, .. }) if branch == task)
                    {
                        request.preload.get_or_insert(false);
                    }
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
                        parameters: None,
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
                StoryCallOutcome::Control(StoryControl::SetChoiceParameters { id, value }) => {
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
                    options
                        .get_mut((id & u32::MAX as u64) as usize)
                        .ok_or(StoryRuntimeError::InvalidChoice)?
                        .parameters = Some(value);
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
                    let parameters = options
                        .iter()
                        .map(|option| option.parameters.clone())
                        .collect();
                    self.choice = Some(ChoiceState::AwaitingSelection {
                        prompt: prompt.clone(),
                        options,
                    });
                    self.blocked = true;
                    return Ok(Some(StoryRuntimeEvent::Choice {
                        parameters,
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
                        .resume(ExecutionId::MAIN, Value::Int(selected as i64))?;
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
                    | StoryEffect::PlaySfxChannel { .. }
                    | StoryEffect::Delay { .. }
            ) || ((explicit || task_mode != Some(ExecutionMode::Interactive))
                && animation_effect(&effect))
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
        if explicit && self.active_task_effects.contains_key(&task) {
            self.awaiting_effects.insert(task);
            self.execution.pause(task)?;
        }
        Ok(())
    }
}

fn animation_effect(effect: &StoryEffect) -> bool {
    matches!(
        effect,
        StoryEffect::Spatial(_)
            | StoryEffect::ShakeCamera { .. }
            | StoryEffect::SetCamera { .. }
            | StoryEffect::SetBackground { .. }
            | StoryEffect::ShowCharacter { .. }
            | StoryEffect::HideCharacter { .. }
            | StoryEffect::ActorMotion { .. }
            | StoryEffect::SetCurtain { .. }
            | StoryEffect::Picture(_)
            | StoryEffect::PlayBgm { .. }
            | StoryEffect::StopBgm { .. }
            | StoryEffect::PlayVoice { .. }
            | StoryEffect::Delay { .. }
            | StoryEffect::PlaySfx { .. }
            | StoryEffect::PlaySfxChannel { .. }
    )
}

#[derive(Debug, Error)]
pub enum StoryRuntimeError {
    #[error(
        "seq/par construction exceeded its instruction budget; these closures must finish before playback"
    )]
    PlanBuildBudgetExceeded,
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
    fn plans_finish_script_evaluation_before_first_effect_and_restore_without_closure_frames() {
        let code = compile_story_bytecode(
            "plan.hks",
            r#"
            global var built = 0
            let plan = seq {
                built += 1
                sleep(0.1)
                built += 1
                sleep(0.2)
            }
            plan.await()
            "Done"
        "#,
        )
        .expect("compile recorded sequence");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) =
            runtime.step().expect("first effect")
        else {
            panic!("first delay");
        };
        assert_eq!(runtime.globals().get("built"), Some(&Value::Int(2)));
        assert!(
            runtime.execution.mode(task).is_none(),
            "finished closure is not retained during playback"
        );
        assert!(runtime.step().expect("wait for first delay").is_none());
        let bytes =
            hiraku_script::hson::to_vec(&runtime.snapshot().expect("snapshot")).expect("serialize");
        runtime = StoryRuntime::restore(
            code,
            hiraku_script::hson::from_slice(&bytes).expect("decode"),
        )
        .expect("restore plan");
        assert!(matches!(
            runtime.step().expect("reattach first effect"),
            Some(StoryRuntimeEvent::TaskEffect { .. })
        ));
        runtime
            .complete_task_effect(task, &effect)
            .expect("first completes");
        assert!(matches!(
            runtime.step().expect("next recorded step"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::Delay { duration_ms: 200 },
                ..
            })
        ));
        assert_eq!(
            runtime.globals().get("built"),
            Some(&Value::Int(2)),
            "restoring playback must not repeat build-time assignments"
        );
    }

    #[test]
    fn masked_portrait_and_picture_wipe_use_regular_story_capabilities() {
        compile_story_bytecode("memory://portrait.hks", r#"
            scene.clipRect("letterbox", 1920, 705).at(.pos(0, 0))
            let entrance = par {
                scene.picture("frame", "ui/frame").at(.right).size(583.5, 715.5).layer(12).fade(400)
                char("alice").e("portrait").at(.right).scale(1.5).depth(12.5).clip(null).show().time(1).easing(.linear)
            }
            par {
                scene.transformPicture("frame").at(.rel(82, 64)).time(0.4).easing(.smoothStep)
                char("alice").at(.rel(82, 64)).time(0.4).easing(.smoothStep)
            }
            entrance.await()
            scene.curtain(1).color(255, 255, 255).dissolve("wipe", 0.25).fade(100).await()
            scene.hidePicture("frame").fade(0)
            scene.clipPicture("frame", null)
        "#).expect("masked portrait and wipe compile without game-specific builtins");
    }

    #[test]
    fn random_draw_is_a_host_boundary_and_does_not_reexecute_after_resume() {
        let code = compile_story_bytecode(
            "memory://random.hks",
            "global let variant = randomInt(2, 4)\nlog(variant.toString())",
        )
        .expect("random signature compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime initializes");
        let event = loop {
            if let Some(event) = runtime.step().expect("advance") {
                break event;
            }
        };
        assert_eq!(event, StoryRuntimeEvent::RandomInt { min: 2, max: 4 });
        runtime.resume(Value::Int(3)).expect("host response");
        for _ in 0..20 {
            if let Some(event) = runtime.step().expect("continue") {
                assert!(!matches!(event, StoryRuntimeEvent::RandomInt { .. }));
                if matches!(event, StoryRuntimeEvent::Completed(_)) {
                    return;
                }
            }
        }
        panic!("story did not complete");
    }

    #[test]
    fn modal_visits_survive_restore_before_selection_and_after_each_visit() {
        let code = compile_story_bytecode(
            "memory://visits.hks",
            r#"
            global var north = false
            global var south = false
            global var east = false
            global var west = false
            global var count = 0
            while count < 4 {
                let room = ui.open("map.ui.hks", north, south, east, west) as! String
                if room == "north" { if north { panic("duplicate") }; north = true }
                if room == "south" { if south { panic("duplicate") }; south = true }
                if room == "east" { if east { panic("duplicate") }; east = true }
                if room == "west" { if west { panic("duplicate") }; west = true }
                count += 1
                "Visited"
            }
            log("complete")
        "#,
        )
        .expect("typed visit loop compiles");
        let names = ["north", "south", "east", "west"];
        for a in 0..4 {
            for b in 0..4 {
                for c in 0..4 {
                    for d in 0..4 {
                        let order = [a, b, c, d];
                        if order
                            .iter()
                            .copied()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            != 4
                        {
                            continue;
                        }
                        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
                        let mut visited = [false; 4];
                        for room in order {
                            let event = runtime.step().expect("next map");
                            assert!(
                                matches!(event, Some(StoryRuntimeEvent::OpenUi { arguments, .. })
                                if arguments == visited.map(Value::Bool).to_vec())
                            );
                            runtime = StoryRuntime::restore(
                                code.clone(),
                                runtime.snapshot().expect("map boundary"),
                            )
                            .expect("restore pending map");
                            runtime
                                .resume(Value::String(names[room].into()))
                                .expect("select room");
                            visited[room] = true;
                            let mut waiting = false;
                            for _ in 0..16 {
                                if matches!(
                                    runtime.step().expect("visit dialogue"),
                                    Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
                                ) {
                                    waiting = true;
                                    break;
                                }
                            }
                            assert!(waiting, "visit must reach dialogue");
                            runtime = StoryRuntime::restore(
                                code.clone(),
                                runtime.snapshot().expect("dialogue boundary"),
                            )
                            .expect("restore visited room");
                            runtime.resume(Value::Unit).expect("leave visited room");
                        }
                        assert!(
                            matches!(runtime.step().expect("all rooms visited"), Some(StoryRuntimeEvent::Effect(StoryEffect::Log(s))) if s == "complete")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn eight_room_visits_restore_in_both_directions_at_each_boundary() {
        let names = (0..8).map(|i| format!("visited{i}")).collect::<Vec<_>>();
        let declarations = names
            .iter()
            .map(|name| format!("global var {name} = false\n"))
            .collect::<String>();
        let branches = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                format!(
                    "if room == {i} {{ if {name} {{ panic(\"duplicate\") }}; {name} = true }}\n"
                )
            })
            .collect::<String>();
        let code = compile_story_bytecode(
            "memory://eight-rooms.hks",
            &format!(
                r#"
            {declarations}
            global var count = 0
            while count < 8 {{
                let room = ui.open("map.ui.hks", {}) as! Int
                if room < 0 || room >= 8 {{ panic("unknown room") }}
                {branches}
                count += 1
                "Visited"
            }}
            log("complete")
        "#,
                names.join(", ")
            ),
        )
        .expect("eight-room script compiles");
        for start in 0..8 {
            for reverse in [false, true] {
                let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
                let mut visited = [false; 8];
                for step in 0..8 {
                    let room = (start + if reverse { 8 - step } else { step }) % 8;
                    assert!(
                        matches!(runtime.step().expect("map"), Some(StoryRuntimeEvent::OpenUi { arguments, .. })
                        if arguments == visited.map(Value::Bool).to_vec())
                    );
                    runtime = StoryRuntime::restore(
                        code.clone(),
                        runtime.snapshot().expect("map snapshot"),
                    )
                    .expect("restore map");
                    runtime
                        .resume(Value::Int(room as i64))
                        .expect("choose room");
                    visited[room] = true;
                    let mut waiting = false;
                    for _ in 0..16 {
                        if matches!(
                            runtime.step().expect("room dialogue"),
                            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
                        ) {
                            waiting = true;
                            break;
                        }
                    }
                    assert!(waiting);
                    runtime = StoryRuntime::restore(
                        code.clone(),
                        runtime.snapshot().expect("room snapshot"),
                    )
                    .expect("restore room");
                    runtime.resume(Value::Unit).expect("return to map");
                }
                assert!(
                    matches!(runtime.step().expect("complete"), Some(StoryRuntimeEvent::Effect(StoryEffect::Log(message))) if message == "complete")
                );
            }
        }
    }

    #[test]
    fn modal_result_loop_reopens_after_back_and_can_navigate_without_falling_through() {
        let bytecode = compile_story_bytecode(
            "menu.hks",
            r#"
            while true {
                let destination = ui.open("menu.ui.hks") as! String
                if destination == "load" {
                    ui.open("load.ui.hks")
                } else {
                    story.goto("title.hks", .{ reset: .presentation })
                }
            }
        "#,
        )
        .expect("typed modal loop compiles");
        let mut runtime = StoryRuntime::new(bytecode).expect("runtime");
        fn next_modal(runtime: &mut StoryRuntime) -> String {
            for _ in 0..32 {
                if let Some(StoryRuntimeEvent::OpenUi { path, .. }) =
                    runtime.step().expect("modal loop advances")
                {
                    return path;
                }
            }
            panic!("modal was not reached");
        }
        assert_eq!(next_modal(&mut runtime), "menu.ui.hks");
        runtime
            .resume(Value::String("load".into()))
            .expect("select load");
        assert_eq!(next_modal(&mut runtime), "load.ui.hks");
        runtime
            .resume(Value::Unit)
            .expect("close load without loading");
        assert_eq!(next_modal(&mut runtime), "menu.ui.hks");
        runtime
            .resume(Value::String("title".into()))
            .expect("select title");
        for _ in 0..32 {
            if matches!(
                runtime.step().expect("navigate"),
                Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(_)))
            ) {
                assert_eq!(runtime.step().expect("source terminated"), None);
                return;
            }
        }
        panic!("title navigation was not reached");
    }

    #[test]
    fn hidden_actor_sequence_commits_final_offsets_without_tween_or_deadlock() {
        for hide in ["alice.hide(0)", "scene.hideCharacters(0)"] {
            let source = format!(
                r#"
                let alice = char("alice").show()
                {hide}
                let jump = seq {{
                    alice.offset(.pos(0, 20)).time(1.0).easing(.linear)
                    alice.offset(.pos(0, 0)).time(1.0).easing(.linear)
                }}
                jump.await()
                "Done"
            "#
            );
            let code = compile_story_bytecode("hidden.hks", &source).expect("fixture");
            let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
            let mut targets = Vec::new();
            let mut reached_dialogue = false;
            for _ in 0..128 {
                match runtime.step().expect("hidden offsets are valid") {
                    Some(StoryRuntimeEvent::TaskEffect {
                        task,
                        effect: effect @ StoryEffect::ActorMotion { .. },
                    }) => {
                        let StoryEffect::ActorMotion { transition, .. } = &effect else {
                            unreachable!()
                        };
                        assert_eq!(transition.animation.duration(), 0.0);
                        targets.push(transition.target);
                        runtime = StoryRuntime::restore(
                            code.clone(),
                            runtime.snapshot().expect("snapshot"),
                        )
                        .expect("restore hidden sequence");
                        assert!(matches!(
                            runtime.step().expect("reattach"),
                            Some(StoryRuntimeEvent::TaskEffect { .. })
                        ));
                        runtime
                            .complete_task_effect(task, &effect)
                            .expect("immediate completion");
                    }
                    Some(StoryRuntimeEvent::Wait(_)) if targets.len() == 2 => {
                        reached_dialogue = true;
                        break;
                    }
                    _ => {}
                }
            }
            assert_eq!(targets, [[0.0, 20.0], [0.0, 0.0]]);
            assert!(reached_dialogue, "join must finish after hidden motions");
        }
    }

    #[test]
    fn script_actor_patches_retain_state_and_commit_once_per_public_statement() {
        let code = compile_story_bytecode(
            "actors.hks",
            r#"
            let alice = char("alice").e("normal").scale(0.5).show()
            char("alice").e("happy").rotation(12)
            alice.at(.pos(20, 30))
            alice
        "#,
        )
        .expect("script-owned Actor API");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let mut shows = Vec::new();
        for _ in 0..32 {
            match runtime.step().expect("commit patch") {
                Some(StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter {
                    actor_id,
                    expressions,
                    scale,
                    position,
                    rotation,
                    ..
                })) => shows.push((actor_id, expressions, scale, position, rotation)),
                Some(StoryRuntimeEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(
            shows.len(),
            3,
            "empty patches do not re-submit presentation"
        );
        assert!(shows.iter().all(|show| show.0 == "alice" && show.2 == 0.5));
        assert_eq!(shows[0].1, ["normal"]);
        assert_eq!(shows[1].1, ["normal", "happy"]);
        assert_eq!(shows[2].1, ["normal", "happy"]);
        assert_eq!(shows[2].3, [20.0, 30.0]);
        assert_eq!(shows[2].4, 12.0);
    }

    #[test]
    fn hiding_and_reshowing_cancels_old_offset_sequence_and_resolves_join() {
        for replacement in [
            "scene.hideCharacters(0)",
            "alice.hide(0)",
            "scene.hideCharacters(0)\nalice.show()",
            "alice.alias(\"middle\").show()",
            "alice.stopMotion()",
        ] {
            let bytecode = compile_story_bytecode(
                "cancel.hks",
                &r#"
            let alice = char("alice").show()
            let jump = seq {
                alice.offset(.pos(0, 20)).time(1.0).easing(.linear)
                alice.offset(.pos(0, 0)).time(1.0).easing(.linear)
            }
            "First"
            REPLACE_ACTOR
            jump.await()
            "Second"
        "#
                .replace("REPLACE_ACTOR", replacement),
            )
            .expect("fixture compiles");
            for restore in [false, true] {
                let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime");
                let mut motion = None;
                for _ in 0..32 {
                    if let Some(StoryRuntimeEvent::TaskEffect {
                        task,
                        effect: effect @ StoryEffect::ActorMotion { .. },
                    }) = runtime.step().expect("first dialogue and animation")
                    {
                        motion = Some((task, effect));
                        break;
                    }
                }
                let (task, effect) = motion.expect("sequence starts while dialogue is waiting");
                while !runtime.is_waiting_for_host_response() {
                    runtime
                        .step()
                        .expect("reach root dialogue after synchronous plan construction");
                }
                runtime.resume(Value::Unit).expect("advance dialogue early");
                for _ in 0..32 {
                    if runtime.step().expect("hide and re-show").is_none() {
                        break;
                    }
                }
                assert_eq!(runtime.waiting_task, Some(task));
                if restore {
                    runtime = StoryRuntime::restore(
                        bytecode.clone(),
                        runtime.snapshot().expect("snapshot"),
                    )
                    .expect("restore interrupted animation");
                    assert_eq!(
                        runtime.step().expect("rebuild pending ECS effect"),
                        Some(StoryRuntimeEvent::TaskEffect {
                            task,
                            effect: effect.clone()
                        })
                    );
                }
                runtime
                    .complete_task_effect(task, &effect)
                    .expect("cancelled tween completion");
                assert!(runtime.completed_groups.contains(&task));
                assert_eq!(runtime.waiting_task, None);
                let mut reached_dialogue = false;
                for _ in 0..32 {
                    let event = runtime.step().expect("continue after cancelled sequence");
                    assert!(
                        !matches!(
                            event,
                            Some(StoryRuntimeEvent::TaskEffect {
                                effect: StoryEffect::ActorMotion { .. },
                                ..
                            })
                        ),
                        "cancelled sequence must not emit another offset"
                    );
                    if matches!(event, Some(StoryRuntimeEvent::Wait(_))) {
                        reached_dialogue = true;
                        break;
                    }
                }
                assert!(
                    reached_dialogue,
                    "cancelled join must reach the next dialogue"
                );
            }
        }
    }

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
    fn picture_sequence_keeps_local_noise_and_order_across_restore() {
        let bytecode = compile_story_bytecode(
            "sequence.hks",
            r#"
            let group = seq {
                var seed: Float = 7.0
                var pulse = 0
                while pulse < 3 {
                    seed *= 48271.0
                    seed -= (seed / 2147483647.0).toInt().toFloat() * 2147483647.0
                    let offset = seed / 2147483647.0
                    scene.movePicture("panel", 50, 50 - offset, 0.1, "smoothStep")
                    scene.movePicture("panel", 50, 50 + offset, 0.2, "smoothStep")
                    scene.movePicture("panel", 50, 50, 0.1, "smoothStep")
                    pulse += 1
                }
            }
            group.await()
        "#,
        )
        .expect("sequence compiles");
        let mut runs = Vec::new();
        for restore in [false, true] {
            let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime");
            let mut positions = Vec::new();
            loop {
                match runtime.step().expect("step") {
                    Some(StoryRuntimeEvent::TaskEffect { task, effect }) => {
                        let StoryEffect::Picture(
                            crate::scene::pictures::PictureCommand::Transform { position, .. },
                        ) = &effect
                        else {
                            panic!("expected a picture move, got {effect:?}");
                        };
                        positions.push(
                            position.map(|value| value.expect("both movement axes are specified")),
                        );
                        assert_eq!(runtime.step().expect("waiting"), None);
                        runtime
                            .complete_task_effect(task, &effect)
                            .expect("animation finishes");
                        if restore && positions.len() == 4 {
                            runtime = StoryRuntime::restore(
                                bytecode.clone(),
                                runtime.snapshot().expect("snapshot"),
                            )
                            .expect("restore local state");
                        }
                    }
                    Some(StoryRuntimeEvent::Completed(_)) => break,
                    other => panic!("unexpected sequence event {other:?}"),
                }
            }
            assert_eq!(positions.len(), 9);
            assert_eq!(positions[8], [50.0, 50.0]);
            runs.push(positions);
        }
        assert_eq!(runs[0], runs[1]);
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
        assert_eq!(first.globals().get("score"), Some(&Value::Int(2)));
        let mut next = StoryRuntime::new(bytecode.clone()).expect("next entry");
        next.inherit_native_state(&first, false);
        next.set_globals(first.globals().clone());
        assert!(matches!(
            next.step().expect("skip initializer"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
        assert_eq!(next.globals().get("score"), Some(&Value::Int(3)));
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
    fn choice_parameters_are_captured_once_and_restored() {
        let source = r#"
            var danger = true
            choice {
                option("Alice") { log("alice") }.params(danger).enable(false)
                option("Bob") { log("bob") }.enable(true).params(false)
                danger = false
            }
        "#;
        let bytecode = compile_story_bytecode("choice.hks", source).expect("choice compiles");
        let mut runtime = StoryRuntime::new(bytecode.clone()).expect("runtime starts");
        let event = runtime.step().expect("choice suspends");
        assert!(
            matches!(&event, Some(StoryRuntimeEvent::Choice { parameters, enabled, .. })
            if parameters == &vec![Some(crate::state::StoredValue::Bool(true)), Some(crate::state::StoredValue::Bool(false))]
                && enabled == &[false, true])
        );
        let snapshot = runtime.snapshot().expect("snapshot captures choice data");
        let bytes = hiraku_script::hson::to_vec(&snapshot).expect("choice snapshot serializes");
        let snapshot = hiraku_script::hson::from_slice(&bytes).expect("choice snapshot decodes");
        let mut restored = StoryRuntime::restore(bytecode, snapshot).expect("snapshot restores");
        assert_eq!(restored.restored_boundary_event(), event);
        restored
            .resume(Value::Int(1))
            .expect("selection resumes branch");
        assert!(restored.step().is_ok());
    }

    #[test]
    fn choice_parameters_reject_live_callables() {
        let bytecode = compile_story_bytecode(
            "choice.hks",
            r#"
            choice { option("Alice") {}.params({ log("alice") }) }
        "#,
        )
        .expect("payload is validated at the UI transport boundary");
        let mut runtime = StoryRuntime::new(bytecode).expect("runtime starts");
        let error = runtime
            .step()
            .expect_err("callbacks cannot cross the plain-data boundary");
        assert!(
            error
                .to_string()
                .contains("UI arguments currently require plain"),
            "{error}"
        );
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
        assert!(!runtime.accepts_choice_response(&Value::Int(0)));
        assert!(!runtime.accepts_choice_response(&Value::Number(0.5)));
        runtime
            .resume(Value::Int(0))
            .expect("disabled selection is ignored");
        assert!(runtime.step().expect("still waiting").is_none());
        runtime
            .resume(Value::Int(1))
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
            .spawn(&closure, ExecutionMode::Interactive)
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
        for value in [Value::Int(1), Value::String("alice".into())] {
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
                assert_eq!(runtime.globals().get("answer"), Some(&Value::Int(1)));
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
                lifetime: None,
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
                arguments: vec![Value::String("Alice".to_string()), Value::Int(3)],
            })
        );
    }

    #[test]
    fn timed_ui_mount_emits_a_presentation_owned_lifetime() {
        let bytecode = compile_story_bytecode(
            "notification.hks",
            "ui.mountFor(\"notice\", \"ui/notice.ui.hks\", 3.0)",
        )
        .expect("timed mount must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("runtime must initialize");
        assert_eq!(
            runtime.step().expect("mount must run"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::MountUiOverlay {
                name: "notice".into(),
                component: "ui/notice.ui.hks".into(),
                lifetime: Some(3.0),
            }),)
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
                camera(.scene)
                    .offset(10, 20, 30)
                    .rotation(1, 2, 3)
                    .zoom(1.25)
                    .projection(.perspective)
                    .time(0.5)
                    .easing(.easeOut)
                camera(.canvas).offset(10, 20, 0).roll(3).zoom(1.25).time(0.5)
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
                scope: crate::script::CameraEffectScope::World,
                ..
            } if (*zoom - 1.25).abs() < f32::EPSILON && *ease == crate::script::animation::Easing::EaseOut
        )));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            StoryEffect::SetCamera {
                offset: Some([10.0, 20.0, 0.0]),
                rotation: Some([0.0, 0.0, 3.0]),
                scope: crate::script::CameraEffectScope::Canvas,
                ..
            }
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
        runtime.resume(Value::Int(0)).expect("selection resumes");
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
                parameters: vec![None, None],
                enabled: vec![true, true],
                options: vec!["Route A".into(), "Route B".into()],
            })
        );
        assert_eq!(runtime.step().expect("choice remains blocked"), None);
        runtime
            .resume(Value::Int(1))
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
    fn choice_navigation_defaults_to_no_preload_and_allows_override() {
        for (options, expected) in [
            ("", false),
            (", .{ preload: true }", true),
            (", .{ preload: false }", false),
        ] {
            let source = format!(
                r#"
                fn route() -> Never {{ story.goto("next.hks"{options}) }}
                choice {{ option("Alice") {{ route() }} }}
            "#
            );
            let code = compile_story_bytecode("entry.hks", &source).expect("navigation compiles");
            let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
            assert!(matches!(
                runtime.step().expect("choice"),
                Some(StoryRuntimeEvent::Choice { .. })
            ));
            // The default is execution-local and survives saving at the choice.
            runtime = StoryRuntime::restore(code, runtime.snapshot().expect("snapshot"))
                .expect("restore choice");
            runtime.resume(Value::Int(0)).expect("select option");
            let Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(request))) =
                runtime.step().expect("navigate")
            else {
                panic!("expected navigation");
            };
            assert_eq!(request.preload, Some(expected));
        }
    }

    #[test]
    fn no_preload_destination_passes_policy_through_calls_and_restore() {
        use crate::script::navigation::NavigationRequest;
        let code = compile_story_bytecode("branch.hks", "story.call(\"common.hks\")")
            .expect("compile call");
        let mut branch = StoryRuntime::new(code.clone()).expect("branch");
        branch.preload_calls = false;
        let mut branch =
            StoryRuntime::restore(code, branch.snapshot().expect("snapshot")).expect("restore");
        let Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(call))) =
            branch.step().expect("call")
        else {
            panic!("expected call");
        };
        assert!(!call.should_preload(Some(&branch)));
        let helper_code = compile_story_bytecode("common.hks", "story.call(\"nested.hks\")")
            .expect("compile helper");
        let mut helper = StoryRuntime::new(helper_code).expect("helper");
        helper.preload_calls = call.should_preload(Some(&branch));
        assert!(
            !NavigationRequest::call("nested.hks".into())
                .expect("call")
                .should_preload(Some(&helper))
        );
        let mut jump = NavigationRequest::goto("next.hks".into()).expect("goto");
        assert!(
            jump.should_preload(Some(&branch)),
            "later ordinary goto starts a new policy"
        );
        jump.preload = Some(false);
        assert!(!jump.should_preload(Some(&branch)));
        let regular =
            StoryRuntime::new(compile_story_bytecode("regular.hks", "").expect("compile"))
                .expect("regular");
        assert!(
            call.should_preload(Some(&regular)),
            "ordinary calls retain preloading"
        );
    }

    #[test]
    fn navigation_after_choice_keeps_normal_preload_default() {
        let code = compile_story_bytecode(
            "entry.hks",
            r#"
            choice { option("Alice") {} }
            story.goto("next.hks")
        "#,
        )
        .expect("compile");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        assert!(matches!(
            runtime.step().expect("choice"),
            Some(StoryRuntimeEvent::Choice { .. })
        ));
        runtime.resume(Value::Int(0)).expect("select");
        for _ in 0..10 {
            if let Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(request))) =
                runtime.step().expect("advance")
            {
                assert_eq!(request.preload, None);
                return;
            }
        }
        panic!("expected navigation after choice");
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
            .resume(Value::Int(0))
            .expect("choice response must start the selected branch");
        assert_eq!(
            runtime.step().expect("movie must suspend its branch"),
            Some(StoryRuntimeEvent::Wait(StoryWait::Movie {
                path: "opening".into(),
                fade_out_ms: 0,
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
                parameters: vec![None, None],
                enabled: vec![true, true],
                options: vec!["Route A".into(), "Route B".into()],
            })
        );
        restored
            .resume(Value::Int(1))
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
                fade_out_ms: 0,
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
                fade_out_ms: 0,
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
        assert!(matches!(
            runtime
                .step()
                .expect("root dialogue follows plan submission"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
        assert_eq!(
            runtime.step().expect("root waits"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
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
        let first = match runtime.step().expect("first voice must start") {
            Some(StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::PlayVoice { ref path, .. },
            }) if path == "voice/first" => task,
            event => panic!("unexpected first sequence event: {event:?}"),
        };
        assert!(matches!(
            runtime.step().expect("root dialogue follows submission"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
        assert!(matches!(
            runtime.step().expect("root waits"),
            Some(StoryRuntimeEvent::Wait(_))
        ));
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
