//! One relative actor translation. Sequencing belongs to story execution.
use super::animation::AnimationSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorOffset {
    /// A transient additive wave; never replaces the actor's placement trajectory.
    pub oscillation: Option<ActorOscillation>,
    pub target: [f32; 2],
    pub animation: AnimationSpec,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActorOscillation {
    pub amplitude: [f32; 2],
    pub period: [f32; 2],
}

impl ActorOffset {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let seconds = self.animation.seconds();
        self.animation
            .easing()
            .validate()
            .map_err(|e| e.to_string())?;
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
        if let Some(wave) = self.oscillation {
            if wave
                .amplitude
                .iter()
                .any(|x| !x.is_finite() || x.abs() > 100000.0)
                || wave.period.iter().any(|x| !x.is_finite() || *x <= 0.0)
            {
                return Err("oscillation requires finite amplitudes and positive periods".into());
            }
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
    /// Cancel translation at its sampled pose; remove transient waves.
    pub(crate) fn stop(&mut self) {
        if self.transition.oscillation.take().is_some() {
            self.offset = self.origin;
        }
        self.transition.target = self.offset;
        self.finish();
    }
    pub(crate) fn finish(&mut self) {
        self.offset = if self.transition.oscillation.is_some() {
            self.origin
        } else {
            self.transition.target
        };
        self.elapsed = self.transition.animation.duration();
        self.finished = true;
    }

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
        if let Some(wave) = self.transition.oscillation {
            let phase_time = if self.finished {
                duration
            } else {
                self.transition.animation.sample(self.elapsed / duration) * duration
            };
            for axis in 0..2 {
                self.offset[axis] = self.origin[axis]
                    + if self.finished {
                        0.0
                    } else {
                        wave.amplitude[axis]
                            * (std::f32::consts::TAU * phase_time / wave.period[axis]).sin()
                    };
            }
            return;
        }
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
    fn oscillation_samples_axes_independently_and_restores_without_phase_reset() {
        let mut wave = ActorMotion::new(
            1,
            ActorOffset {
                target: [0.0; 2],
                animation: AnimationSpec::Linear(0.25, false),
                oscillation: Some(ActorOscillation {
                    amplitude: [5.0, 5.0],
                    period: [0.1, 0.02],
                }),
            },
            [10.0, 20.0],
        );
        wave.transition.validate().expect("valid oscillator");
        wave.advance(0.005);
        assert!((wave.offset[1] - 25.0).abs() < 0.0001);
        assert!(wave.offset[0] > 10.0 && wave.offset[0] < 15.0);
        let bytes = hiraku_script::hson::to_string(&wave).expect("snapshot");
        let mut restored: ActorMotion = hiraku_script::hson::from_str(&bytes).expect("restore");
        wave.advance(0.01);
        restored.advance(0.01);
        assert_eq!(restored.offset, wave.offset);
        wave.finish();
        restored.advance(10.0);
        assert_eq!(restored.offset, [10.0, 20.0]);
        assert_eq!(restored.offset, wave.offset);
        assert!(restored.finished);
    }

    #[test]
    fn invalid_wave_periods_fail_and_stop_removes_transient_displacement() {
        let mut motion = ActorMotion::new(
            1,
            ActorOffset {
                target: [0.0; 2],
                animation: AnimationSpec::Linear(1.0, false),
                oscillation: Some(ActorOscillation {
                    amplitude: [5.0; 2],
                    period: [0.1; 2],
                }),
            },
            [0.0; 2],
        );
        motion.advance(0.025);
        motion.stop();
        assert_eq!(motion.offset, [0.0; 2]);
        let mut invalid = motion.transition;
        invalid.oscillation = Some(ActorOscillation {
            amplitude: [5.0; 2],
            period: [0.0, 0.1],
        });
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn stopped_motion_keeps_sampled_pose_after_restore_and_finish() {
        let mut motion = ActorMotion::new(
            1,
            ActorOffset {
                oscillation: None,
                target: [0.0, 20.0],
                animation: AnimationSpec::Linear(1.0, false),
            },
            [0.0; 2],
        );
        motion.advance(0.25);
        motion.stop();
        assert_eq!(motion.offset, [0.0, 5.0]);
        let bytes = hiraku_script::hson::to_vec(&motion).expect("serialize stopped motion");
        let mut restored: ActorMotion =
            hiraku_script::hson::from_slice(&bytes).expect("restore stopped motion");
        restored.advance(1.0);
        restored.finish();
        assert_eq!(restored.offset, [0.0, 5.0]);
        assert!(restored.finished);
    }
    #[test]
    fn atomic_translation_restores_progress() {
        let transition = ActorOffset {
            oscillation: None,
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
            oscillation: None,
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
