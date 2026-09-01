use std::collections::BTreeMap;

use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use super::{ScriptRequestId, ScriptResponse, ScriptResponseMessage};
use crate::script::hks_runtime::StoryRuntime;

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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CameraProjectionMode {
    #[default]
    Orthographic,
    Perspective,
}

/// ECS-facing state for the currently running HKS story.
///
/// The bytecode VM and native host own script execution state. This resource only
/// keeps the request/response boundary needed to coordinate the VM with Bevy systems.
#[derive(Default, Resource)]
pub struct ScriptRuntimeState {
    pub story: Option<StoryRuntime>,
    pub wait_request: Option<ScriptRequestId>,
    pub current_script: Option<String>,
    pub call_stack: Vec<ScriptCallFrame>,
    pub pending_ui_screen: Option<String>,
    pub pending_ui_arguments: Vec<crate::state::StoredValue>,
    /// Script-defined semantic UI role mappings. Values are normalized VFS paths.
    pub ui_registry: BTreeMap<String, String>,
    /// Non-modal UI mounts keyed by their stable script-provided mount name.
    /// Values retain the role or path used to mount so the component can be
    /// reconstructed after restoring a save.
    pub mounted_ui_overlays: BTreeMap<String, String>,
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
