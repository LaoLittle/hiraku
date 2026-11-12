use std::collections::BTreeMap;

use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use super::{ExecutionId, ScriptRequestId, ScriptResponse, ScriptResponseMessage};
use crate::script::story_runtime::StoryRuntime;

pub struct ScriptCallFrame {
    pub script: String,
    pub story: StoryRuntime,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraEffectScope {
    Background,
    #[default]
    World,
    Canvas,
    Ui,
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
    pub(crate) recovered_presentation: Option<super::replay_restore::Presentation>,
    pub(crate) restore_batch_size: usize,
    /// Preparation runs separately; the live session remains intact until it succeeds.
    pub(crate) restore_pending: bool,
    pub replay: Option<super::replay::ReplayJournal>,
    pub replay_dialogue: String,
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
    pub mounted_ui_overlays: BTreeMap<String, crate::state::MountedUiOverlay>,
    pub response_inbox: BTreeMap<ScriptRequestId, ScriptResponse>,
    pub task_requests: BTreeMap<ScriptRequestId, (ExecutionId, super::capabilities::StoryEffect)>,
    next_request_id: u64,
}

impl ScriptRuntimeState {
    pub(crate) fn overlay_matches(
        &self,
        name: &str,
        component: &str,
        arguments: &[crate::state::StoredValue],
    ) -> bool {
        self.mounted_ui_overlays
            .get(name)
            .is_some_and(|overlay| overlay.component == component && overlay.arguments == arguments)
    }

    pub(crate) fn cancel_pending_requests(&mut self) {
        self.pending_ui_screen = None;
        self.pending_ui_arguments.clear();
        self.wait_request = None;
        self.response_inbox.clear();
        self.task_requests.clear();
    }

    pub(crate) fn restore_presentation(&mut self, scene: &mut crate::state::SceneSnapshot) {
        if let Some(presentation) = self.recovered_presentation.take() {
            *scene = crate::state::SceneSnapshot {
                dialogue: presentation.dialogue,
                actor_blurs: presentation.actor_blurs,
                actor_tints: presentation.actor_tints,
                ..Default::default()
            };
            self.restore_batch_size = presentation.effects.len();
            if let Some(story) = self.story.as_mut() {
                story.prepend_effects(presentation.effects);
            }
        }
    }

    pub(crate) fn apply_restored(&mut self, mut prepared: Self) {
        // Old ECS responses must never collide with a new session's requests.
        prepared.next_request_id = self.next_request_id;
        *self = prepared;
    }

    pub fn allocate_request(&mut self) -> ScriptRequestId {
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .expect("script request identifier space must not be exhausted");
        ScriptRequestId(self.next_request_id)
    }

    pub fn accept_response(&mut self, message: ScriptResponseMessage) {
        if self.wait_request == Some(message.request) {
            self.response_inbox
                .entry(message.request)
                .or_insert(message.response);
        }
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
    fn persistent_overlay_identity_includes_component_and_arguments() {
        use crate::state::{MountedUiOverlay, StoredValue};
        let mut runtime = ScriptRuntimeState::default();
        let arguments = vec![StoredValue::Int(1)];
        runtime.mounted_ui_overlays.insert(
            "alice".into(),
            MountedUiOverlay {
                component: "alice.ui.hks".into(),
                arguments: arguments.clone(),
            },
        );
        assert!(runtime.overlay_matches("alice", "alice.ui.hks", &arguments));
        assert!(!runtime.overlay_matches("alice", "bob.ui.hks", &arguments));
        assert!(!runtime.overlay_matches("alice", "alice.ui.hks", &[StoredValue::Int(2)]));
        runtime.mounted_ui_overlays.remove("alice");
        assert!(!runtime.overlay_matches("alice", "alice.ui.hks", &arguments));
    }

    #[test]
    fn script_request_ids_are_stable_and_monotonic() {
        let mut runtime = ScriptRuntimeState::default();
        assert_eq!(runtime.allocate_request(), ScriptRequestId(1));
        assert_eq!(runtime.allocate_request(), ScriptRequestId(2));
    }

    #[test]
    fn restored_sessions_do_not_reuse_previous_request_ids() {
        let mut live = ScriptRuntimeState::default();
        let previous = live.allocate_request();
        live.restore_pending = true;
        live.apply_restored(ScriptRuntimeState::default());
        assert!(!live.restore_pending);
        assert!(live.allocate_request() > previous);
    }

    #[test]
    fn cancelled_requests_cannot_resume_a_new_wait_or_accumulate_in_the_inbox() {
        let mut runtime = ScriptRuntimeState::default();
        let old = runtime.allocate_request();
        runtime.wait_request = Some(old);
        runtime.pending_ui_screen = Some("alice.hks".into());
        runtime.accept_response(ScriptResponseMessage {
            request: old,
            response: ScriptResponse::Continue,
        });
        runtime.cancel_pending_requests();
        assert!(runtime.pending_ui_screen.is_none());
        assert!(runtime.response_inbox.is_empty());
        let current = runtime.allocate_request();
        assert!(current > old);
        runtime.wait_request = Some(current);
        runtime.accept_response(ScriptResponseMessage {
            request: old,
            response: ScriptResponse::Continue,
        });
        assert!(runtime.response_inbox.is_empty());
        runtime.accept_response(ScriptResponseMessage {
            request: current,
            response: ScriptResponse::Continue,
        });
        runtime.accept_response(ScriptResponseMessage {
            request: current,
            response: ScriptResponse::Choice(crate::state::StoredValue::String("bob".into())),
        });
        assert!(matches!(
            runtime.take_response(current),
            Some(ScriptResponse::Continue)
        ));
        runtime.wait_request = None;
        runtime.accept_response(ScriptResponseMessage {
            request: current,
            response: ScriptResponse::Continue,
        });
        assert!(runtime.response_inbox.is_empty());
    }
}
