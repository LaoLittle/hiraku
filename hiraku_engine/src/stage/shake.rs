use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageCameraShake {
    #[serde(default = "default_randomized")]
    pub randomized: bool,
    pub motion: u64,
    pub force: Vec3,
    pub frequency: u32,
    pub damping: f32,
    pub animation: crate::script::AnimationSpec,
    pub elapsed: f32,
    pub seed: u32,
}
pub(super) fn default_randomized() -> bool {
    true
}

impl StageCameraShake {
    pub(crate) fn valid(&self) -> bool {
        self.motion > 0
            && self.force.is_finite()
            && self.frequency > 0
            && self.damping.is_finite()
            && self.damping >= 0.0
            && self.animation.duration().is_finite()
            && self.animation.duration() > 0.0
            && self.animation.easing().validate().is_ok()
            && !self.animation.repeats()
            && self.elapsed.is_finite()
            && (0.0..=self.animation.duration()).contains(&self.elapsed)
    }

    pub fn offset(&self) -> Vec3 {
        let t = self
            .animation
            .sample(self.elapsed / self.animation.duration());
        if t <= 0.0 || t >= 1.0 {
            return Vec3::ZERO;
        }
        let frequency = self.frequency as f32;
        let envelope = (-frequency * self.damping / std::f32::consts::TAU * t).exp()
            * ((frequency - 0.5) * std::f32::consts::PI * t).sin();
        if !self.randomized {
            return self.force * envelope;
        }
        let mut bits = self.elapsed.to_bits() ^ self.seed ^ 0x9e37_79b9;
        bits ^= bits << 13;
        bits ^= bits >> 17;
        bits ^= bits << 5;
        let random = (f32::from_bits(0x3f80_0000 | (bits >> 9)) - 1.0) * 2.0 - 1.0;
        self.force * envelope * random
    }

    pub(crate) fn advance(&mut self, delta: f32) -> bool {
        self.elapsed = (self.elapsed + delta.max(0.0)).min(self.animation.duration());
        self.elapsed >= self.animation.duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> StageCameraShake {
        StageCameraShake {
            randomized: true,
            motion: 1,
            force: Vec3::new(0.4, -0.2, 0.0),
            frequency: 50,
            damping: 1.0,
            animation: crate::script::AnimationSpec::Linear(3.0, false),
            elapsed: 0.0,
            seed: 17,
        }
    }

    #[test]
    fn regular_oscillation_uses_the_envelope_without_random_noise() {
        let mut shake = fixture();
        shake.randomized = false;
        shake.frequency = 600;
        shake.damping = 0.0;
        shake.animation = crate::script::AnimationSpec::Linear(60.0, false);
        shake.advance(0.125);
        let expected = shake.force * ((599.5 * std::f32::consts::PI * 0.125 / 60.0).sin());
        assert!(shake.offset().abs_diff_eq(expected, 0.00001));
        let before = shake.offset();
        shake.seed = 99;
        assert_eq!(shake.offset(), before);
        let bytes = hiraku_script::bhson::to_vec(&shake).expect("serialize mode");
        let restored: StageCameraShake =
            hiraku_script::bhson::from_slice(&bytes).expect("restore mode");
        assert!(!restored.randomized);
        assert_eq!(restored.offset(), before);
    }

    #[test]
    fn endpoints_are_exactly_zero_and_clock_pause_preserves_offset() {
        let mut shake = fixture();
        assert_eq!(shake.offset(), Vec3::ZERO);
        shake.advance(0.03);
        let sampled = shake.offset();
        assert_ne!(sampled, Vec3::ZERO);
        shake.advance(0.0);
        assert_eq!(sampled, shake.offset());
        assert!(shake.advance(3.0));
        assert_eq!(shake.offset(), Vec3::ZERO);
    }

    #[test]
    fn restore_and_split_frame_steps_produce_the_same_sample() {
        let mut a = fixture();
        a.advance(0.5);
        let encoded = hiraku_script::bhson::to_vec(&a).expect("encode shake");
        let restored: StageCameraShake =
            hiraku_script::bhson::from_slice(&encoded).expect("restore shake");
        let mut b = fixture();
        b.advance(0.25);
        b.advance(0.25);
        assert_eq!(a.offset(), restored.offset());
        assert_eq!(a.offset(), b.offset());
        assert!(restored.valid());
    }

    #[test]
    fn invalid_snapshot_parameters_are_rejected() {
        let mut shake = fixture();
        shake.frequency = 0;
        assert!(!shake.valid());
        shake = fixture();
        shake.damping = -1.0;
        assert!(!shake.valid());
        shake = fixture();
        shake.elapsed = f32::NAN;
        assert!(!shake.valid());
    }
}
