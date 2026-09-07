//! Statement-scoped scene presentation builders, unrelated to anchored 3D stages.
use std::{collections::BTreeMap, time::Duration};

use hiraku_script::native::{NativeError, NativeRegistry};
use serde::{Deserialize, Serialize};

use super::{CharacterContext, StoryEffect};
use crate::scene::pictures::PictureCommand;

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "SceneTransition", handle_type = 4)]
struct SceneTransitionHandle(u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum SceneVisualTarget {
    Picture(PictureCommand),
    Background(String),
    Curtain { opacity: f32, mask: Option<String>, softness: f32 },
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SceneVisualState {
    next: u64,
    pending: BTreeMap<u64, (SceneVisualTarget, Option<u64>)>,
}

impl SceneVisualState {
    fn begin(&mut self, target: SceneVisualTarget) -> Result<SceneTransitionHandle, NativeError> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("scene transition identifiers exhausted"))?;
        self.pending.insert(self.next, (target, None));
        Ok(SceneTransitionHandle(self.next))
    }

    pub(super) fn commit(&mut self, effects: &mut Vec<StoryEffect>) {
        for (_, (target, fade_ms)) in std::mem::take(&mut self.pending) {
            effects.push(match target {
                SceneVisualTarget::Picture(mut picture) => {
                    let seconds = fade_ms.unwrap_or(0) as f32 / 1000.0;
                    match &mut picture {
                        PictureCommand::Show {
                            seconds: duration, ..
                        }
                        | PictureCommand::Hide {
                            seconds: duration, ..
                        } => *duration = seconds,
                        _ => {}
                    }
                    StoryEffect::Picture(picture)
                }
                SceneVisualTarget::Background(texture) => StoryEffect::SetBackground {
                    texture,
                    fade_in_ms: fade_ms,
                },
                SceneVisualTarget::Curtain { opacity, mask, softness } => StoryEffect::SetCurtain { opacity, fade_ms, mask, softness },
            });
        }
    }
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    api::register_hks(registry)
        .expect("scene presentation API registration must be internally consistent");
}

fn milliseconds(seconds: f64) -> Result<u64, NativeError> {
    let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
        NativeError::message("duration must be finite, non-negative and representable")
    })?;
    u64::try_from(duration.as_millis())
        .map_err(|_| NativeError::message("duration exceeds the supported millisecond range"))
}

#[hiraku_script::hks_module]
mod api {
    use super::*;

