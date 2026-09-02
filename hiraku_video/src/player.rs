use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bevy::{
    asset::{AssetApp, LoadState, RenderAssetUsages},
    audio::{AddAudioSource, AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings},
    image::Image,
    picking::Pickable,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use crate::{
    VideoAsset, VideoAssetLoader,
    decode::{DecodeEvent, DecodedFrame, VideoAudio, drain_ready_frames, spawn_decoder},
    render::{Yuv420Material, load_internal_shader},
};

const VIDEO_Z_INDEX: i32 = 30_000;
const LAST_FRAME_HOLD: Duration = Duration::from_millis(50);
const AUDIO_SINK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VideoPlaybackId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoPlaybackState {
    Loading,
    Playing,
    Paused,
    Finished,
    Skipped,
    Failed(String),
}

#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum VideoEvent {
    Started { id: VideoPlaybackId },
    Finished { id: VideoPlaybackId },
    Skipped { id: VideoPlaybackId },
    Failed { id: VideoPlaybackId, error: String },
}

struct PendingPlayback {
    id: VideoPlaybackId,
    asset: Handle<VideoAsset>,
}

enum PlaybackControl {
    Pause(VideoPlaybackId),
    Resume(VideoPlaybackId),
    Skip(VideoPlaybackId),
}

/// Public control surface for the video plugin.
///
/// Story APIs currently only call [`play`](Self::play). Hosts can already use
/// pause/resume/skip, so adding script policies later does not require changing
/// the decoder or asset ABI.
#[derive(Resource, Default)]
pub struct VideoPlayer {
    next_id: u64,
    pending: VecDeque<PendingPlayback>,
    controls: VecDeque<PlaybackControl>,
    states: BTreeMap<VideoPlaybackId, VideoPlaybackState>,
    active: Option<VideoPlaybackId>,
}

impl VideoPlayer {
    pub fn play(&mut self, asset: Handle<VideoAsset>) -> VideoPlaybackId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("video playback identifier space must not be exhausted");
        let id = VideoPlaybackId(self.next_id);
        self.states.insert(id, VideoPlaybackState::Loading);
        self.pending.push_back(PendingPlayback { id, asset });
        id
    }

    pub fn pause(&mut self, id: VideoPlaybackId) {
        self.controls.push_back(PlaybackControl::Pause(id));
    }

    pub fn resume(&mut self, id: VideoPlaybackId) {
        self.controls.push_back(PlaybackControl::Resume(id));
    }

    pub fn skip(&mut self, id: VideoPlaybackId) {
        self.controls.push_back(PlaybackControl::Skip(id));
    }

    pub fn pause_active(&mut self) {
        if let Some(id) = self.active {
            self.pause(id);
        }
    }

    pub fn resume_active(&mut self) {
        if let Some(id) = self.active {
            self.resume(id);
        }
    }

    pub fn skip_active(&mut self) {
        if let Some(id) = self.active {
            self.skip(id);
        }
    }

    pub fn active(&self) -> Option<VideoPlaybackId> {
        self.active
    }

    pub fn state(&self, id: VideoPlaybackId) -> Option<&VideoPlaybackState> {
        self.states.get(&id)
    }
}

#[derive(Resource, Default)]
struct ActiveVideo(Option<ActivePlayback>);

struct ActivePlayback {
    id: VideoPlaybackId,
    receiver: crossbeam_channel::Receiver<DecodeEvent>,
    frames: VecDeque<DecodedFrame>,
    surface: Option<VideoSurface>,
    root: Entity,
    audio_entity: Entity,
    position: Duration,
    paused: bool,
    started: bool,
    decoder_ended: bool,
    last_timestamp: Duration,
    cancellation: Arc<AtomicBool>,
    age: Duration,
}

struct VideoSurface {
    y_image: Handle<Image>,
    u_image: Handle<Image>,
    v_image: Handle<Image>,
    image_entity: Entity,
}

pub struct HirakuVideoPlugin;

impl Plugin for HirakuVideoPlugin {
    fn build(&self, app: &mut App) {
        load_internal_shader(app);
        app.init_asset::<VideoAsset>()
            .init_asset_loader::<VideoAssetLoader>()
            .add_audio_source::<VideoAudio>()
            .add_plugins(UiMaterialPlugin::<Yuv420Material>::default())
            .init_resource::<VideoPlayer>()
            .init_resource::<ActiveVideo>()
            .add_message::<VideoEvent>()
            .add_systems(
                Update,
                (start_pending_video, apply_video_controls, update_video).chain(),
            );
    }
}

