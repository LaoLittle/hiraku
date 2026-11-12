use super::AnimationSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActorTint {
    pub color: [f32; 4],
    pub from: [f32; 4],
    pub target: [f32; 4],
    pub elapsed: f32,
    pub animation: AnimationSpec,
    pub animation_id: Option<String>,
}
impl ActorTint {
    pub fn new(
        from: [f32; 4],
        target: [f32; 4],
        animation: AnimationSpec,
        animation_id: Option<String>,
    ) -> Self {
        Self {
            color: from,
            from,
            target,
            elapsed: 0.0,
            animation,
            animation_id,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        super::actor_motion::ActorOffset {
            oscillation: None,
            target: [0.0; 2],
            animation: self.animation,
        }
        .validate()?;
        if self
            .color
            .into_iter()
            .chain(self.from)
            .chain(self.target)
            .any(|c| !c.is_finite() || !(0.0..=1.0).contains(&c))
            || !self.elapsed.is_finite()
            || self.elapsed < 0.0
            || self.elapsed > self.animation.duration()
        {
            return Err("invalid actor tint timeline".into());
        }
        Ok(())
    }
    pub fn advance(&mut self, delta: f32) -> bool {
        self.elapsed = (self.elapsed + delta.max(0.0)).min(self.animation.duration());
        let progress = if self.animation.duration() <= 0.0 {
            1.0
        } else {
            self.elapsed / self.animation.duration()
        };
        let fraction = self.animation.sample(progress);
        self.color = std::array::from_fn(|i| {
            (self.from[i] + (self.target[i] - self.from[i]) * fraction).clamp(0.0, 1.0)
        });
        if progress >= 1.0 {
            self.color = self.target;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_progress_restores_without_restarting() {
        let mut tint = ActorTint::new(
            [1.0; 4],
            [1.0, 0.0, 0.0, 0.5],
            AnimationSpec::Linear(1.0, false),
            None,
        );
        assert!(!tint.advance(0.5));
        assert_eq!(tint.color, [1.0, 0.5, 0.5, 0.75]);
        let bytes = hiraku_script::bhson::to_vec(&tint).expect("encode");
        let mut restored: ActorTint = hiraku_script::bhson::from_slice(&bytes).expect("restore");
        restored.validate().expect("valid tint");
        assert!(restored.advance(0.5));
        assert_eq!(restored.color, [1.0, 0.0, 0.0, 0.5]);
    }

    #[test]
    fn scene_storage_roundtrip_preserves_tint_and_validates_channels() {
        let mut tint = ActorTint::new(
            [1.0; 4],
            [1.0, 0.0, 0.0, 0.5],
            AnimationSpec::Linear(1.0, false),
            None,
        );
        tint.advance(0.5);
        let scene = crate::state::SceneSnapshot {
            actor_tints: std::collections::BTreeMap::from([("alice".into(), tint)]),
            ..Default::default()
        };
        let proto = crate::proto::SceneSnapshot::from(&scene);
        let restored = crate::state::SceneSnapshot::try_from(proto.clone()).expect("restore scene");
        assert_eq!(restored.actor_tints["alice"].color, [1.0, 0.5, 0.5, 0.75]);
        assert_eq!(restored.actor_tints["alice"].elapsed, 0.5);
        let mut empty = proto;
        empty.actor_tints_hson.clear();
        assert!(
            crate::state::SceneSnapshot::try_from(empty)
                .expect("default tint state")
                .actor_tints
                .is_empty()
        );
        let mut invalid = restored.actor_tints["alice"].clone();
        invalid.target[0] = f32::NAN;
        assert!(invalid.validate().is_err());
    }
}
