//! Actor-local appearance timeline, independent of placement and group opacity.
use super::AnimationSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActorBlur {
    pub radius: f32,
    pub target: f32,
    pub from: f32,
    pub elapsed: f32,
    pub animation: AnimationSpec,
    pub animation_id: Option<String>,
}

impl ActorBlur {
    pub fn validate(&self) -> Result<(), String> {
        super::actor_motion::ActorOffset {
            oscillation: None,
            target: [0.0; 2],
            animation: self.animation,
        }
        .validate()?;
        if [self.radius, self.target, self.from, self.elapsed]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
            || self.elapsed > self.animation.duration()
        {
            return Err("invalid actor blur timeline".into());
        }
        Ok(())
    }
    pub fn new(
        from: f32,
        target: f32,
        animation: AnimationSpec,
        animation_id: Option<String>,
    ) -> Self {
        Self {
            radius: from,
            target,
            from,
            elapsed: 0.0,
            animation,
            animation_id,
        }
    }
    pub fn advance(&mut self, delta: f32) -> bool {
        self.elapsed = (self.elapsed + delta).min(self.animation.duration());
        let fraction = if self.animation.duration() <= 0.0 {
            1.0
        } else {
            self.elapsed / self.animation.duration()
        };
        self.radius =
            (self.from + (self.target - self.from) * self.animation.sample(fraction)).max(0.0);
        if fraction >= 1.0 {
            self.radius = self.target;
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
    fn blur_interpolates_and_restores_without_restarting() {
        let mut blur = ActorBlur::new(0.0, 12.0, AnimationSpec::Linear(1.0, false), None);
        assert!(!blur.advance(0.5));
        assert_eq!(blur.radius, 6.0);
        let bytes = hiraku_script::hson::to_vec(&blur).expect("encode timeline");
        let mut restored: ActorBlur =
            hiraku_script::hson::from_slice(&bytes).expect("restore timeline");
        assert!(restored.advance(0.5));
        assert_eq!(restored.radius, 12.0);
    }
}