fn start_pending_video(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    videos: Res<Assets<VideoAsset>>,
    mut audio_assets: ResMut<Assets<VideoAudio>>,
    mut player: ResMut<VideoPlayer>,
    mut active: ResMut<ActiveVideo>,
    mut events: MessageWriter<VideoEvent>,
) {
    if active.0.is_some() {
        return;
    }
    let Some(pending) = player.pending.front() else {
        return;
    };
    let Some(asset) = videos.get(&pending.asset) else {
        if let LoadState::Failed(error) = asset_server.load_state(&pending.asset) {
            let pending = player
                .pending
                .pop_front()
                .expect("the inspected video request must remain queued");
            fail_playback(
                pending.id,
                format!("video asset failed to load: {error}"),
                &mut player,
                &mut events,
            );
        }
        return;
    };
    let pending = player
        .pending
        .pop_front()
        .expect("the loaded video request must remain queued");
    let start_paused = matches!(
        player.states.get(&pending.id),
        Some(VideoPlaybackState::Paused)
    );
    let stream = spawn_decoder(asset);
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: px(0),
                bottom: px(0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(VIDEO_Z_INDEX),
            Pickable::IGNORE,
        ))
        .id();
    let audio = audio_assets.add(stream.audio);
    // Keep audio stopped until the first decoded frame is ready. This makes
    // Opus the master clock without allowing it to run ahead during startup.
    let playback_settings = PlaybackSettings::ONCE.paused();
    let audio_entity = commands.spawn((AudioPlayer(audio), playback_settings)).id();
    player.active = Some(pending.id);
    active.0 = Some(ActivePlayback {
        id: pending.id,
        receiver: stream.video,
        frames: VecDeque::new(),
        surface: None,
        root,
        audio_entity,
        position: Duration::ZERO,
        paused: start_paused,
        started: false,
        decoder_ended: false,
        last_timestamp: Duration::ZERO,
        cancellation: stream.cancellation,
        age: Duration::ZERO,
    });
}

fn apply_video_controls(
    mut commands: Commands,
    mut player: ResMut<VideoPlayer>,
    mut active: ResMut<ActiveVideo>,
    sinks: Query<&AudioSink>,
    mut events: MessageWriter<VideoEvent>,
) {
    while let Some(control) = player.controls.pop_front() {
        match control {
            PlaybackControl::Pause(id) => {
                if let Some(playback) = active.0.as_mut().filter(|playback| playback.id == id) {
                    playback.paused = true;
                    if let Ok(sink) = sinks.get(playback.audio_entity) {
                        sink.pause();
                    }
                    player.states.insert(id, VideoPlaybackState::Paused);
                } else if player.pending.iter().any(|pending| pending.id == id) {
                    player.states.insert(id, VideoPlaybackState::Paused);
                }
            }
            PlaybackControl::Resume(id) => {
                if let Some(playback) = active.0.as_mut().filter(|playback| playback.id == id) {
                    playback.paused = false;
                    if let Ok(sink) = sinks.get(playback.audio_entity) {
                        sink.play();
                    }
                    player.states.insert(id, VideoPlaybackState::Playing);
                } else if player.pending.iter().any(|pending| pending.id == id) {
                    player.states.insert(id, VideoPlaybackState::Loading);
                }
            }
            PlaybackControl::Skip(id) => {
                if let Some(playback) = active.0.as_ref().filter(|playback| playback.id == id) {
                    if let Ok(sink) = sinks.get(playback.audio_entity) {
                        sink.stop();
                    }
                    cleanup_playback(&mut commands, playback);
                    player.states.insert(id, VideoPlaybackState::Skipped);
                    player.active = None;
                    active.0 = None;
                    events.write(VideoEvent::Skipped { id });
                } else if let Some(index) =
                    player.pending.iter().position(|pending| pending.id == id)
                {
                    player.pending.remove(index);
                    player.states.insert(id, VideoPlaybackState::Skipped);
                    events.write(VideoEvent::Skipped { id });
                }
            }
        }
    }
}

