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

use crate::platform::{
    VideoFrameI420, VideoFrameNv12, VideoFrameUpload, VideoUpload, install_video_upload,
};
use crate::{
    VideoAsset, VideoAssetLoader,
    audio::VideoAudio,
    render::{Yuv420Material, load_internal_shader},
};
use hiraku_media::{
    DecodeSettings, DecoderHandle, VideoEvent as DecodeEvent, VideoFrame as DecodedFrame,
    VideoPixels as DecodedPixels, YuvPixelFormat,
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

/// Controls native AV1 decoder parallelism.
///
/// Leave either field as `None` to select a conservative value from the
/// process' available parallelism. These settings are ignored by WebCodecs.
#[derive(Clone, Debug, Resource)]
pub struct VideoDecodeSettings {
    pub decoder_threads: Option<u32>,
    pub max_frame_delay: Option<u32>,
}

impl Default for VideoDecodeSettings {
    fn default() -> Self {
        Self {
            decoder_threads: None,
            max_frame_delay: None,
        }
    }
}

impl VideoDecodeSettings {
    pub fn fixed(decoder_threads: u32, max_frame_delay: u32) -> Self {
        Self {
            decoder_threads: Some(decoder_threads.max(1)),
            max_frame_delay: Some(max_frame_delay.max(1)),
        }
    }
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

#[derive(Default)]
struct ActiveVideo(Option<ActivePlayback>);

struct ActivePlayback {
    id: VideoPlaybackId,
    receiver: crossbeam_channel::Receiver<DecodeEvent>,
    frames: VecDeque<DecodedFrame>,
    surface: Option<VideoSurface>,
    root: Entity,
    audio_entity: Option<Entity>,
    position: Duration,
    paused: bool,
    started: bool,
    decoder_ended: bool,
    last_timestamp: Duration,
    cancellation: Arc<AtomicBool>,
    queued_frames: Option<Arc<std::sync::atomic::AtomicUsize>>,
    age: Duration,
    _decoder: DecoderHandle,
}

enum VideoSurface {
    YuvI420 {
        y_image: Handle<Image>,
        u_image: Handle<Image>,
        v_image: Handle<Image>,
        image_entity: Entity,
    },
    YuvNv12 {
        y_image: Handle<Image>,
        uv_image: Handle<Image>,
        image_entity: Entity,
    },
    Rgba {
        image: Handle<Image>,
        image_entity: Entity,
    },
}

pub struct HirakuVideoPlugin;

impl Plugin for HirakuVideoPlugin {
    fn build(&self, app: &mut App) {
        load_internal_shader(app);
        install_video_upload(app);
        app.insert_non_send(ActiveVideo::default());
        app.init_asset::<VideoAsset>()
            .init_asset_loader::<VideoAssetLoader>()
            .add_audio_source::<VideoAudio>()
            .add_plugins(UiMaterialPlugin::<Yuv420Material>::default())
            .init_resource::<VideoDecodeSettings>()
            .init_resource::<VideoPlayer>()
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
    mut active: NonSendMut<ActiveVideo>,
    decode_settings: Res<VideoDecodeSettings>,
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
    let stream = hiraku_media::decode(
        &asset.media,
        &DecodeSettings {
            decoder_threads: decode_settings.decoder_threads,
            max_frame_delay: decode_settings.max_frame_delay,
        },
    );
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
    let audio_entity = spawn_movie_audio(
        &mut commands,
        &mut audio_assets,
        VideoAudio::new(stream.audio, stream.metadata),
    );
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
        queued_frames: stream.queued_frames,
        age: Duration::ZERO,
        _decoder: stream.handle,
    });
}

fn apply_video_controls(
    mut commands: Commands,
    mut player: ResMut<VideoPlayer>,
    mut active: NonSendMut<ActiveVideo>,
    sinks: Query<&AudioSink>,
    mut events: MessageWriter<VideoEvent>,
) {
    while let Some(control) = player.controls.pop_front() {
        match control {
            PlaybackControl::Pause(id) => {
                if let Some(playback) = active.0.as_mut().filter(|playback| playback.id == id) {
                    playback.paused = true;
                    if let Some(audio_entity) = playback.audio_entity
                        && let Ok(sink) = sinks.get(audio_entity)
                    {
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
                    if let Some(audio_entity) = playback.audio_entity
                        && let Ok(sink) = sinks.get(audio_entity)
                    {
                        sink.play();
                    }
                    player.states.insert(id, VideoPlaybackState::Playing);
                } else if player.pending.iter().any(|pending| pending.id == id) {
                    player.states.insert(id, VideoPlaybackState::Loading);
                }
            }
            PlaybackControl::Skip(id) => {
                if let Some(playback) = active.0.as_ref().filter(|playback| playback.id == id) {
                    if let Some(audio_entity) = playback.audio_entity
                        && let Ok(sink) = sinks.get(audio_entity)
                    {
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
    mut active: NonSendMut<ActiveVideo>,
    mut events: MessageWriter<VideoEvent>,
    mut video_upload: ResMut<VideoUpload>,
) {
    let Some(playback) = active.0.as_mut() else {
        return;
    };
    playback.age += time.delta();
    if playback
        .audio_entity
        .is_none_or(|audio_entity| sinks.get(audio_entity).is_err())
        && playback.age >= AUDIO_SINK_TIMEOUT
    {
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
        if !playback.paused {}
    }
    if playback.started && !playback.paused {
        if let Some(audio_entity) = playback.audio_entity
            && let Ok(sink) = sinks.get(audio_entity)
            && sink.is_paused()
        {
            sink.play();
        }
        playback.position = playback
            .audio_entity
            .and_then(|audio_entity| sinks.get(audio_entity).ok())
            .map(AudioSinkPlayback::position)
            .unwrap_or_else(|| playback.position + time.delta());
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
        if let Some(queued_frames) = playback.queued_frames.as_ref() {
            queued_frames.fetch_sub(1, Ordering::Relaxed);
        }
        playback.last_timestamp = frame.timestamp;
        present_frame(
            &mut commands,
            &mut images,
            &mut materials,
            &mut nodes,
            &mut video_upload,
            playback,
            frame,
        );
    }
    if playback.paused
        && let Some(audio_entity) = playback.audio_entity
        && let Ok(sink) = sinks.get(audio_entity)
        && !sink.is_paused()
    {
        sink.pause();
    }
    let audio_finished = playback
        .audio_entity
        .and_then(|audio_entity| sinks.get(audio_entity).ok())
        .is_some_and(AudioSinkPlayback::empty);
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
    upload: &mut VideoUpload,
    playback: &mut ActivePlayback,
    frame: DecodedFrame,
) {
    let aspect_ratio = frame.width as f32 / frame.height as f32;
    let (y, u, v) = match frame.pixels {
        DecodedPixels::I420Planar { y, u, v } => (y, u, v),
        DecodedPixels::Rgba(rgba) => {
            if let Some(VideoSurface::Rgba {
                image,
                image_entity,
            }) = playback.surface.as_ref()
            {
                replace_rgba(images, image, frame.width, frame.height, rgba);
                if let Ok(mut node) = nodes.get_mut(*image_entity) {
                    node.aspect_ratio = Some(aspect_ratio);
                }
                return;
            }
            replace_surface(commands, playback);
            let image = images.add(rgba_image(frame.width, frame.height, rgba));
            let image_entity = commands
                .spawn((
                    ImageNode::new(image.clone()),
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
            playback.surface = Some(VideoSurface::Rgba {
                image,
                image_entity,
            });
            return;
        }
        pixels @ DecodedPixels::I420Strided { .. } => {
            present_strided_frame(
                commands,
                images,
                materials,
                nodes,
                upload,
                playback,
                DecodedFrame { pixels, ..frame },
            );
            return;
        }
        pixels @ DecodedPixels::Nv12Strided { .. } => {
            present_nv12_frame(
                commands,
                images,
                materials,
                nodes,
                upload,
                playback,
                DecodedFrame { pixels, ..frame },
            );
            return;
        }
    };

    if let Some(VideoSurface::YuvI420 {
        y_image,
        u_image,
        v_image,
        image_entity,
    }) = playback.surface.as_ref()
    {
        replace_plane(images, y_image, frame.width, frame.height, y);
        replace_plane(images, u_image, frame.chroma_width, frame.chroma_height, u);
        replace_plane(images, v_image, frame.chroma_width, frame.chroma_height, v);
        if let Ok(mut node) = nodes.get_mut(*image_entity) {
            node.aspect_ratio = Some(aspect_ratio);
        }
        return;
    }
    replace_surface(commands, playback);

    let y_image = images.add(plane_image(frame.width, frame.height, y));
    let u_image = images.add(plane_image(frame.chroma_width, frame.chroma_height, u));
    let v_image = images.add(plane_image(frame.chroma_width, frame.chroma_height, v));
    let material = materials.add(Yuv420Material {
        y: y_image.clone(),
        u: u_image.clone(),
        v: v_image.clone(),
        color_transform: frame.color_transform.into(),
        transfer: frame.transfer.into(),
        format: YuvPixelFormat::I420,
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
    playback.surface = Some(VideoSurface::YuvI420 {
        y_image,
        u_image,
        v_image,
        image_entity,
    });
}

fn present_strided_frame(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    materials: &mut Assets<Yuv420Material>,
    nodes: &mut Query<&mut Node>,
    upload: &mut VideoUpload,
    playback: &mut ActivePlayback,
    frame: DecodedFrame,
) {
    let DecodedPixels::I420Strided {
        planes,
        u_offset,
        v_offset,
        y_stride,
        chroma_stride,
    } = frame.pixels
    else {
        unreachable!("strided frame presenter received a packed WebCodecs frame")
    };
    let aspect_ratio = frame.width as f32 / frame.height as f32;
    let (y_image, u_image, v_image, image_entity) = if let Some(VideoSurface::YuvI420 {
        y_image,
        u_image,
        v_image,
        image_entity,
    }) = playback.surface.as_ref()
    {
        (
            y_image.clone(),
            u_image.clone(),
            v_image.clone(),
            *image_entity,
        )
    } else {
        replace_surface(commands, playback);
        let y_image = images.add(empty_plane_image(frame.width, frame.height));
        let u_image = images.add(empty_plane_image(frame.chroma_width, frame.chroma_height));
        let v_image = images.add(empty_plane_image(frame.chroma_width, frame.chroma_height));
        let material = materials.add(Yuv420Material {
            y: y_image.clone(),
            u: u_image.clone(),
            v: v_image.clone(),
            color_transform: frame.color_transform.into(),
            transfer: frame.transfer.into(),
            format: YuvPixelFormat::I420,
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
        playback.surface = Some(VideoSurface::YuvI420 {
            y_image: y_image.clone(),
            u_image: u_image.clone(),
            v_image: v_image.clone(),
            image_entity,
        });
        (y_image, u_image, v_image, image_entity)
    };

    if let Ok(mut node) = nodes.get_mut(image_entity) {
        node.aspect_ratio = Some(aspect_ratio);
    }

    let DecodedFrame {
        width,
        height,
        chroma_width,
        chroma_height,
        ..
    } = frame;
    upload.publish(VideoFrameUpload::I420(VideoFrameI420 {
        y_image,
        u_image,
        v_image,
        width,
        height,
        chroma_width,
        chroma_height,
        planes,
        u_offset,
        v_offset,
        y_stride,
        chroma_stride,
    }));
}

fn present_nv12_frame(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    materials: &mut Assets<Yuv420Material>,
    nodes: &mut Query<&mut Node>,
    upload: &mut VideoUpload,
    playback: &mut ActivePlayback,
    frame: DecodedFrame,
) {
    let DecodedPixels::Nv12Strided {
        planes,
        uv_offset,
        y_stride,
        uv_stride,
    } = frame.pixels
    else {
        unreachable!("Nv12 presenter received a non-NV12 frame");
    };

    let aspect_ratio = frame.width as f32 / frame.height as f32;

    let (y_image, uv_image, image_entity) = if let Some(VideoSurface::YuvNv12 {
        y_image,
        uv_image,
        image_entity,
    }) = playback.surface.as_ref()
    {
        (y_image.clone(), uv_image.clone(), *image_entity)
    } else {
        replace_surface(commands, playback);

        let y_image = images.add(empty_plane_image(frame.width, frame.height));

        let uv_image = images.add(empty_uv_plane_image(
            frame.chroma_width,
            frame.chroma_height,
        ));

        let dummy_image = images.add(empty_plane_image(1, 1));

        let material = materials.add(Yuv420Material {
            y: y_image.clone(),
            u: uv_image.clone(),
            v: dummy_image.clone(),
            color_transform: frame.color_transform.into(),
            transfer: frame.transfer.into(),
            format: YuvPixelFormat::Nv12,
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

        playback.surface = Some(VideoSurface::YuvNv12 {
            y_image: y_image.clone(),
            uv_image: uv_image.clone(),
            image_entity,
        });

        (y_image, uv_image, image_entity)
    };

    let DecodedFrame {
        width,
        height,
        chroma_width,
        chroma_height,
        ..
    } = frame;

    upload.publish(VideoFrameUpload::Nv12(VideoFrameNv12 {
        y_image,
        uv_image,
        width,
        height,
        chroma_width,
        chroma_height,
        planes,
        uv_offset,
        y_stride,
        uv_stride,
    }));
}

fn replace_surface(commands: &mut Commands, playback: &mut ActivePlayback) {
    let entity = match playback.surface.take() {
        Some(VideoSurface::YuvI420 { image_entity, .. }) => image_entity,
        Some(VideoSurface::Rgba { image_entity, .. }) => image_entity,
        Some(VideoSurface::YuvNv12 { image_entity, .. }) => image_entity,
        None => return,
    };
    commands.entity(entity).try_despawn();
}

fn cleanup_playback(commands: &mut Commands, playback: &ActivePlayback) {
    playback.cancellation.store(true, Ordering::Relaxed);
    commands.entity(playback.root).try_despawn();
    if let Some(audio_entity) = playback.audio_entity {
        commands.entity(audio_entity).try_despawn();
    }
}

fn spawn_movie_audio(
    commands: &mut Commands,
    audio_assets: &mut Assets<VideoAudio>,
    audio: VideoAudio,
) -> Option<Entity> {
    let audio = audio_assets.add(audio);
    Some(
        commands
            .spawn((AudioPlayer(audio), PlaybackSettings::ONCE.paused()))
            .id(),
    )
}

fn drain_ready_frames(
    receiver: &crossbeam_channel::Receiver<DecodeEvent>,
    queue: &mut VecDeque<DecodedFrame>,
) -> Option<Result<(), String>> {
    let mut terminal = None;
    while queue.len() < 3 {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            DecodeEvent::Frame(frame) => queue.push_back(frame),
            DecodeEvent::End => terminal = Some(Ok(())),
            DecodeEvent::Error(error) => terminal = Some(Err(error)),
        }
    }
    terminal
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

fn empty_plane_image(width: u32, height: u32) -> Image {
    Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0],
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

fn empty_uv_plane_image(width: u32, height: u32) -> Image {
    Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0],
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

fn replace_rgba(
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
            && image.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb
        {
            image.data = Some(data);
        } else {
            *image = rgba_image(width, height, data);
        }
    }
}

fn rgba_image(width: u32, height: u32, data: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
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
