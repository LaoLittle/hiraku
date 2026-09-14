//! Recorded presentation work, independent of script call frames.
use super::capabilities::StoryEffect;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum PlanMode {
    Sequence,
    Parallel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct AnimationPlan {
    pub mode: PlanMode,
    batches: VecDeque<(Vec<StoryEffect>, bool)>,
    // Authoring advances the host through every step before playback starts.
    // Cancellation checks the final authored revision, while individual effects
    // retain their unique revision for ECS restore/reattachment identity.
    motion_revisions: BTreeMap<String, u64>,
}

impl AnimationPlan {
    pub fn new(mode: PlanMode) -> Self {
        Self {
            mode,
            batches: VecDeque::new(),
            motion_revisions: BTreeMap::new(),
        }
    }
    pub fn record(&mut self, effects: Vec<StoryEffect>, barrier: bool) {
        for effect in &effects {
            if let StoryEffect::ActorMotion {
                actor_id, revision, ..
            } = effect
            {
                self.motion_revisions.insert(actor_id.clone(), *revision);
            }
        }
        if !effects.is_empty() {
            self.batches.push_back((effects, barrier));
        } else if barrier && let Some(last) = self.batches.back_mut() {
            last.1 = true;
        }
    }
    pub fn next(&mut self) -> Option<Vec<StoryEffect>> {
        match self.mode {
            PlanMode::Sequence => self.batches.pop_front().map(|batch| batch.0),
            PlanMode::Parallel if !self.batches.is_empty() => {
                let mut effects = Vec::new();
                while let Some((batch, barrier)) = self.batches.pop_front() {
                    effects.extend(batch);
                    if barrier {
                        break;
                    }
                }
                Some(effects)
            }
            PlanMode::Parallel => None,
        }
    }
    pub fn motion_revision(&self, actor: &str) -> Option<u64> {
        self.motion_revisions.get(actor).copied()
    }
}