fn update_video(
    mut commands: Commands,
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<Yuv420Material>>,
    mut nodes: Query<&mut Node>,
    sinks: Query<&AudioSink>,
    mut player: ResMut<VideoPlayer>,
    mut active: ResMut<ActiveVideo>,
    mut events: MessageWriter<VideoEvent>,
) {
    let Some(playback) = active.0.as_mut() else {
        return;
    };
    playback.age += time.delta();
    if sinks.get(playback.audio_entity).is_err() && playback.age >= AUDIO_SINK_TIMEOUT {
        let id = playback.id;
        cleanup_playback(&mut commands, playback);
        active.0 = None;
        player.active = None;
        fail_playback(
            id,
            "Bevy did not create an audio sink for the movie".into(),
            &mut player,
            &mut events,
        );
        return;
    }
    if let Some(result) = drain_ready_frames(&playback.receiver, &mut playback.frames) {
        match result {
            Ok(()) => playback.decoder_ended = true,
            Err(error) => {
                let id = playback.id;
                cleanup_playback(&mut commands, playback);
                active.0 = None;
                player.active = None;
                fail_playback(id, error, &mut player, &mut events);
                return;
            }
        }
    }
    if !playback.started && !playback.frames.is_empty() {
        playback.started = true;
        player.states.insert(
            playback.id,
            if playback.paused {
                VideoPlaybackState::Paused
            } else {
                VideoPlaybackState::Playing
            },
        );
        events.write(VideoEvent::Started { id: playback.id });
    }
    if playback.started && !playback.paused {
        if let Ok(sink) = sinks.get(playback.audio_entity)
            && sink.is_paused()
        {
            sink.play();
        }
        playback.position = sinks
            .get(playback.audio_entity)
            .map(AudioSinkPlayback::position)
            .unwrap_or_else(|_| playback.position + time.delta());
    }
    while playback
        .frames
        .front()
        .is_some_and(|frame| frame.timestamp <= playback.position)
    {
        let frame = playback
            .frames
            .pop_front()
            .expect("the checked frame queue must not be empty");
        playback.last_timestamp = frame.timestamp;
        present_frame(
            &mut commands,
            &mut images,
            &mut materials,
            &mut nodes,
            playback,
            frame,
        );
    }
    if playback.paused
        && let Ok(sink) = sinks.get(playback.audio_entity)
        && !sink.is_paused()
    {
        sink.pause();
    }
    let audio_finished = sinks
        .get(playback.audio_entity)
        .is_ok_and(AudioSinkPlayback::empty);
    if playback.decoder_ended
        && playback.frames.is_empty()
        && playback.position >= playback.last_timestamp + LAST_FRAME_HOLD
        && audio_finished
    {
        let id = playback.id;
        cleanup_playback(&mut commands, playback);
        active.0 = None;
        player.active = None;
        player.states.insert(id, VideoPlaybackState::Finished);
        events.write(VideoEvent::Finished { id });
    }
}

fn present_frame(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    materials: &mut Assets<Yuv420Material>,
    nodes: &mut Query<&mut Node>,
    playback: &mut ActivePlayback,
    frame: DecodedFrame,
) {
    let aspect_ratio = frame.width as f32 / frame.height as f32;
    if let Some(surface) = playback.surface.as_ref() {
        replace_plane(images, &surface.y_image, frame.width, frame.height, frame.y);
        replace_plane(
            images,
            &surface.u_image,
            frame.chroma_width,
            frame.chroma_height,
            frame.u,
        );
        replace_plane(
            images,
            &surface.v_image,
            frame.chroma_width,
            frame.chroma_height,
            frame.v,
        );
        if let Ok(mut node) = nodes.get_mut(surface.image_entity) {
            node.aspect_ratio = Some(aspect_ratio);
        }
        return;
    }

    let y_image = images.add(plane_image(frame.width, frame.height, frame.y));
    let u_image = images.add(plane_image(
        frame.chroma_width,
        frame.chroma_height,
        frame.u,
    ));
    let v_image = images.add(plane_image(
        frame.chroma_width,
        frame.chroma_height,
        frame.v,
    ));
    let material = materials.add(Yuv420Material {
        y: y_image.clone(),
        u: u_image.clone(),
        v: v_image.clone(),
        color_transform: frame.color_transform,
        transfer: frame.transfer,
    });
    let image_entity = commands
        .spawn((
            MaterialNode(material),
            Node {
                width: percent(100),
                max_height: percent(100),
                aspect_ratio: Some(aspect_ratio),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(playback.root).add_child(image_entity);
    playback.surface = Some(VideoSurface {
        y_image,
        u_image,
        v_image,
        image_entity,
    });
}

fn cleanup_playback(commands: &mut Commands, playback: &ActivePlayback) {
    playback.cancellation.store(true, Ordering::Relaxed);
    commands.entity(playback.root).try_despawn();
    commands.entity(playback.audio_entity).try_despawn();
}

fn fail_playback(
    id: VideoPlaybackId,
    error: String,
    player: &mut VideoPlayer,
    events: &mut MessageWriter<VideoEvent>,
) {
    player
        .states
        .insert(id, VideoPlaybackState::Failed(error.clone()));
    events.write(VideoEvent::Failed { id, error });
}

fn replace_plane(
    images: &mut Assets<Image>,
    handle: &Handle<Image>,
    width: u32,
    height: u32,
    data: Vec<u8>,
) {
    if let Some(mut image) = images.get_mut(handle) {
        let size = image.texture_descriptor.size;
        if size.width == width
            && size.height == height
            && image.texture_descriptor.format == TextureFormat::R8Unorm
        {
            image.data = Some(data);
        } else {
            *image = plane_image(width, height, data);
        }
    }
}

fn plane_image(width: u32, height: u32, data: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_ids_are_monotonic_and_controls_are_available_without_a_backend() {
        let mut player = VideoPlayer::default();
        let first = player.play(Handle::default());
        let second = player.play(Handle::default());
        assert_eq!(first, VideoPlaybackId(1));
        assert_eq!(second, VideoPlaybackId(2));
        assert_eq!(player.state(first), Some(&VideoPlaybackState::Loading));
        player.pause(first);
        player.resume(first);
        player.skip(first);
        assert_eq!(player.controls.len(), 3);
    }
}
