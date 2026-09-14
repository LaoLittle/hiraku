//! Atomic boundary between the script-owned Actor patch and presentation state.
use super::*;
use hiraku_script::native::FromHksValue;

pub(super) fn apply(
    context: &mut CharacterContext,
    patch: Value,
    wait: bool,
) -> Result<(), NativeError> {
    let patch = match patch {
        Value::Typed { value, .. } => *value,
        value => value,
    };
    let Value::Map(fields) = patch else {
        return Err(NativeError::TypeMismatch("Actor patch"));
    };
    fn field<T: FromHksValue>(
        fields: &BTreeMap<String, Value>,
        name: &str,
    ) -> Result<T, NativeError> {
        T::from_hks_value(
            fields
                .get(name)
                .ok_or_else(|| NativeError::message(format!("actor patch is missing `{name}`")))?,
        )
    }
    // Decode the complete contract before changing any presentation state.
    let actor: ActorIdentity = field(&fields, "identity")?;
    let expressions: Vec<String> = field(&fields, "expressions")?;
    let position: Option<Position> = field(&fields, "position")?;
    let scale: Option<f64> = field(&fields, "scaleValue")?;
    let focus: Option<bool> = field(&fields, "focused")?;
    let rotation: Option<f64> = field(&fields, "rotationValue")?;
    let depth: Option<f64> = field(&fields, "depthValue")?;
    let clip: Option<Option<String>> = field(&fields, "clipValue")?;
    let showing: Option<bool> = field(&fields, "showing")?;
    let hiding: Option<f64> = field(&fields, "hiding")?;
    let offset: Option<Position> = field(&fields, "offsetValue")?;
    let oscillation: Option<Position> = field(&fields, "oscillation")?;
    let period_x: f64 = field(&fields, "periodX")?;
    let period_y: f64 = field(&fields, "periodY")?;
    let stopping: bool = field(&fields, "stopping")?;
    let seconds: Option<f64> = field(&fields, "seconds")?;
    let easing: Option<Easing> = field(&fields, "curve")?;

    for expression in expressions {
        native_api::native_emotion(context, actor, expression)?;
    }
    if let Some(position) = position {
        native_api::native_at(context, actor, position)?;
    }
    if let Some(scale) = scale {
        native_api::native_scale(context, actor, scale)?;
    }
    if let Some(focused) = focus {
        native_api::native_focus(context, actor, Some(focused))?;
    }
    if let Some(rotation) = rotation {
        native_api::native_actor_rotation(context, actor, rotation)?;
    }
    if let Some(depth) = depth {
        native_api::native_actor_depth(context, actor, depth)?;
    }
    if let Some(clip) = clip {
        native_api::native_actor_clip(context, actor, clip)?;
    }
    if showing == Some(true) {
        native_api::native_show(context, actor)?;
    }
    if stopping {
        native_api::native_actor_stop_motion(context, actor)?;
    }
    if let Some(offset) = offset {
        native_api::native_actor_offset(context, actor, offset)?;
    }
    if let Some(amplitude) = oscillation {
        native_api::native_actor_oscillate(context, actor, amplitude, period_x, period_y)?;
    }
    if let Some(seconds) = seconds {
        native_api::actor_time(context, actor, seconds)?;
    }
    if let Some(easing) = easing {
        native_api::actor_easing(context, actor, easing)?;
    }
    if let Some(hiding) = hiding {
        native_api::native_hide(context, actor, Some(hiding))?;
    }
    if wait {
        native_api::await_actor(context, actor)?;
    }
    context
        .commit_actor(actor.0)
        .map_err(|error| NativeError::message(error.to_string()))
}
