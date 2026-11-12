//! Engine-owned virtual scene time. Never pauses the embedding application's clock.
use bevy::prelude::*;

#[derive(Default, Debug)]
pub struct SceneClock {
    pub paused: bool,
}

#[derive(Component)]
pub(crate) struct ScenePausePolicy(pub bool);

pub(crate) fn advance(
    time: Res<Time>,
    screens: Res<crate::ui::ScreenUiState>,
    paused_roots: Query<&super::ui_timers::UiTimersPaused>,
    policies: Query<&ScenePausePolicy>,
    dependencies: Option<Res<crate::dependencies::ScriptDependencies>>,
    mut clock: ResMut<Time<SceneClock>>,
) {
    let scoped_pause = screens
        .active_root
        .into_iter()
        .chain(screens.stack.iter().map(|(root, _)| *root))
        .any(|root| {
            policies.get(root).is_ok_and(|p| p.0) || paused_roots.get(root).is_ok_and(|p| p.0)
        });
    let paused =
        scoped_pause || screens.pending_root.is_some() || dependencies.is_some_and(|d| d.loading);
    // Advance by a frame delta, never by wall time since the last unpaused tick.
    clock.context_mut().paused = paused;
    clock.advance_by(if paused {
        std::time::Duration::ZERO
    } else {
        time.delta()
    });
}

#[derive(Component)]
pub(crate) struct ManagedVoicePause;

pub(crate) fn advance_story_timers(
    clock: Res<Time<SceneClock>>,
    mut runtime: ResMut<crate::script::ScriptRuntimeState>,
    mut models: ResMut<crate::ui::UiModels>,
    mut redraw: crate::redraw::Redraw,
) {
    if let Some(story) = runtime.story.as_mut()
        && story.advance_time(clock.delta())
    {
        redraw.request();
    }
    models.set_countdowns(
        runtime
            .story
            .as_ref()
            .map(|story| story.named_countdowns())
            .unwrap_or_default(),
    );
}

