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
    let dissolve_mask: Option<String> = field(&fields, "dissolveMask")?;
    let dissolve_softness: Option<f64> = field(&fields, "dissolveSoftness")?;
    let showing: Option<bool> = field(&fields, "showing")?;
    let hiding: Option<f64> = field(&fields, "hiding")?;
    let offset: Option<Position> = field(&fields, "offsetValue")?;
    let oscillation: Option<Position> = field(&fields, "oscillation")?;
    let jitter: Option<Position> = field(&fields, "jitterValue")?;
    let jitter_interval: f64 = field(&fields, "jitterInterval")?;
    let period_x: f64 = field(&fields, "periodX")?;
    let period_y: f64 = field(&fields, "periodY")?;
    let stopping: bool = field(&fields, "stopping")?;
    let seconds: Option<f64> = field(&fields, "seconds")?;
    let easing: Option<Easing> = field(&fields, "curve")?;

    // Visibility owns its own timeline. A hide must not create a placement
    // animation just to consume `.time`, or leave one behind for the next show.
    // Validate unsupported modifiers before touching any pending actor fields.
    let hide_ms = if let Some(legacy_ms) = hiding {
        if easing.is_some() {
            return Err(NativeError::message(
                "Actor.hide does not support easing yet; use .hide().time(seconds) for a linear fade",
            ));
        }
        Some(if let Some(seconds) = seconds {
            crate::script::animation::duration_millis(seconds)?
        } else {
            hide_duration(Some(legacy_ms))?
        })
    } else {
        None
    };

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
    if let Some(mask) = dissolve_mask {
        native_api::native_actor_dissolve(
            context,
            actor,
            mask,
            dissolve_softness
                .ok_or_else(|| NativeError::message("Actor.dissolve requires softness"))?,
        )?;
    } else if dissolve_softness.is_some() {
        return Err(NativeError::message("Actor.dissolve requires a mask"));
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
    if let Some(amplitude) = jitter {
        native_api::native_actor_jitter(context, actor, amplitude, jitter_interval)?;
    }
    if let Some(seconds) = seconds.filter(|_| hide_ms.is_none()) {
        native_api::actor_time(context, actor, seconds)?;
    }
    if let Some(easing) = easing {
        native_api::actor_easing(context, actor, easing)?;
    }
    if let Some(hide_ms) = hide_ms {
        native_api::native_hide_with_duration(context, actor, hide_ms)?;
    }
    if wait {
        native_api::await_actor(context, actor)?;
    }
    context
        .commit_actor(actor.0)
        .map_err(|error| NativeError::message(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{StoryRuntime, StoryRuntimeEvent};

    #[test]
    fn actor_dissolve_is_a_one_shot_typed_show_modifier() {
        let code = compile_story_bytecode(
            "test.hks",
            "char(\"alice\").dissolve(\"rule/door\", 0.25).show().time(0.5).await()",
        )
        .expect("compile actor dissolve");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        assert!(matches!(
            runtime.step().expect("submit actor dissolve"),
            Some(StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::ShowCharacter {
                    dissolve: Some((ref path, softness)), ..
                }, ..
            }) if path == "rule/door" && (softness - 0.25).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn actor_dissolve_does_not_leak_into_later_expression_changes() {
        let code = compile_story_bytecode(
            "test.hks",
            "char(\"alice\").dissolve(\"rule/door\", 0.25).show().time(0.5)\nchar(\"alice\").e(\"happy\")",
        )
        .expect("compile actor sequence");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let first = runtime.step().expect("first show");
        assert!(matches!(first, Some(StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter {
            dissolve: Some(_), ..
        }))));
        let second = runtime.step().expect("second show");
        assert!(matches!(second, Some(StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter {
            dissolve: None, ..
        }))));
    }

    #[test]
    fn script_actor_jitter_submits_a_timed_local_motion() {
        let code = compile_story_bytecode(
            "test.hks",
            "char(\"alice\").jitter(.pos(7.5, 7.5), 0.04).time(0.3)",
        )
        .expect("compile actor jitter");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        assert!(matches!(
            runtime.step().expect("submit jitter"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::ActorMotion {
                transition: crate::script::actor_motion::ActorOffset {
                    oscillation: Some(crate::script::actor_motion::ActorOscillation::Jitter {
                        interval, ..
                    }), ..
                }, ..
            })) if (interval - 0.04).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn hide_time_controls_visibility_in_either_modifier_order() {
        for (modifiers, duration) in [
            (".hide().time(0.4)", 400),
            (".time(0.4).hide()", 400),
            (".at(.right).time(0.4).hide()", 400),
            (".hide(200).time(0)", 0),
            (".hide().time(120)", 120000),
        ] {
            let code = compile_story_bytecode(
                "test.hks",
                &format!("let alice = char(\"alice\")\nalice{modifiers}.await()\nalice.show()"),
            )
            .expect("typed actor visibility");
            let mut runtime = StoryRuntime::new(code).expect("runtime");
            let Some(StoryRuntimeEvent::TaskEffect { task, effect }) =
                runtime.step().expect("hide")
            else {
                panic!("expected awaitable hide: {modifiers}");
            };
            assert!(
                matches!(&effect, StoryEffect::HideCharacter { fade_ms, .. } if *fade_ms == duration),
                "{modifiers}: {effect:?}"
            );
            runtime
                .complete_task_effect(task, &effect)
                .expect("complete hide");
            let Some(StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter {
                placement_animation,
                ..
            })) = runtime.step().expect("show")
            else {
                panic!("expected show");
            };
            assert!(
                placement_animation.is_none(),
                "hide timing must not leak into show"
            );
        }
    }

    #[test]
    fn unsupported_hide_easing_reports_visibility_not_placement_error() {
        let code = compile_story_bytecode(
            "test.hks",
            "char(\"alice\").hide().time(0.4).easing(.easeOut)",
        )
        .expect("compile receiver");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let error = runtime.step().expect_err("unsupported hide easing");
        assert!(
            error
                .to_string()
                .contains("Actor.hide does not support easing")
        );
    }
}
