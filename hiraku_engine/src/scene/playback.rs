//! Story pacing is independent of UI time, audio pitch and host input devices.
use super::*;
use bevy::ecs::system::SystemParam;
use std::time::Duration;

#[derive(Resource, Default)]
pub(crate) struct FastForward {
    pub active: bool,
}

/// Finite story animations share one pacing policy. Standalone systems/tests
/// without the playback resource retain normal time.
#[derive(SystemParam)]
pub struct StoryTime<'w> {
    time: Res<'w, Time>,
    clock: Option<Res<'w, Time<super::clock::SceneClock>>>,
    pacing: Option<Res<'w, FastForward>>,
}

impl StoryTime<'_> {
    pub fn delta(&self) -> Duration {
        self.clock
            .as_ref()
            .map_or(self.time.delta(), |clock| clock.delta())
            .mul_f32(if self.pacing.as_ref().is_some_and(|p| p.active) {
                32.0
            } else {
                1.0
            })
    }
    pub fn delta_secs(&self) -> f32 {
        self.delta().as_secs_f32()
    }
}

pub(crate) fn update_fast_forward(
    mut input: MessageReader<crate::input::HirakuActionInput>,
    mut dialogue: ResMut<DialogueState>,
    mut pacing: ResMut<FastForward>,
    choice: Res<ChoiceState>,
    screens: Res<ScreenUiState>,
    movies: Res<PendingMovieWaits>,
    dependencies: Res<crate::dependencies::ScriptDependencies>,
    focus: Res<crate::input::HirakuTextFocus>,
    mut redraw: crate::redraw::Redraw,
) {
    use crate::input::HirakuAction;
    for event in input.read() {
        match event.0 {
            HirakuAction::FastForwardHeld(held) => dialogue.fast_forward_held = held,
            HirakuAction::ToggleFastForward => {
                dialogue.fast_forward_enabled = !dialogue.fast_forward_enabled
            }
            HirakuAction::NextDialogue | HirakuAction::Back => {
                dialogue.fast_forward_enabled = false;
                dialogue.fast_forward_held = false;
            }
            _ => {}
        }
    }
    let stop = choice.waiting.is_some()
        || screens.active_root.is_some()
        || screens.pending_root.is_some()
        || movies.is_waiting()
        || focus.0.is_some();
    if stop {
        dialogue.fast_forward_enabled = false;
        dialogue.fast_forward_held = false;
    }
    pacing.active = !dependencies.loading
        && !stop
        && (dialogue.fast_forward_enabled || dialogue.fast_forward_held);
    if pacing.active {
        dialogue.auto_enabled = false;
        redraw.request();
    } else {
        dialogue.fast_forward_elapsed = 0.0;
    }
}