pub(crate) fn pause_voices(
    clock: Res<Time<SceneClock>>,
    shared: Res<crate::state::SceneSharedState>,
    mut commands: Commands,
    mut voices: Query<
        (
            Entity,
            Option<&bevy::audio::AudioSink>,
            &mut bevy::audio::PlaybackSettings,
            Has<ManagedVoicePause>,
        ),
        With<super::audio_runtime::VoiceChannel>,
    >,
) {
    use bevy::audio::AudioSinkPlayback;
    let paused = clock.context().paused || shared.0.voice_paused;
    for (entity, sink, mut settings, owned) in &mut voices {
        let already_paused = sink.map_or(settings.paused, AudioSinkPlayback::is_paused);
        if paused && !already_paused {
            if let Some(sink) = sink {
                sink.pause();
            }
            settings.paused = true;
            commands.entity(entity).try_insert(ManagedVoicePause);
        } else if !paused && owned {
            if let Some(sink) = sink {
                sink.play();
            }
            settings.paused = false;
            commands.entity(entity).try_remove::<ManagedVoicePause>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_and_modal_voice_pauses_do_not_release_each_other() {
        use bevy::audio::{AudioSink, AudioSinkPlayback};

        let mut app = App::new();
        app.init_resource::<Time<SceneClock>>()
            .init_resource::<crate::state::SceneSharedState>()
            .add_systems(Update, pause_voices);
        let (player, _queue) = rodio::Player::new();
        let voice = app
            .world_mut()
            .spawn((
                AudioSink::new(player),
                bevy::audio::PlaybackSettings::ONCE,
                super::super::audio_runtime::VoiceChannel {
                    path: "alice".into(),
                    volume: 1.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .voice_paused = true;
        app.update();
        assert!(
            app.world()
                .get::<AudioSink>(voice)
                .expect("voice")
                .is_paused()
        );
        assert!(app.world().get::<ManagedVoicePause>(voice).is_some());
        app.world_mut()
            .resource_mut::<Time<SceneClock>>()
            .context_mut()
            .paused = true;
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .voice_paused = false;
        app.update();
        assert!(
            app.world()
                .get::<AudioSink>(voice)
                .expect("voice")
                .is_paused()
        );
        app.world_mut()
            .resource_mut::<Time<SceneClock>>()
            .context_mut()
            .paused = false;
        app.update();
        assert!(
            !app.world()
                .get::<AudioSink>(voice)
                .expect("voice")
                .is_paused()
        );
        assert!(app.world().get::<ManagedVoicePause>(voice).is_none());

        // An unrelated host pause must not become owned by the scene clock.
        app.world().get::<AudioSink>(voice).expect("voice").pause();
        app.update();
        assert!(
            app.world()
                .get::<AudioSink>(voice)
                .expect("voice")
                .is_paused()
        );

        let (player, _queue) = rodio::Player::new();
        player.pause();
        let initially_paused = app
            .world_mut()
            .spawn((
                AudioSink::new(player),
                bevy::audio::PlaybackSettings::ONCE,
                super::super::audio_runtime::VoiceChannel {
                    path: "bob".into(),
                    volume: 1.0,
                },
                ManagedVoicePause,
            ))
            .id();
        app.update();
        assert!(
            !app.world()
                .get::<AudioSink>(initially_paused)
                .expect("new voice")
                .is_paused()
        );
    }

    #[test]
    fn a_voice_waiting_for_asset_decode_inherits_and_releases_the_pause() {
        use bevy::audio::PlaybackSettings;
        let mut app = App::new();
        app.init_resource::<Time<SceneClock>>()
            .init_resource::<crate::state::SceneSharedState>()
            .add_systems(Update, pause_voices);
        let pending = app
            .world_mut()
            .spawn((
                PlaybackSettings::ONCE,
                super::super::audio_runtime::VoiceChannel {
                    path: "alice".into(),
                    volume: 1.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .voice_paused = true;
        app.update();
        assert!(
            app.world()
                .get::<PlaybackSettings>(pending)
                .expect("pending voice")
                .paused
        );
        assert!(app.world().get::<ManagedVoicePause>(pending).is_some());
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .voice_paused = false;
        app.update();
        assert!(
            !app.world()
                .get::<PlaybackSettings>(pending)
                .expect("pending voice")
                .paused
        );
        assert!(app.world().get::<ManagedVoicePause>(pending).is_none());
    }

    #[test]
    fn story_countdown_pauses_with_modal_scene_time_and_requests_redraw_only_while_running() {
        use crate::script::{ScriptRuntimeState, StoryRuntime, StoryRuntimeEvent};
        let code = crate::script::capabilities::compile_story_bytecode(
            "alice.hks",
            r#"
            let timer = clock.countdown(5).named("alice")
            narrate("Alice")
            log(timer.remaining().toString())
        "#,
        )
        .expect("timer story");
        let mut story = StoryRuntime::new(code).expect("story");
        loop {
            match story.step().expect("step") {
                Some(StoryRuntimeEvent::Wait(_)) => break,
                Some(_) => (),
                None => panic!("expected dialogue wait"),
            }
        }
        let mut app = App::new();
        let mut runtime = ScriptRuntimeState::default();
        runtime.story = Some(story);
        app.init_resource::<Time>()
            .init_resource::<Time<SceneClock>>()
            .init_resource::<crate::ui::ScreenUiState>()
            .init_resource::<crate::ui::UiModels>()
            .insert_resource(runtime)
            .add_message::<bevy::window::RequestRedraw>()
            .add_systems(Update, (advance, advance_story_timers).chain());
        let mut cursor =
            bevy::ecs::message::MessageCursor::<bevy::window::RequestRedraw>::default();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(1));
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui::UiModels>()
                .countdowns
                .get("alice"),
            Some(&std::time::Duration::from_secs(4))
        );
        let revision = app.world().resource::<crate::ui::UiModels>().revision();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<Messages<bevy::window::RequestRedraw>>()
                )
                .count(),
            1
        );
        let modal = app.world_mut().spawn(ScenePausePolicy(true)).id();
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .active_root = Some(modal);
        for _ in 0..10 {
            app.update();
            assert_eq!(
                app.world().resource::<crate::ui::UiModels>().revision(),
                revision
            );
            assert_eq!(
                cursor
                    .read(
                        app.world()
                            .resource::<Messages<bevy::window::RequestRedraw>>()
                    )
                    .count(),
                0
            );
        }
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .active_root = None;
        app.update();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<Messages<bevy::window::RequestRedraw>>()
                )
                .count(),
            1
        );
        let mut runtime = app.world_mut().resource_mut::<ScriptRuntimeState>();
        let story = runtime.story.as_mut().expect("live story");
        story.resume(hiraku_script::Value::Unit).expect("advance");
        loop {
            match story.step().expect("step") {
                Some(StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::Log(
                    value,
                ))) => {
                    assert_eq!(value.parse::<f64>().expect("remaining"), 3.0);
                    break;
                }
                Some(_) => (),
                None => panic!("expected remaining time"),
            }
        }
        runtime.story = None;
        drop(runtime);
        app.update();
        assert!(
            app.world()
                .resource::<crate::ui::UiModels>()
                .countdowns
                .is_empty()
        );
    }
    #[test]
    fn nested_screen_pauses_without_catch_up() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<SceneClock>>()
            .init_resource::<crate::ui::ScreenUiState>()
            .add_systems(Update, advance);
        let root = app.world_mut().spawn_empty().id();
        let modal = app.world_mut().spawn(ScenePausePolicy(true)).id();
        let delta = std::time::Duration::from_millis(16);
        app.world_mut().resource_mut::<Time>().advance_by(delta);
        app.update();
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .stack
            .push((root, None));
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .active_root = Some(modal);
        for _ in 0..100 {
            app.update();
        }
        assert_eq!(app.world().resource::<Time<SceneClock>>().elapsed(), delta);
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .stack
            .clear();
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .active_root = Some(root);
        app.update();
        let clock = app.world().resource::<Time<SceneClock>>();
        assert_eq!(clock.delta(), delta);
        assert_eq!(clock.elapsed(), delta * 2);
    }

    #[test]
    fn pause_scope_survives_a_child_screen_without_pausing_host_time() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<SceneClock>>()
            .init_resource::<crate::ui::ScreenUiState>()
            .add_systems(Update, advance);
        let owner = app.world_mut().spawn(ScenePausePolicy(true)).id();
        let child = app.world_mut().spawn(ScenePausePolicy(false)).id();
        {
            let mut screens = app.world_mut().resource_mut::<crate::ui::ScreenUiState>();
            screens.stack.push((owner, None));
            screens.active_root = Some(child);
        }
        let delta = std::time::Duration::from_millis(16);
        app.world_mut().resource_mut::<Time>().advance_by(delta);
        app.update();
        assert!(app.world().resource::<Time<SceneClock>>().context().paused);
        assert_eq!(app.world().resource::<Time>().delta(), delta);
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .stack
            .clear();
        app.update();
        assert_eq!(app.world().resource::<Time<SceneClock>>().delta(), delta);
    }
}
