use hiraku_script::{
    FunctionSignature, ScriptType,
    native::{FromHksValue, HksNativeType, NativeError, NativeRegistry, RegistrationError},
};
use serde::{Deserialize, Serialize};

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnimationSpec {
    Linear(f64, bool),
    EaseIn(f64, bool),
    EaseOut(f64, bool),
    EaseInOut(f64, bool),
}

#[allow(non_snake_case)]
impl AnimationSpec {
    fn linear(seconds: f64) -> AnimationSpec { Self::Linear(seconds, false) }
    fn easeIn(seconds: f64) -> AnimationSpec { Self::EaseIn(seconds, false) }
    fn easeOut(seconds: f64) -> AnimationSpec { Self::EaseOut(seconds, false) }
    fn easeInOut(seconds: f64) -> AnimationSpec { Self::EaseInOut(seconds, false) }
}
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnimationPhase {
    Transform(f64, f64, f64, f64),
}

impl AnimationPhase {
    fn rotation(degrees: f64) -> AnimationPhase { Self::Transform(degrees, 1.0, 0.0, 0.0) }
    fn scale(value: f64) -> AnimationPhase { Self::Transform(0.0, value, 0.0, 0.0) }
    fn offset(x: f64, y: f64) -> AnimationPhase { Self::Transform(0.0, 1.0, x, y) }
    fn transform(rotation: f64, scale: f64, x: f64, y: f64) -> AnimationPhase {
        Self::Transform(rotation, scale, x, y)
    }
}
}

impl AnimationPhase {
    pub fn values(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Transform(rotation, scale, x, y) => {
                (rotation as f32, scale as f32, x as f32, y as f32)
            }
        }
    }
}

impl AnimationSpec {
    pub fn duration(self) -> f32 {
        match self {
            Self::Linear(value, _)
            | Self::EaseIn(value, _)
            | Self::EaseOut(value, _)
            | Self::EaseInOut(value, _) => value.max(0.0) as f32,
        }
    }

    pub fn repeats(self) -> bool {
        match self {
            Self::Linear(_, repeat)
            | Self::EaseIn(_, repeat)
            | Self::EaseOut(_, repeat)
            | Self::EaseInOut(_, repeat) => repeat,
        }
    }

    pub fn sample(self, progress: f32) -> f32 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear(..) => progress,
            Self::EaseIn(..) => progress * progress,
            Self::EaseOut(..) => 1.0 - (1.0 - progress) * (1.0 - progress),
            Self::EaseInOut(..) => {
                if progress < 0.5 {
                    2.0 * progress * progress
                } else {
                    1.0 - (-2.0 * progress + 2.0).powi(2) / 2.0
                }
            }
        }
    }

    pub(crate) fn repeat_forever(self) -> Self {
        match self {
            Self::Linear(value, _) => Self::Linear(value, true),
            Self::EaseIn(value, _) => Self::EaseIn(value, true),
            Self::EaseOut(value, _) => Self::EaseOut(value, true),
            Self::EaseInOut(value, _) => Self::EaseInOut(value, true),
        }
    }
}

pub fn register_animation_api<C: 'static>(
    registry: &mut NativeRegistry<C>,
) -> Result<(), RegistrationError> {
    let owner = AnimationSpec::register_hks(registry)?;
    AnimationPhase::register_hks(registry)?;
    let builtin = registry.register_raw_fn("repeatForever", move |_context, call| {
        let value = call
            .receiver
            .as_ref()
            .ok_or(NativeError::TypeMismatch("expected AnimationSpec receiver"))?;
        Ok(AnimationSpec::from_hks_value(value)?
            .repeat_forever()
            .into_hks_typed(owner))
    })?;
    registry.set_signature(
        builtin,
        FunctionSignature {
            receiver: Some(ScriptType::Named(owner)),
            parameters: Vec::new(),
            result: ScriptType::Named(owner),
        },
    )?;
    Ok(())
}
