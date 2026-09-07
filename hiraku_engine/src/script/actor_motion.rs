//! One relative actor translation. Sequencing belongs to story execution.
use super::animation::AnimationSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorOffset {
    pub target: [f32; 2],
    pub animation: AnimationSpec,
}

impl ActorOffset {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let seconds = match self.animation {
            AnimationSpec::Linear(s, _)
            | AnimationSpec::EaseIn(s, _)
            | AnimationSpec::EaseOut(s, _)
            | AnimationSpec::EaseInOut(s, _) => s,
        };
        if self
            .target
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 100000.0)
            || !seconds.is_finite()
            || !(0.0..=3600.0).contains(&seconds)
            || self.animation.repeats()
        {
            return Err("actor offset requires finite pixel coordinates and a non-repeating animation in 0..=3600 seconds".into());
        }
        Ok(())
    }
}

/// Retain completed revisions so restored executions can reattach their wait.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorMotion {
    pub revision: u64,
    pub transition: ActorOffset,
    pub origin: [f32; 2],
    pub offset: [f32; 2],
    pub elapsed: f32,
    pub finished: bool,
    #[serde(skip)]
    pub(crate) animation_id: Option<String>,
}

impl ActorMotion {
    pub(crate) fn new(revision: u64, transition: ActorOffset, origin: [f32; 2]) -> Self {
        Self {
            revision,
            transition,
            origin,
            offset: origin,
            elapsed: 0.0,
            finished: false,
            animation_id: None,
        }
    }

    pub(crate) fn advance(&mut self, seconds: f32) {
        if self.finished {
            return;
        }
        let duration = self.transition.animation.duration();
        self.elapsed = (self.elapsed + seconds).min(duration);
        self.finished = self.elapsed >= duration;
        let t = if self.finished {
            1.0
        } else {
            self.transition.animation.sample(self.elapsed / duration)
        };
        for i in 0..2 {
            self.offset[i] = self.origin[i] + (self.transition.target[i] - self.origin[i]) * t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_translation_restores_progress() {
        let transition = ActorOffset {
            target: [0.0, 20.0],
            animation: AnimationSpec::EaseOut(0.2, false),
        };
        transition.validate().expect("valid translation");
        let mut motion = ActorMotion::new(1, transition, [0.0; 2]);
        motion.advance(0.1);
        assert!((motion.offset[1] - 15.0).abs() < 0.001);
        let bytes = hiraku_script::hson::to_vec(&motion).expect("serialize motion");
        let mut restored: ActorMotion =
            hiraku_script::hson::from_slice(&bytes).expect("restore motion");
        motion.advance(0.1);
        restored.advance(0.1);
        assert_eq!(motion, restored);
        assert!(restored.finished);
        assert_eq!(restored.offset, [0.0, 20.0]);
    }
    #[test]
    fn zero_duration_and_invalid_animation() {
        let mut transition = ActorOffset {
            target: [1.0, 2.0],
            animation: AnimationSpec::Linear(0.0, false),
        };
        let mut motion = ActorMotion::new(2, transition, [0.0; 2]);
        motion.advance(0.0);
        assert_eq!(motion.offset, [1.0, 2.0]);
        assert!(motion.finished);
        for seconds in [-1.0, f64::NAN, f64::INFINITY] {
            transition.animation = AnimationSpec::Linear(seconds, false);
            assert!(transition.validate().is_err());
        }
        transition.animation = AnimationSpec::Linear(1.0, true);
        assert!(transition.validate().is_err());
    }
}
