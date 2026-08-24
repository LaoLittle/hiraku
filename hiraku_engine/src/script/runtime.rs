use std::collections::{BTreeMap, VecDeque};

use bevy::ecs::{resource::Resource, system::ResMut};
use serde::{Deserialize, Serialize};

use super::{ScriptRequestId, ScriptResponse, ScriptResponseMessage};
use crate::script::hks_runtime::{StoryRuntime, StoryRuntimeEvent};

pub struct ScriptCallFrame {
    pub script: String,
    pub story: StoryRuntime,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CameraEffectScope {
    #[default]
    World,
    Canvas,
}

/// ECS-facing state for the currently running HKS story.
///
/// The bytecode VM and native host own script execution state. This resource only
/// keeps the request/response boundary needed to coordinate the VM with Bevy systems.
#[derive(Default, Resource)]
pub struct ScriptRuntimeState {
    pub story: Option<StoryRuntime>,
    pub story_events: VecDeque<StoryRuntimeEvent>,
    pub wait_request: Option<ScriptRequestId>,
    pub current_script: Option<String>,
    pub call_stack: Vec<ScriptCallFrame>,
    pub pending_ui_screen: Option<String>,
    pub response_inbox: BTreeMap<ScriptRequestId, ScriptResponse>,
    pub task_requests: BTreeMap<ScriptRequestId, u64>,
    next_request_id: u64,
}

impl ScriptRuntimeState {
    pub fn allocate_request(&mut self) -> ScriptRequestId {
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .expect("script request identifier space must not be exhausted");
        ScriptRequestId(self.next_request_id)
    }

    pub fn accept_response(&mut self, message: ScriptResponseMessage) {
        self.response_inbox
            .insert(message.request, message.response);
    }

    pub fn take_response(&mut self, request: ScriptRequestId) -> Option<ScriptResponse> {
        self.response_inbox.retain(|id, _| *id >= request);
        self.response_inbox.remove(&request)
    }

    fn tick(&mut self) {
        if !self.story_events.is_empty() {
            return;
        }
        let Some(story) = self.story.as_mut() else {
            return;
        };
        match story.step() {
            Ok(Some(event)) => self.story_events.push_back(event),
            Ok(None) => {}
            Err(error) => {
                bevy::log::warn!("HKS runtime failed: {error}");
                self.story = None;
            }
        }
    }
}

pub fn tick_script_runtime(mut runtime: ResMut<ScriptRuntimeState>) {
    runtime.tick();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_request_ids_are_stable_and_monotonic() {
        let mut runtime = ScriptRuntimeState::default();
        assert_eq!(runtime.allocate_request(), ScriptRequestId(1));
        assert_eq!(runtime.allocate_request(), ScriptRequestId(2));
    }
}
