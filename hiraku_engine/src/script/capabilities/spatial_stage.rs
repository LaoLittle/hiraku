use super::scene_visuals::{
    SceneVisualTarget, StageAnimationHandle, StageCameraTransitionHandle, StageViewTransitionHandle,
};
use super::*;
use crate::stage::runtime::StageCommand;

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "Stage", handle_type = 7)]
struct Stage(u64);

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    crate::stage::ViewClip::register_hks(registry).expect("view clip registration");
    api::register_hks(registry).expect("stage API registration");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::story_runtime::{StoryRuntime, StoryRuntimeEvent};

    #[test]
    fn named_light_api_is_typed_and_emits_ordinary_spatial_effects() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            room.setLightColor("alice", 1, 0.4, 0.2)
            room.resetLightColor("alice")
        "#,
        )
        .expect("typed light API");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("open")
        else {
            panic!("stage load")
        };
        runtime.complete_task_effect(task, &effect).expect("loaded");
        let Some(StoryRuntimeEvent::Effect(StoryEffect::Spatial(StageCommand::LightColor {
            name,
            color,
            ..
        }))) = runtime.step().expect("color")
        else {
            panic!("light override")
        };
        assert_eq!(name, "alice");
        assert_eq!(color, Some([1.0, 0.4, 0.2]));
        let Some(StoryRuntimeEvent::Effect(StoryEffect::Spatial(StageCommand::LightColor {
            name,
            color,
            ..
        }))) = runtime.step().expect("reset")
        else {
            panic!("light reset")
        };
        assert_eq!(name, "alice");
        assert_eq!(color, None);
        assert!(compile_story_bytecode("invalid.hks", r#"let room = stage.open("room.stage.hson"); room.setLightColor("alice", "red", 0.4, 0.2)"#).is_err());
    }

    #[test]
    fn stage_sequence_finishes_opening_before_starting_loop() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            let sequence = seq {
                room.play("opening")
                room.play("idle").looped(true)
            }
            sequence.await()
            "#,
        )
        .expect("typed stage sequence");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("open")
        else {
            panic!("stage load event");
        };
        runtime.complete_task_effect(task, &effect).expect("loaded");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("opening")
        else {
            panic!("opening event");
        };
        assert!(matches!(&effect, StoryEffect::Spatial(StageCommand::Play {
            name, looping: false, ..
        }) if name == "opening"));
        assert!(runtime.step().expect("opening is still playing").is_none());
        runtime.complete_task_effect(task, &effect).expect("opened");
        let Some(StoryRuntimeEvent::TaskEffect { effect, .. }) = runtime.step().expect("idle")
        else {
            panic!("idle event");
        };
        assert!(matches!(&effect, StoryEffect::Spatial(StageCommand::Play {
            name, looping: true, ..
        }) if name == "idle"));
    }

    #[test]
    fn stage_animation_is_typed_and_waits_for_its_own_completion() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            room.play("opening").await()
            room.play("idle").looped(true)
        "#,
        )
        .expect("typed model animation API");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("open")
        else {
            panic!("stage load event");
        };
        runtime.complete_task_effect(task, &effect).expect("loaded");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) =
            runtime.step().expect("animation")
        else {
            panic!("stage animation event");
        };
        assert!(matches!(&effect, StoryEffect::Spatial(StageCommand::Play {
            name, looping: false, playback, ..
        }) if name == "opening" && *playback != 0));
        assert!(runtime.step().expect("waiting").is_none());
        runtime
            .complete_task_effect(task, &effect)
            .expect("opening completed");
        let Some(StoryRuntimeEvent::Effect(effect)) = runtime.step().expect("idle") else {
            panic!("looping stage animation event");
        };
        assert!(matches!(&effect, StoryEffect::Spatial(StageCommand::Play {
            name, looping: true, ..
        }) if name == "idle"));
    }

    #[test]
    fn camera_shake_uses_the_common_animation_handle_and_host_wait() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            room.shakeCamera(0.4, -0.2, 0).frequency(50).damping(1).randomized(false).track("main").time(3).await()
        "#,
        )
        .expect("typed camera shake");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("open")
        else {
            panic!("stage load")
        };
        runtime.complete_task_effect(task, &effect).expect("loaded");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) = runtime.step().expect("shake")
        else {
            panic!("shake effect")
        };
        assert!(
            matches!(&effect, StoryEffect::Spatial(StageCommand::Shake { randomized: false, force, frequency: 50, damping, motion, animation, .. })
            if *force == bevy::prelude::Vec3::new(0.4, -0.2, 0.0) && *damping == 1.0 && *motion > 0 && animation.duration() == 3.0)
        );
        assert!(runtime.step().expect("waiting").is_none());
        runtime
            .complete_task_effect(task, &effect)
            .expect("finished shake");
        assert!(matches!(
            runtime.step().expect("finished"),
            Some(StoryRuntimeEvent::Completed(_))
        ));
    }

    #[test]
    fn stage_open_waits_for_host_before_anchoring_or_camera_commands() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            room.place(char("alice"), "desk")
            room.camera("closeup").time(0.5).easing(.easeOut).await()
            room.close()
        "#,
        )
        .expect("typed spatial API");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) =
            runtime.step().expect("open event")
        else {
            panic!("stage loading task");
        };
        assert!(
            matches!(&effect, StoryEffect::Spatial(StageCommand::Open { path, .. }) if path == "room.stage.hson")
        );
        assert!(runtime.step().expect("loading blocks").is_none());
        runtime.complete_task_effect(task, &effect).expect("loaded");
        let Some(StoryRuntimeEvent::TaskEffect { effect, .. }) =
            runtime.step().expect("place event")
        else {
            panic!("placement task");
        };
        assert!(
            matches!(effect, StoryEffect::Spatial(StageCommand::Place { actor, anchor, .. }) if actor == "alice" && anchor == "desk")
        );
    }
}

