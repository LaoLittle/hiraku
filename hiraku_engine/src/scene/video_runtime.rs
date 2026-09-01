use std::collections::BTreeMap;

use bevy::prelude::*;
use hiraku_video::{VideoAsset, VideoEvent, VideoPlaybackId, VideoPlayer};

use crate::script::{ScriptRequestId, ScriptResponse, ScriptResponseMessage, VideoCommand};

#[derive(Resource, Default)]
pub struct PendingMovieWaits(BTreeMap<VideoPlaybackId, ScriptRequestId>);

pub(super) fn dispatch_video_command(
    command: VideoCommand,
    asset_server: &AssetServer,
    player: &mut VideoPlayer,
    waits: &mut PendingMovieWaits,
) {
    match command {
        VideoCommand::Play { path, done } => {
            let asset: Handle<VideoAsset> = asset_server.load(path);
            let playback = player.play(asset);
            waits.0.insert(playback, done);
        }
    }
}

pub fn complete_movie_waits(
    mut events: MessageReader<VideoEvent>,
    mut waits: ResMut<PendingMovieWaits>,
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
        if let Some(request) = waits.0.remove(&playback) {
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
    fn terminal_video_events_resume_only_the_matching_story_wait() {
        let mut app = App::new();
        app.init_resource::<PendingMovieWaits>()
            .add_message::<VideoEvent>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, complete_movie_waits);
        let playback = VideoPlaybackId(7);
        app.world_mut()
            .resource_mut::<PendingMovieWaits>()
            .0
            .insert(playback, ScriptRequestId(11));
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
}
