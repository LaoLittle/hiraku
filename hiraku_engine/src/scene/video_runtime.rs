use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use hiraku_video::{VideoAsset, VideoEvent, VideoPlaybackId, VideoPlayer};

use crate::script::{
    ScriptRequestId, ScriptResponse, ScriptResponseMessage, ScriptRuntimeState, VideoCommand,
};

#[derive(Resource, Default)]
pub struct PendingMovieWaits {
    requests: BTreeMap<VideoPlaybackId, ScriptRequestId>,
    playbacks: BTreeSet<VideoPlaybackId>,
}

impl PendingMovieWaits {
    pub(crate) fn is_waiting(&self) -> bool {
        !self.requests.is_empty()
    }
}

pub(super) fn dispatch_video_command(
    command: VideoCommand,
    asset_server: &AssetServer,
    player: &mut VideoPlayer,
    waits: &mut PendingMovieWaits,
) {
    match command {
        VideoCommand::Play {
            path,
            layout,
            done,
            fade_out,
        } => {
            let asset: Handle<VideoAsset> = asset_server
                .load_builder()
                .with_settings(move |settings: &mut hiraku_video::VideoLoaderSettings| {
                    settings.layout = layout
                })
                .load(path);
            let playback = if done.is_some() {
                player.play(asset)
            } else {
                player.play_under_ui(asset)
            };
            let configured = player.set_fade_out(playback, fade_out);
            waits.playbacks.insert(playback);
            debug_assert!(
                configured,
                "newly queued movie accepts playback configuration"
            );
            if let Some(done) = done {
                waits.requests.insert(playback, done);
            }
        }
        VideoCommand::Stop => {
            for &id in &waits.playbacks {
                player.skip(id);
            }
        }
    }
}

/// Only engine-owned playback follows scene time. Videos hosted by the outer
/// application keep their own clock and manual pause policy.
pub(crate) fn sync_movie_clock(
    clock: Res<Time<super::clock::SceneClock>>,
    waits: Res<PendingMovieWaits>,
    mut player: ResMut<VideoPlayer>,
) {
    for &id in &waits.playbacks {
        player.set_suspended(id, clock.context().paused);
    }
}

pub fn complete_movie_waits(
    mut events: MessageReader<VideoEvent>,
    mut waits: ResMut<PendingMovieWaits>,
    runtime: Res<ScriptRuntimeState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    for event in events.read() {
        let playback = match event {
            VideoEvent::Started { .. } => continue,
            VideoEvent::Finished { id } | VideoEvent::Skipped { id } => *id,
            VideoEvent::Failed { id, error } => {
                warn!("movie playback failed: {error}");
                *id
            }
        };
        waits.playbacks.remove(&playback);
        if let Some(request) = waits.requests.remove(&playback)
            && runtime.wait_request == Some(request)
        {
            responses.write(ScriptResponseMessage {
                request,
                response: ScriptResponse::Continue,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_clock_suspends_only_engine_movies_and_terminal_events_release_ownership() {
        let mut app = App::new();
        app.init_resource::<PendingMovieWaits>()
            .init_resource::<ScriptRuntimeState>()
            .init_resource::<VideoPlayer>()
            .init_resource::<Time<super::super::clock::SceneClock>>()
            .add_message::<VideoEvent>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, (sync_movie_clock, complete_movie_waits).chain());
        let id = app
            .world_mut()
            .resource_mut::<VideoPlayer>()
            .play(Handle::default());
        let external = app
            .world_mut()
            .resource_mut::<VideoPlayer>()
            .play(Handle::default());
        app.world_mut()
            .resource_mut::<PendingMovieWaits>()
            .playbacks
            .insert(id);
        app.world_mut()
            .resource_mut::<Time<super::super::clock::SceneClock>>()
            .context_mut()
            .paused = true;
        app.update();
        assert!(app.world().resource::<VideoPlayer>().is_suspended(id));
        assert!(!app.world().resource::<VideoPlayer>().is_suspended(external));
        assert!(
            app.world()
                .resource::<PendingMovieWaits>()
                .playbacks
                .contains(&id)
        );
        app.world_mut()
            .resource_mut::<Time<super::super::clock::SceneClock>>()
            .context_mut()
            .paused = false;
        app.update();
        assert!(!app.world().resource::<VideoPlayer>().is_suspended(id));
        app.world_mut().write_message(VideoEvent::Skipped { id });
        app.update();
        assert!(
            app.world()
                .resource::<PendingMovieWaits>()
                .playbacks
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<Messages<ScriptResponseMessage>>()
                .is_empty()
        );
    }

    #[test]
    fn background_video_completion_does_not_consume_a_dialogue_wait() {
        let mut app = App::new();
        app.init_resource::<PendingMovieWaits>()
            .init_resource::<ScriptRuntimeState>()
            .add_message::<VideoEvent>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, complete_movie_waits);
        app.world_mut()
            .resource_mut::<ScriptRuntimeState>()
            .wait_request = Some(ScriptRequestId(4));
        app.world_mut().write_message(VideoEvent::Finished {
            id: VideoPlaybackId(1),
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<Messages<ScriptResponseMessage>>()
                .len(),
            0
        );
        assert_eq!(
            app.world().resource::<ScriptRuntimeState>().wait_request,
            Some(ScriptRequestId(4))
        );
    }

    #[test]
    fn terminal_video_events_resume_only_the_matching_story_wait() {
        let mut app = App::new();
        app.init_resource::<PendingMovieWaits>()
            .init_resource::<ScriptRuntimeState>()
            .add_message::<VideoEvent>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, complete_movie_waits);
        let playback = VideoPlaybackId(7);
        app.world_mut()
            .resource_mut::<PendingMovieWaits>()
            .requests
            .insert(playback, ScriptRequestId(11));
        app.world_mut()
            .resource_mut::<ScriptRuntimeState>()
            .wait_request = Some(ScriptRequestId(11));
        app.world_mut()
            .write_message(VideoEvent::Finished { id: playback });
        app.update();
        let responses = app.world().resource::<Messages<ScriptResponseMessage>>();
        let mut cursor = responses.get_cursor();
        let response = cursor
            .read(responses)
            .next()
            .expect("completion must resume the matching story request");
        assert_eq!(response.request, ScriptRequestId(11));
    }

    #[test]
    fn terminal_video_events_discard_cancelled_story_waits() {
        let mut app = App::new();
        app.init_resource::<PendingMovieWaits>()
            .init_resource::<ScriptRuntimeState>()
            .add_message::<VideoEvent>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, complete_movie_waits);
        let playback = VideoPlaybackId(9);
        app.world_mut()
            .resource_mut::<PendingMovieWaits>()
            .requests
            .insert(playback, ScriptRequestId(12));
        app.world_mut()
            .write_message(VideoEvent::Finished { id: playback });
        app.update();

        let responses = app.world().resource::<Messages<ScriptResponseMessage>>();
        assert_eq!(responses.len(), 0);
        assert!(
            app.world()
                .resource::<PendingMovieWaits>()
                .requests
                .is_empty()
        );
    }
}