#[hiraku_script::hks_module]
mod api {
    use super::*;

    #[hks(name = "setEffect", selector = "Stage", receiver)]
    fn set_effect(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
        enabled: bool,
    ) -> Result<(), NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("stage effect name must not be empty"));
        }
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::Effect {
                id: stage.0,
                name,
                enabled,
            }));
        Ok(())
    }

    #[hks(name = "setLightColor", selector = "Stage", receiver)]
    fn set_light_color(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
        red: f64,
        green: f64,
        blue: f64,
    ) -> Result<(), NativeError> {
        let color = [red as f32, green as f32, blue as f32];
        if name.trim().is_empty()
            || !color
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            return Err(NativeError::message(
                "stage light color requires a name and RGB values in 0..1",
            ));
        }
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::LightColor {
                id: stage.0,
                name,
                color: Some(color),
            }));
        Ok(())
    }

    #[hks(name = "resetLightColor", selector = "Stage", receiver)]
    fn reset_light_color(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
    ) -> Result<(), NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("stage light name must not be empty"));
        }
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::LightColor {
                id: stage.0,
                name,
                color: None,
            }));
        Ok(())
    }

    #[hks(name = "play", selector = "Stage", receiver)]
    fn play(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
    ) -> Result<StageAnimationHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message(
                "stage animation name must not be empty",
            ));
        }
        let handle: StageAnimationHandle =
            context
                .scene_visuals
                .begin_as(SceneVisualTarget::Spatial(StageCommand::Play {
                    id: stage.0,
                    playback: 0,
                    name,
                    looping: false,
                }))?;
        if let Some((SceneVisualTarget::Spatial(StageCommand::Play { playback, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        {
            *playback = handle.0;
        }
        Ok(handle)
    }

    #[hks(name = "looped", selector = "StageAnimation", receiver)]
    fn looped(
        context: &mut CharacterContext,
        handle: StageAnimationHandle,
        value: bool,
    ) -> Result<StageAnimationHandle, NativeError> {
        let Some((SceneVisualTarget::Spatial(StageCommand::Play { looping, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "looped requires an uncommitted stage animation",
            ));
        };
        *looping = value;
        Ok(handle)
    }

    #[hks(name = "clipView", selector = "Stage", receiver)]
    fn clip_view(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
        clip: crate::stage::ViewClip,
    ) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::Clip {
                id: stage.0,
                view: name,
                clip,
            }));
        Ok(())
    }

    #[hks(name = "open", selector = "stage")]
    fn open(context: &mut CharacterContext, path: String) -> Result<Stage, NativeError> {
        if path.trim().is_empty() {
            return Err(NativeError::message("stage path must not be empty"));
        }
        let handle =
            context
                .scene_visuals
                .begin(SceneVisualTarget::Spatial(StageCommand::Open {
                    path,
                    id: 0,
                }))?;
        if let Some((SceneVisualTarget::Spatial(StageCommand::Open { id, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        {
            *id = handle.0;
        }
        context.await_effects = true;
        Ok(Stage(handle.0))
    }

    #[hks(name = "close", selector = "Stage", receiver)]
    fn close(context: &mut CharacterContext, stage: Stage) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::Close { id: stage.0 }));
        Ok(())
    }

    #[hks(name = "camera", selector = "Stage", receiver)]
    fn camera(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Spatial(StageCommand::Camera {
                id: stage.0,
                view: "main".into(),
                name,
                animation: crate::script::animation::AnimationSpec::Linear(0.0, false),
            }))
    }

    #[hks(name = "shakeCamera", selector = "Stage", receiver)]
    fn shake_camera(
        context: &mut CharacterContext,
        stage: Stage,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        let force = bevy::prelude::Vec3::new(x as f32, y as f32, z as f32);
        if !force.is_finite() {
            return Err(NativeError::message("camera shake force must be finite"));
        }
        let handle: StageCameraTransitionHandle =
            context
                .scene_visuals
                .begin_as(SceneVisualTarget::Spatial(StageCommand::Shake {
                    randomized: true,
                    id: stage.0,
                    view: "main".into(),
                    motion: 1,
                    force,
                    frequency: 10,
                    damping: 1.0,
                    seed: 0,
                    animation: crate::script::AnimationSpec::Linear(0.5, false),
                }))?;
        if let Some((SceneVisualTarget::Spatial(StageCommand::Shake { motion, seed, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        {
            *motion = handle.0;
            *seed = handle.0 as u32;
        }
        Ok(handle)
    }

    #[hks(name = "randomized", selector = "StageCameraTransition", receiver)]
    fn randomized(
        context: &mut CharacterContext,
        handle: StageCameraTransitionHandle,
        enabled: bool,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        let Some((SceneVisualTarget::Spatial(StageCommand::Shake { randomized, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "randomized requires an uncommitted stage camera shake",
            ));
        };
        *randomized = enabled;
        Ok(handle)
    }

    #[hks(name = "frequency", selector = "StageCameraTransition", receiver)]
    fn frequency(
        context: &mut CharacterContext,
        handle: StageCameraTransitionHandle,
        value: i64,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        let value = u32::try_from(value)
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                NativeError::message("shake frequency must be a positive 32-bit integer")
            })?;
        let Some((SceneVisualTarget::Spatial(StageCommand::Shake { frequency, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "frequency requires an uncommitted camera shake",
            ));
        };
        *frequency = value;
        Ok(handle)
    }

    #[hks(name = "damping", selector = "StageCameraTransition", receiver)]
    fn damping(
        context: &mut CharacterContext,
        handle: StageCameraTransitionHandle,
        value: f64,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        let value = value as f32;
        if !value.is_finite() || value < 0.0 {
            return Err(NativeError::message(
                "shake damping must be finite and non-negative",
            ));
        }
        let Some((SceneVisualTarget::Spatial(StageCommand::Shake { damping, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "damping requires an uncommitted camera shake",
            ));
        };
        *damping = value;
        Ok(handle)
    }

    #[hks(name = "track", selector = "StageCameraTransition", receiver)]
    fn track(
        context: &mut CharacterContext,
        handle: StageCameraTransitionHandle,
        name: String,
    ) -> Result<StageCameraTransitionHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("stage view name must not be empty"));
        }
        let Some((
            SceneVisualTarget::Spatial(
                StageCommand::Camera { view, .. } | StageCommand::Shake { view, .. },
            ),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "track requires an uncommitted stage camera transition",
            ));
        };
        *view = name;
        Ok(handle)
    }

    #[hks(name = "showView", selector = "Stage", receiver)]
    fn show_view(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
    ) -> Result<StageViewTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Spatial(StageCommand::View {
                id: stage.0,
                view: name,
                visible: true,
                animation: crate::script::AnimationSpec::Linear(0.0, false),
            }))
    }

    #[hks(name = "hideView", selector = "Stage", receiver)]
    fn hide_view(
        context: &mut CharacterContext,
        stage: Stage,
        name: String,
    ) -> Result<StageViewTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Spatial(StageCommand::View {
                id: stage.0,
                view: name,
                visible: false,
                animation: crate::script::AnimationSpec::Linear(0.0, false),
            }))
    }

    #[hks(name = "intrinsics.engine.placeActor")]
    fn place(
        context: &mut CharacterContext,
        stage: Stage,
        actor: ActorIdentity,
        anchor: String,
    ) -> Result<(), NativeError> {
        let actor = context
            .actor_mut(actor.0)
            .map_err(|error| NativeError::message(error.to_string()))?
            .display_instance
            .clone();
        context
            .commands
            .push(StoryEffect::Spatial(StageCommand::Place {
                id: stage.0,
                actor,
                anchor,
            }));
        // Resolve anchor errors before allowing the next statement to continue.
        context.await_effects = true;
        Ok(())
    }
}
