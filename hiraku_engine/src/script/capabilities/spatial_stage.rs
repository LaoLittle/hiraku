use super::scene_visuals::{SceneTransitionHandle, SceneVisualTarget};
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
    fn stage_open_waits_for_host_before_anchoring_or_camera_commands() {
        let code = compile_story_bytecode(
            "stage.hks",
            r#"
            let room = stage.open("room.stage.hson")
            room.place(char("alice"), "desk")
            room.camera("closeup").animation(.easeOut(0.5)).await()
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin(SceneVisualTarget::Spatial(StageCommand::Camera {
                id: stage.0,
                view: "main".into(),
                name,
                animation: crate::script::animation::AnimationSpec::Linear(0.0, false),
            }))
    }

    #[hks(name = "track", selector = "SceneTransition", receiver)]
    fn track(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        name: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("stage view name must not be empty"));
        }
        let Some((SceneVisualTarget::Spatial(StageCommand::Camera { view, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin(SceneVisualTarget::Spatial(StageCommand::View {
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin(SceneVisualTarget::Spatial(StageCommand::View {
                id: stage.0,
                view: name,
                visible: false,
                animation: crate::script::AnimationSpec::Linear(0.0, false),
            }))
    }

    #[hks(name = "place", selector = "Stage", receiver)]
    fn place(
        context: &mut CharacterContext,
        stage: Stage,
        actor: ActorHandle,
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

    #[hks(name = "time", selector = "SceneTransition", receiver)]
    fn time(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        seconds: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !seconds.is_finite() || seconds < 0.0 || seconds > f32::MAX as f64 {
            return Err(NativeError::message("invalid stage camera duration"));
        }
        let Some((SceneVisualTarget::Spatial(StageCommand::Camera { animation, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "time requires an uncommitted stage camera transition",
            ));
        };
        *animation = crate::script::animation::AnimationSpec::Linear(seconds, false);
        Ok(handle)
    }
}