/// Use normal completion paths so seq/par and .await() receive their tokens.
pub(crate) fn skip_voices(
    pacing: Res<FastForward>,
    mut commands: Commands,
    mut voices: ResMut<VoiceState>,
    mut animations: ResMut<AnimationState>,
    sounds: Query<(Entity, &super::audio_runtime::SfxCompletion)>,
) {
    if pacing.active {
        super::audio_runtime::finish_all_voices(&mut commands, &mut animations, &mut voices);
        for (entity, completion) in &sounds {
            if let Some(id) = &completion.animation_id {
                animations.completed.insert(id.clone());
                commands.entity(entity).try_despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{HirakuAction, HirakuActionInput};

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<DialogueState>()
            .init_resource::<FastForward>()
            .init_resource::<ChoiceState>()
            .init_resource::<ScreenUiState>()
            .init_resource::<PendingMovieWaits>()
            .init_resource::<crate::dependencies::ScriptDependencies>()
            .init_resource::<crate::input::HirakuTextFocus>()
            .add_message::<HirakuActionInput>()
            .insert_resource(FrontendState {
                startup_script: "startup.hks".into(),
                notice: None,
                runtime_started: false,
            })
            .configure_sets(
                PreUpdate,
                crate::HirakuRuntimeSystems.run_if(crate::runtime_initialized),
            )
            .add_systems(
                PreUpdate,
                update_fast_forward.in_set(crate::HirakuRuntimeSystems),
            );
        app
    }

    #[test]
    fn pacing_waits_for_stage_initialization() {
        let mut app = app();
        let world = app.world_mut();
        let frontend = world
            .remove_resource::<FrontendState>()
            .expect("frontend fixture");
        let dialogue = world
            .remove_resource::<DialogueState>()
            .expect("dialogue fixture");
        let choice = world
            .remove_resource::<ChoiceState>()
            .expect("choice fixture");
        let screens = world
            .remove_resource::<ScreenUiState>()
            .expect("screen fixture");

        // Asset loading can take multiple frames; stage resources do not exist yet.
        app.update();
        app.update();
        assert!(!app.world().resource::<FastForward>().active);

        let world = app.world_mut();
        world.insert_resource(dialogue);
        world.insert_resource(choice);
        world.insert_resource(screens);
        world.insert_resource(frontend);
        world.write_message(HirakuActionInput(HirakuAction::ToggleFastForward));
        app.update();
        assert!(app.world().resource::<FastForward>().active);
    }

    #[test]
    fn modal_and_choice_stop_fast_forward_until_explicitly_reenabled() {
        let mut app = app();
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::FastForwardHeld(true)));
        app.update();
        assert!(app.world().resource::<FastForward>().active);
        app.world_mut().resource_mut::<ChoiceState>().waiting = Some(ScriptRequestId(1));
        app.update();
        assert!(!app.world().resource::<FastForward>().active);
        app.world_mut().resource_mut::<ChoiceState>().waiting = None;
        app.update();
        assert!(!app.world().resource::<FastForward>().active);
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::ToggleFastForward));
        app.update();
        assert!(app.world().resource::<FastForward>().active);
        let modal = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(modal);
        app.update();
        assert!(!app.world().resource::<FastForward>().active);
        assert!(!app.world().resource::<DialogueState>().fast_forward_enabled);
    }

    #[test]
    fn loading_pauses_pacing_and_release_during_loading_is_not_lost() {
        let mut app = app();
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::FastForwardHeld(true)));
        app.update();
        app.world_mut()
            .resource_mut::<crate::dependencies::ScriptDependencies>()
            .loading = true;
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::FastForwardHeld(false)));
        app.update();
        app.world_mut()
            .resource_mut::<crate::dependencies::ScriptDependencies>()
            .loading = false;
        app.update();
        assert!(!app.world().resource::<FastForward>().active);
    }

    #[test]
    fn story_clock_accelerates_without_changing_application_clock() {
        #[derive(Resource, Default)]
        struct Sample(f32);
        fn sample(time: StoryTime, mut sample: ResMut<Sample>) {
            sample.0 = time.delta_secs();
        }
        let mut app = app();
        app.init_resource::<Sample>().add_systems(Update, sample);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(10));
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::ToggleFastForward));
        app.update();
        assert!((app.world().resource::<Sample>().0 - 0.32).abs() < 0.0001);
        assert_eq!(
            app.world().resource::<Time>().delta(),
            Duration::from_millis(10)
        );
        app.world_mut()
            .write_message(HirakuActionInput(HirakuAction::NextDialogue));
        app.update();
        assert!((app.world().resource::<Sample>().0 - 0.01).abs() < 0.0001);
    }

    #[test]
    fn voice_completion_releases_sequential_and_parallel_waits() {
        let mut app = App::new();
        app.insert_resource(FastForward { active: true })
            .init_resource::<VoiceState>()
            .init_resource::<AnimationState>()
            .add_systems(Update, skip_voices);
        let alice = app.world_mut().spawn_empty().id();
        let bob = app.world_mut().spawn_empty().id();
        {
            let mut voices = app.world_mut().resource_mut::<VoiceState>();
            voices.active = Some(ActiveVoice {
                entity: alice,
                animation_id: Some("alice".into()),
            });
            voices.concurrent.insert(
                bob,
                ActiveVoice {
                    entity: bob,
                    animation_id: Some("bob".into()),
                },
            );
        }
        app.update();
        assert!(app.world().get_entity(alice).is_err());
        assert!(app.world().get_entity(bob).is_err());
        let animations = app.world().resource::<AnimationState>();
        assert!(animations.completed.contains("alice"));
        assert!(animations.completed.contains("bob"));
        app.update(); // Finishing twice must not submit duplicate responses.
        assert_eq!(app.world().resource::<AnimationState>().completed.len(), 2);
    }
}
