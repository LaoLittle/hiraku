//! Value-based layer settings: evaluating a builder has no rendering side effects.
use super::{CameraScope, CharacterContext, StoryEffect};
use crate::{effect::post_process::EffectParameters, script::CameraEffectScope};
use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{
    Value,
    native::{FromHksValue, HksNativeType},
};

hiraku_script::hks_define! {
#[derive(Clone, Copy)]
enum PostProcess {
    Settings(f64, f64, f64, f64, f64, f64),
}
impl PostProcess {
    #[getter]
    fn identity() -> PostProcess { Self::Settings(0.0, 0.0, 1.0, 0.0, 0.0, 0.0) }
}
}

#[hiraku_script::hks_module]
mod modifiers {
    use super::*;
    #[hks(
        selector = "PostProcess",
        receiver = "PostProcess",
        result = "PostProcess"
    )]
    fn blur(
        context: &mut CharacterContext,
        effect: Value,
        radius: f64,
    ) -> Result<Value, NativeError> {
        let _ = context;
        if !radius.is_finite() || !(0.0..=128.0).contains(&radius) {
            return Err(NativeError::message(
                "blur radius must be in 0..=128 texture pixels",
            ));
        }
        modify(effect, |PostProcess::Settings(_, exposure, saturation, r, g, b)| {
            PostProcess::Settings(radius, exposure, saturation, r, g, b)
        })
    }

    #[hks(
        selector = "PostProcess",
        receiver = "PostProcess",
        result = "PostProcess"
    )]
    fn exposure(
        context: &mut CharacterContext,
        effect: Value,
        stops: f64,
    ) -> Result<Value, NativeError> {
        let _ = context;
        if !stops.is_finite() || !(-16.0..=16.0).contains(&stops) {
            return Err(NativeError::message("exposure must be in -16..=16 stops"));
        }
        modify(effect, |PostProcess::Settings(blur, _, saturation, r, g, b)| {
            PostProcess::Settings(blur, stops, saturation, r, g, b)
        })
    }

    #[hks(
        selector = "PostProcess",
        receiver = "PostProcess",
        result = "PostProcess"
    )]
    fn saturation(
        context: &mut CharacterContext,
        effect: Value,
        value: f64,
    ) -> Result<Value, NativeError> {
        let _ = context;
        if !value.is_finite() || !(0.0..=8.0).contains(&value) {
            return Err(NativeError::message("saturation must be in 0..=8"));
        }
        modify(effect, |PostProcess::Settings(blur, exposure, _, r, g, b)| {
            PostProcess::Settings(blur, exposure, value, r, g, b)
        })
    }

    #[hks(
        name = "grayscaleGamma",
        selector = "PostProcess",
        receiver = "PostProcess",
        result = "PostProcess"
    )]
    fn grayscale_gamma(
        context: &mut CharacterContext,
        effect: Value,
        red: f64,
        green: f64,
        blue: f64,
    ) -> Result<Value, NativeError> {
        let _ = context;
        if [red, green, blue].into_iter().any(|v| !v.is_finite() || v <= 0.0 || v > 16.0) {
            return Err(NativeError::message("grayscale gamma must be in 0..=16"));
        }
        modify(effect, |PostProcess::Settings(blur, exposure, saturation, _, _, _)| {
            PostProcess::Settings(blur, exposure, saturation, red, green, blue)
        })
    }
}

fn modify(
    value: Value,
    update: impl FnOnce(PostProcess) -> PostProcess,
) -> Result<Value, NativeError> {
    let effect = PostProcess::from_hks_value(&value)?;
    let Value::Typed { type_id, .. } = value else {
        return Err(NativeError::TypeMismatch("PostProcess"));
    };
    Ok(update(effect).into_hks_typed(type_id))
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    PostProcess::register_hks(registry).expect("post-process type registration must be consistent");
    modifiers::register_hks(registry).expect("post-process modifiers must be consistent");
    api::register_hks(registry).expect("post-process API registration must be consistent");
}

#[hiraku_script::hks_module("scene")]
mod api {
    use super::*;

    #[hks(name = "postProcess")]
    fn post_process(
        context: &mut CharacterContext,
        scope: CameraScope,
        effect: PostProcess,
    ) -> Result<(), NativeError> {
        let scope = match scope {
            CameraScope::Scene => CameraEffectScope::World,
            CameraScope::Ui => CameraEffectScope::Ui,
            CameraScope::Canvas => CameraEffectScope::Canvas,
        };
        context.commands.push(StoryEffect::PostProcess {
            scope,
            parameters: effect.parameters(),
        });
        Ok(())
    }

    #[hks(name = "postProcessPicture")]
    fn picture(
        context: &mut CharacterContext,
        id: String,
        effect: PostProcess,
    ) -> Result<(), NativeError> {
        if id.trim().is_empty() {
            return Err(NativeError::message("picture identity must not be empty"));
        }
        context.commands.push(StoryEffect::Picture(
            crate::scene::pictures::PictureCommand::PostProcess {
                id,
                parameters: effect.parameters(),
            },
        ));
        Ok(())
    }
}

impl PostProcess {
    fn parameters(self) -> EffectParameters {
        let Self::Settings(blur, exposure, saturation, red, green, blue) = self;
        EffectParameters {
            blur_radius: blur as f32,
            exposure: exposure as f32,
            saturation: saturation as f32,
            grayscale_gamma: if red > 0.0 {
                bevy::prelude::Vec4::new(red as f32, green as f32, blue as f32, 1.0)
            } else {
                bevy::prelude::Vec4::ZERO
            },
            ..Default::default()
        }
    }
}
