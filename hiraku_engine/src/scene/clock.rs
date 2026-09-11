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
pub(crate) struct ClockPausedVoice;

pub(crate) fn pause_voices(
    clock: Res<Time<SceneClock>>,
    mut commands: Commands,
    voices: Query<
        (Entity, &bevy::audio::AudioSink, Has<ClockPausedVoice>),
        With<super::audio_runtime::VoiceChannel>,
    >,
) {
    use bevy::audio::AudioSinkPlayback;
    for (entity, sink, owned) in &voices {
        if clock.context().paused && !sink.is_paused() {
            sink.pause();
            commands.entity(entity).try_insert(ClockPausedVoice);
        } else if !clock.context().paused && owned {
            sink.play();
            commands.entity(entity).try_remove::<ClockPausedVoice>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