    #[hks(name = "picture", selector = "scene")]
    fn picture(
        context: &mut CharacterContext,
        id: String,
        texture: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if id.trim().is_empty() || texture.trim().is_empty() {
            return Err(NativeError::message(
                "picture identity and texture must not be empty",
            ));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Show {
                id,
                path: texture,
                rect: None,
                position: [50.0, 50.0],
                scale: 1.0,
                rotation: 0.0,
                layer: 1.0,
                seconds: 0.0,
            }))
    }

    #[hks(name = "frame", receiver)]
    fn frame(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        x: f64,
        y: f64,
        scale: f64,
        rotation: f64,
        layer: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if ![x, y, scale, rotation, layer]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 100000.0)
            || scale <= 0.0
            || !(0.0..30.0).contains(&layer)
        {
            return Err(NativeError::message("invalid picture frame"));
        }
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                position,
                scale: size,
                rotation: angle,
                layer: z,
                ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&id)
        else {
            return Err(NativeError::message(
                "frame requires an uncommitted picture",
            ));
        };
        *position = [x as f32, y as f32];
        *size = scale as f32;
        *angle = rotation as f32;
        *z = layer as f32;
        Ok(SceneTransitionHandle(id))
    }

    #[hks(name = "hidePicture", selector = "scene")]
    fn hide_picture(
        context: &mut CharacterContext,
        id: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Hide {
                id,
                seconds: 0.0,
            }))
    }

    #[hks(name = "movePicture", selector = "scene")]
    fn move_picture(
        context: &mut CharacterContext,
        id: String,
        x: f64,
        y: f64,
        seconds: f64,
        ease: String,
    ) -> Result<(), NativeError> {
        milliseconds(seconds)?;
        if ![x, y].iter().all(|n| n.is_finite() && n.abs() <= 100000.0)
            || !["linear", "easeOutQuad", "easeOutBack"].contains(&ease.as_str())
        {
            return Err(NativeError::message("invalid picture movement or easing"));
        }
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::Move {
                id,
                position: [x as f32, y as f32],
                seconds: seconds as f32,
                ease,
            }));
        Ok(())
    }

    #[hks(name = "clearPictures", selector = "scene")]
    fn clear_pictures(context: &mut CharacterContext) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::Clear));
        Ok(())
    }

    #[hks(name = "animatePictureX", selector = "scene")]
    fn animate_picture_x(
        context: &mut CharacterContext,
        id: String,
        offsets: Vec<f64>,
        step_seconds: f64,
    ) -> Result<(), NativeError> {
        if offsets.is_empty()
            || offsets.len() > 4096
            || !offsets.iter().all(|n| n.is_finite() && n.abs() < 100000.0)
            || !(0.001..=60.0).contains(&step_seconds)
        {
            return Err(NativeError::message("invalid picture keyframes"));
        }
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::AnimateX {
                id,
                offsets: offsets.into_iter().map(|n| n as f32).collect(),
                step_seconds: step_seconds as f32,
            }));
        Ok(())
    }

    #[hks(name = "bg")]
    fn background(
        context: &mut CharacterContext,
        texture: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if texture.trim().is_empty() {
            return Err(NativeError::message("background texture must not be empty"));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Background(texture))
    }

    /// A scene-space blackout, below script-owned UI. Independent of background
    /// identity, so replacing a background does not reveal it before the fade.
    #[hks(name = "curtain", selector = "scene")]
    fn curtain(
        context: &mut CharacterContext,
        opacity: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(NativeError::message(
                "curtain opacity must be between 0 and 1",
            ));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Curtain { opacity: opacity as f32, mask: None, softness: 0.0 })
    }

    /// A red-channel threshold mask, sampled as linear data across the canvas.
    #[hks(name = "dissolve", receiver)]
    fn dissolve(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        texture: String,
        softness: Option<f64>,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let softness = softness.unwrap_or(0.0);
        if texture.trim().is_empty() || !(0.0..=1.0).contains(&softness) {
            return Err(NativeError::message("dissolve needs a texture and softness between 0 and 1"));
        }
        let Some((SceneVisualTarget::Curtain { mask, softness: edge, .. }, _)) = context.scene_visuals.pending.get_mut(&id) else {
            return Err(NativeError::message("dissolve requires an uncommitted scene.curtain(...)"));
        };
        *mask = Some(texture);
        *edge = softness as f32;
        Ok(SceneTransitionHandle(id))
    }

    /// Join the curtain's loading and animation before continuing the story.
    #[hks(name = "awaitCompletion", receiver)]
    fn wait_curtain(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
    ) -> Result<(), NativeError> {
        if !matches!(context.scene_visuals.pending.get(&id), Some((SceneVisualTarget::Curtain { .. }, _))) {
            return Err(NativeError::message("awaitCompletion requires an uncommitted scene.curtain(...)"));
        }
        context.wait = Some(super::super::StoryWait::Curtain);
        Ok(())
    }

    /// Milliseconds, matching Bgm.fadeIn. The builder commits at statement end.
    #[hks(name = "fade", receiver)]
    fn fade_in(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        duration_ms: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let duration = milliseconds(duration_ms / 1000.0)?;
        let pending = context.scene_visuals.pending.get_mut(&id).ok_or_else(|| {
            NativeError::message(
                "scene transition has already been committed; start a new presentation statement",
            )
        })?;
        pending.1 = (duration > 0).then_some(duration);
        Ok(SceneTransitionHandle(id))
    }

    /// Seconds. Interactive stories wait; seq waits in order; par schedules the
    /// timer without suspending its command list, and wait(handle) joins it.
    #[hks(name = "sleep")]
    fn sleep(context: &mut CharacterContext, seconds: f64) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::Delay {
            duration_ms: milliseconds(seconds)?,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::capabilities::{StoryWait, compile_story_bytecode};
    use crate::script::{StoryRuntime, StoryRuntimeEvent};
    use hiraku_script::Value;

    fn runtime(source: &str) -> StoryRuntime {
        StoryRuntime::new(compile_story_bytecode("test.hks", source).expect("script compiles"))
            .expect("runtime initializes")
    }

    fn event(runtime: &mut StoryRuntime) -> StoryRuntimeEvent {
        for _ in 0..100 {
            if let Some(event) = runtime.step().expect("runtime steps") {
                return event;
            }
        }
        panic!("expected a runtime event");
    }

    #[test]
    fn validates_duration_without_panicking() {
        assert_eq!(milliseconds(1.8).expect("valid duration"), 1800);
        for value in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(milliseconds(value).is_err());
        }
    }

    #[test]
    fn actor_identity_is_silent_and_visible_actors_retain_their_state() {
        let mut runtime=runtime(r#"
            global let alice = char("alice")
            alice.at(.pos(120, 80)).scale(0.5).e("happy")
            alice: "Not on stage yet"
            alice.show()
            char("alice").e("sad")
        "#);
        assert!(matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::Say { speaker, .. }) if speaker == "alice"));
        assert!(matches!(event(&mut runtime),StoryRuntimeEvent::Wait(_)));
        runtime.resume(Value::Unit).expect("advance dialogue");
        assert!(matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { position:[120.0,80.0],scale, .. }) if scale==0.5));
        assert!(matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { position:[120.0,80.0],scale,expressions,.. }) if scale==0.5 && expressions==["happy","sad"]));
    }

    #[test]
    fn hiding_accepts_a_fade_without_flushing_a_new_actor() {
        let mut runtime = runtime("char(\"alice\").hide(300)\nscene.hideCharacters(600)");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: Some("alice".into()),
                fade_ms: 300
            })
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: None,
                fade_ms: 600
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn picture_and_character_visibility_are_native_capabilities() {
        let mut runtime = runtime(
            "scene.picture(\"room\", \"alice/background\").frame(80, -35, 3, -10, 5).fade(300)\nchar(\"alice\").hide()\nscene.hideCharacters()",
        );
        assert!(
            matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show { position:[80.0,-35.0],seconds, .. })) if (seconds-0.3).abs()<0.001)
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: Some("alice".into()),
                fade_ms: 0
            })
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: None,
                fade_ms: 0
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn curtain_uses_a_scene_selector_and_commits_a_single_transition() {
        let mut runtime = runtime("scene.curtain(0).fade(1200)");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetCurtain {
                opacity: 0.0,
                fade_ms: Some(1200),
                mask: None,
                softness: 0.0,
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn curtain_dissolve_is_data_driven_and_statement_scoped() {
        let mut runtime = runtime("scene.curtain(1).dissolve(\"transitions/blinds\", 0.1).fade(900)");
        assert_eq!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::SetCurtain {
            opacity: 1.0, fade_ms: Some(900), mask: Some("transitions/blinds".into()), softness: 0.1,
        }));
        assert!(matches!(event(&mut runtime), StoryRuntimeEvent::Completed(_)));
    }

    #[test]
    fn curtain_wait_yields_after_commit_and_resumes_once() {
        let source = "scene.curtain(1).dissolve(\"transitions/blinds\").fade(900).awaitCompletion()";
        let mut runtime = runtime(source);
        assert!(matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::SetCurtain { .. })));
        assert_eq!(event(&mut runtime), StoryRuntimeEvent::Wait(StoryWait::Curtain));
        let snapshot = runtime.snapshot().expect("curtain boundary can be saved");
        let mut runtime = StoryRuntime::restore(compile_story_bytecode("test.hks", source).expect("deterministic recompile"), snapshot)
            .expect("curtain wait restores");
        assert_eq!(runtime.restored_boundary_event(), Some(StoryRuntimeEvent::Wait(StoryWait::Curtain)));
        runtime.resume(Value::Unit).expect("curtain completion resumes host wait");
        assert!(matches!(event(&mut runtime), StoryRuntimeEvent::Completed(_)));
    }

    #[test]
    fn background_builder_commits_once_and_delay_restores_at_host_boundary() {
        let source = "bg(\"alice/background\").fade(1200)\nsleep(1.8)\n\"after\"";
        let mut runtime = runtime(source);
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetBackground {
                texture: "alice/background".into(),
                fade_in_ms: Some(1200),
            })
        );
        let wait = StoryRuntimeEvent::Wait(StoryWait::Delay { duration_ms: 1800 });
        assert_eq!(event(&mut runtime), wait);
        assert!(runtime.step().expect("wait stays idle").is_none());
        let snapshot = runtime.snapshot().expect("snapshot at wait");
        let bytecode = compile_story_bytecode("test.hks", source).expect("same script compiles");
        let mut runtime = StoryRuntime::restore(bytecode, snapshot).expect("restore");
        assert_eq!(runtime.restored_boundary_event(), Some(wait));
        runtime.resume(Value::Unit).expect("timer completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Say { text, .. }) if text == "after")
        );
    }

    #[test]
    fn sequence_delays_suspend_but_parallel_delays_are_joined() {
        for mode in ["seq", "par"] {
            let mut runtime = runtime(&format!(
                "let h = {mode} {{ sleep(0.1); sleep(0.2) }}\nwait(h)\nlog(\"done\")"
            ));
            let StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::Delay { duration_ms: 100 },
            } = event(&mut runtime)
            else {
                panic!("first delay");
            };
            if mode == "seq" {
                assert!(runtime.step().expect("sequence waits").is_none());
                runtime.resume_task(task).expect("first timer completes");
            }
            assert!(matches!(
                event(&mut runtime),
                StoryRuntimeEvent::TaskEffect {
                    effect: StoryEffect::Delay { duration_ms: 200 },
                    ..
                }
            ));
            assert!(runtime.step().expect("join waits").is_none());
            if mode == "par" {
                runtime
                    .resume_task(task)
                    .expect("first parallel timer completes");
            }
            runtime.resume_task(task).expect("second timer completes");
            assert_eq!(
                event(&mut runtime),
                StoryRuntimeEvent::Effect(StoryEffect::Log("done".into()))
            );
        }
    }

    #[test]
    fn delay_in_a_choice_branch_waits_for_its_timer() {
        let mut runtime = runtime("choice { option(\"A\") { sleep(0.1); log(\"after\") } }");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Choice { .. }
        ));
        runtime.resume(Value::Number(0.0)).expect("select option");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Wait(StoryWait::Delay { duration_ms: 100 })
        );
        assert!(runtime.step().expect("branch waits").is_none());
        runtime.resume(Value::Unit).expect("timer completes");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Log("after".into()))
        );
    }
}
