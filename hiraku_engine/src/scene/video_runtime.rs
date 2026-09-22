use std::collections::BTreeMap;

use bevy::prelude::*;
use hiraku_video::{VideoAsset, VideoEvent, VideoPlaybackId, VideoPlayer};

use crate::script::{
    ScriptRequestId, ScriptResponse, ScriptResponseMessage, ScriptRuntimeState, VideoCommand,
};

#[derive(Resource, Default)]
pub struct PendingMovieWaits(BTreeMap<VideoPlaybackId, ScriptRequestId>);

impl PendingMovieWaits {
    pub(crate) fn is_waiting(&self) -> bool {
        !self.0.is_empty()
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
            debug_assert!(
                configured,
                "newly queued movie accepts playback configuration"
            );
            if let Some(done) = done {
                waits.0.insert(playback, done);
            }
        }
        VideoCommand::Stop => player.stop_all(),
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
        if let Some(request) = waits.0.remove(&playback)
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
            .0
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
            .0
            .insert(playback, ScriptRequestId(12));
        app.world_mut()
            .write_message(VideoEvent::Finished { id: playback });
        app.update();

        let responses = app.world().resource::<Messages<ScriptResponseMessage>>();
        assert_eq!(responses.len(), 0);
        assert!(app.world().resource::<PendingMovieWaits>().0.is_empty());
    }
}
