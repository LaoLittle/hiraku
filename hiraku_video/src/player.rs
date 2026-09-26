use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};

use bevy::{
    asset::{AssetApp, LoadState, RenderAssetUsages},
    audio::{AddAudioSource, AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings},
    camera::visibility::RenderLayers,
    image::Image,
    pbr::MaterialPlugin,
    picking::Pickable,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use crate::decode::{MediaDecoder, VideoEvent as DecodeEvent};
use crate::upload::{VideoUpload, VideoUploads, install_video_upload};
use crate::{
    VideoAsset, VideoAssetLoader,
    audio::VideoAudio,
    render::{Yuv420Material, load_internal_shader},
};
use hiraku_media::{
    DecodeSettings, VideoFrame as DecodedFrame, VideoPixels as DecodedPixels, YuvPixelFormat,
};

const VIDEO_Z_INDEX: i32 = 30_000;
mod views;
pub use views::VideoWorldView;
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
    looping: bool,
    parent: Option<Entity>,
    world_size: Option<Vec2>,
    id: VideoPlaybackId,
    asset: Handle<VideoAsset>,
    z_index: i32,
    fade_out: Duration,
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
    reparents: Vec<(VideoPlaybackId, Entity)>,
    states: BTreeMap<VideoPlaybackId, VideoPlaybackState>,
    active: Option<VideoPlaybackId>,
    suspended: BTreeSet<VideoPlaybackId>,
    opacity: BTreeMap<VideoPlaybackId, f32>,
}

impl VideoPlayer {
    /// Transfer the owning spatial parent without restarting playback. The old
    /// parent must remain alive until PostUpdate applies the presentation change.
    pub fn reparent_world(&mut self, id: VideoPlaybackId, parent: Entity) -> bool {
        if let Some(pending) = self.pending.iter_mut().find(|p| p.id == id) {
            if pending.world_size.is_none() {
                return false;
            }
            pending.parent = Some(parent);
            return true;
        }
        if !matches!(
            self.states.get(&id),
            Some(
                VideoPlaybackState::Loading
                    | VideoPlaybackState::Playing
                    | VideoPlaybackState::Paused
            )
        ) {
            return false;
        }
        self.reparents.push((id, parent));
        true
    }
    /// Host animation opacity, multiplied with the video's natural exit fade.
    pub fn set_opacity(&mut self, id: VideoPlaybackId, opacity: f32) -> bool {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) || !self.states.contains_key(&id)
        {
            return false;
        }
        self.opacity.insert(id, opacity);
        true
    }
    pub fn is_suspended(&self, id: VideoPlaybackId) -> bool {
        self.suspended.contains(&id)
    }

    /// Suspend a host-owned playback clock without changing its manual pause
    /// state. This also applies while loading and across loop boundaries.
    /// Releasing suspension never resumes an explicitly paused playback.
    pub fn set_suspended(&mut self, id: VideoPlaybackId, suspended: bool) -> bool {
        if !matches!(
            self.states.get(&id),
            Some(
                VideoPlaybackState::Loading
                    | VideoPlaybackState::Playing
                    | VideoPlaybackState::Paused
            )
        ) {
            return false;
        }
        if suspended {
            self.suspended.insert(id);
        } else {
            self.suspended.remove(&id);
        }
        true
    }

    pub fn play(&mut self, asset: Handle<VideoAsset>) -> VideoPlaybackId {
        self.play_at_layer(asset, VIDEO_Z_INDEX)
    }

    /// Non-modal fullscreen video, below the host's positive-Z UI layers.
    pub fn play_under_ui(&mut self, asset: Handle<VideoAsset>) -> VideoPlaybackId {
        self.play_at_layer(asset, 0)
    }

    /// Independent playback contained by a host-owned UI node. It does not
    /// replace or queue behind fullscreen movies; controls address its own ID.
    /// Despawning the parent cancels playback and releases its decoder/surfaces.
    pub fn play_in(&mut self, asset: Handle<VideoAsset>, parent: Entity) -> VideoPlaybackId {
        let id = self.play_at_layer(asset, 0);
        self.pending
            .back_mut()
            .expect("new playback is queued")
            .parent = Some(parent);
        id
    }

    /// Render an unlit video quad in the host's world hierarchy. Size is in
    /// world units; position, rotation and scale belong to the parent's Transform.
    pub fn play_world(
        &mut self,
        asset: Handle<VideoAsset>,
        parent: Entity,
        size: Vec2,
    ) -> Result<VideoPlaybackId, &'static str> {
        if !size.is_finite() || size.min_element() <= 0.0 {
            return Err("video quad size must be finite and positive");
        }
        let id = self.play_in(asset, parent);
        self.pending.back_mut().expect("new playback").world_size = Some(size);
        Ok(id)
    }

    /// Configure natural completion before playback starts. The last frame
    /// remains alive until its exit fade completes; skip still cancels at once.
    pub fn set_fade_out(&mut self, id: VideoPlaybackId, duration: Duration) -> bool {
        if let Some(pending) = self.pending.iter_mut().find(|pending| pending.id == id) {
            pending.fade_out = duration;
            true
        } else {
            false
        }
    }

    /// Configure repetition before startup. A looping instance finishes only
    /// when explicitly stopped or when its host is removed.
    pub fn set_looping(&mut self, id: VideoPlaybackId, looping: bool) -> bool {
        if let Some(pending) = self.pending.iter_mut().find(|pending| pending.id == id) {
            pending.looping = looping;
            true
        } else {
            false
        }
    }

    fn play_at_layer(&mut self, asset: Handle<VideoAsset>, z_index: i32) -> VideoPlaybackId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("video playback identifier space must not be exhausted");
        let id = VideoPlaybackId(self.next_id);
        self.states.insert(id, VideoPlaybackState::Loading);
        self.pending.push_back(PendingPlayback {
            looping: false,
            parent: None,
            world_size: None,
            id,
            asset,
            z_index,
            fade_out: Duration::ZERO,
        });
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

    /// Cancel loading requests as well as active playback through normal terminal events.
    pub fn stop_all(&mut self) {
        let live: Vec<_> = self
            .states
            .iter()
            .filter_map(|(id, state)| {
                matches!(
                    state,
                    VideoPlaybackState::Loading
                        | VideoPlaybackState::Playing
                        | VideoPlaybackState::Paused
                )
                .then_some(*id)
            })
            .collect();
        for id in live {
            self.skip(id);
        }
    }

    /// Active fullscreen playback, excluding independently hosted surfaces.
    pub fn active(&self) -> Option<VideoPlaybackId> {
        self.active
    }

    pub fn state(&self, id: VideoPlaybackId) -> Option<&VideoPlaybackState> {
        self.states.get(&id)
    }
}

#[derive(Default)]
struct ActiveVideo(BTreeMap<VideoPlaybackId, ActivePlayback>);

struct ActivePlayback {
    repeat: Option<(VideoAsset, DecodeSettings)>,
    awaiting_restart: bool,
    skip_blank_lead: bool,
    frame_step: Duration,
    world: Option<(Handle<Mesh>, RenderLayers)>,
    alpha_layout: Option<crate::AlphaLayout>,
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
    audio_clock: VideoAudio,
    age: Duration,
    decoder: MediaDecoder,
    fade_out: Duration,
    exit_elapsed: Option<Duration>,
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

impl ActivePlayback {
    fn display_aspect(&self, width: u32, height: u32) -> f32 {
        let (width, height) = self
            .alpha_layout
            .and_then(|layout| layout.display_size(width, height))
            .unwrap_or((width, height));
        width as f32 / height as f32
    }
}

pub struct HirakuVideoPlugin;

/// Hosts update clock policy before this set; completion consumers run after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoPlaybackSystems;

impl Plugin for HirakuVideoPlugin {
    fn build(&self, app: &mut App) {
        load_internal_shader(app);
        install_video_upload(app);
        app.insert_non_send(ActiveVideo::default());
        views::install(app);
        app.init_asset::<VideoAsset>()
            .init_asset_loader::<VideoAssetLoader>()
            .add_audio_source::<VideoAudio>()
            .add_plugins(UiMaterialPlugin::<Yuv420Material>::default())
            .add_plugins(MaterialPlugin::<Yuv420Material>::default())
            .init_resource::<VideoDecodeSettings>()
            .init_resource::<VideoPlayer>()
            .add_message::<VideoEvent>()
            .add_systems(PostUpdate, sync_video_opacity)
            .add_systems(
                Update,
                (start_pending_video, apply_video_controls, update_video)
                    .chain()
                    .in_set(VideoPlaybackSystems),
            );
    }
}

fn sync_video_opacity(
    active: NonSend<ActiveVideo>,
    mut player: ResMut<VideoPlayer>,
    mut materials: ResMut<Assets<Yuv420Material>>,
    surfaces: Query<(
        Option<&MaterialNode<Yuv420Material>>,
        Option<&MeshMaterial3d<Yuv420Material>>,
    )>,
    mut images: Query<&mut ImageNode>,
) {
    for (&id, playback) in &active.0 {
        let Some(surface) = &playback.surface else {
            continue;
        };
        let entity = match surface {
            VideoSurface::YuvI420 { image_entity, .. }
            | VideoSurface::YuvNv12 { image_entity, .. }
            | VideoSurface::Rgba { image_entity, .. } => *image_entity,
        };
        let opacity = player.opacity.get(&id).copied().unwrap_or(1.0)
            * playback
                .exit_elapsed
                .map_or(1.0, |elapsed| exit_opacity(elapsed, playback.fade_out));
        if let Ok((ui, world)) = surfaces.get(entity)
            && let Some(handle) = ui.map(|n| &n.0).or_else(|| world.map(|m| &m.0))
            && let Some(mut material) = materials.get_mut(handle)
            && material.opacity != opacity
        {
            material.opacity = opacity;
        }
        if let Ok(mut image) = images.get_mut(entity)
            && image.color.alpha() != opacity
        {
            image.color.set_alpha(opacity);
        }
    }
    let VideoPlayer {
        opacity, states, ..
    } = &mut *player;
    opacity.retain(|id, _| {
        matches!(
            states.get(id),
            Some(
                VideoPlaybackState::Loading
                    | VideoPlaybackState::Playing
                    | VideoPlaybackState::Paused
            )
        )
    });
}

fn start_pending_video(
    redraw: Option<Res<Messages<bevy::window::RequestRedraw>>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    videos: Res<Assets<VideoAsset>>,
    mut audio_assets: ResMut<Assets<VideoAudio>>,
    mut player: ResMut<VideoPlayer>,
    mut active: NonSendMut<ActiveVideo>,
    decode_settings: Res<VideoDecodeSettings>,
    mut events: MessageWriter<VideoEvent>,
    parents: Query<(Option<&Node>, Option<&Transform>, Option<&RenderLayers>)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let mut index = 0;
    let mut fullscreen_claimed = player.active.is_some();
    while index < player.pending.len() {
        let pending = &player.pending[index];
        if pending.parent.is_none() {
            if fullscreen_claimed {
                index += 1;
                continue;
            }
            // Loading fullscreen requests keep their order; independent
            // surfaces later in the queue may still proceed.
            fullscreen_claimed = true;
        }
        if pending
            .parent
            .is_some_and(|parent| parents.get(parent).is_err())
        {
            let pending = player.pending.remove(index).expect("pending index");
            player
                .states
                .insert(pending.id, VideoPlaybackState::Skipped);
            events.write(VideoEvent::Skipped { id: pending.id });
            continue;
        }
        if let Some(parent) = pending.parent {
            let (node, transform, _) = parents.get(parent).expect("parent exists");
            if (pending.world_size.is_some() && transform.is_none())
                || (pending.world_size.is_none() && node.is_none())
            {
                let pending = player.pending.remove(index).expect("pending index");
                fail_playback(
                    pending.id,
                    "video parent requires a Transform for world playback or Node for UI playback"
                        .into(),
                    &mut player,
                    &mut events,
                );
                continue;
            }
        }
        if redraw.is_some() {
            commands.write_message(bevy::window::RequestRedraw);
        }
        let Some(asset) = videos.get(&pending.asset) else {
            if let LoadState::Failed(error) = asset_server.load_state(&pending.asset) {
                let pending = player
                    .pending
                    .remove(index)
                    .expect("the inspected video request must remain queued");
                fail_playback(
                    pending.id,
                    format!("video asset failed to load: {error}"),
                    &mut player,
                    &mut events,
                );
            } else {
                index += 1;
            }
            continue;
        };
        let pending = player
            .pending
            .remove(index)
            .expect("the loaded video request must remain queued");
        let start_paused = matches!(
            player.states.get(&pending.id),
            Some(VideoPlaybackState::Paused)
        );
        let stream = match MediaDecoder::new(
            &asset.media,
            DecodeSettings {
                decoder_threads: decode_settings.decoder_threads,
                max_frame_delay: decode_settings.max_frame_delay,
            },
        ) {
            Ok(stream) => stream,
            Err(error) => {
                fail_playback(pending.id, error.to_string(), &mut player, &mut events);
                continue;
            }
        };
        let audio = VideoAudio::new(stream.audio.clone(), asset.metadata);
        let world = pending.world_size.map(|size| {
            (
                meshes.add(Rectangle::from_size(size)),
                pending
                    .parent
                    .and_then(|parent| parents.get(parent).ok())
                    .and_then(|(_, _, layers)| layers.cloned())
                    .unwrap_or_default(),
            )
        });
        let root = if world.is_some() {
            commands
                .spawn((
                    Transform::default(),
                    Visibility::Inherited,
                    Pickable::IGNORE,
                ))
                .id()
        } else {
            commands
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
                    BackgroundColor(if pending.z_index == 0 || asset.alpha_layout.is_some() {
                        Color::NONE
                    } else {
                        Color::BLACK
                    }),
                    Pickable::IGNORE,
                ))
                .id()
        };
        if let Some(parent) = pending.parent {
            commands.entity(root).insert(ChildOf(parent));
        } else {
            commands.entity(root).insert(GlobalZIndex(pending.z_index));
        }
        let audio_entity = if asset.metadata.channels == 0 {
            None
        } else {
            spawn_movie_audio(&mut commands, &mut audio_assets, audio.clone())
        };
        if pending.parent.is_none() {
            player.active = Some(pending.id);
        }
        active.0.insert(
            pending.id,
            ActivePlayback {
                repeat: pending.looping.then(|| {
                    (
                        asset.clone(),
                        DecodeSettings {
                            decoder_threads: decode_settings.decoder_threads,
                            max_frame_delay: decode_settings.max_frame_delay,
                        },
                    )
                }),
                awaiting_restart: false,
                skip_blank_lead: pending.looping,
                frame_step: LAST_FRAME_HOLD,
                world,
                alpha_layout: asset.alpha_layout,
                id: pending.id,
                receiver: stream.video.clone(),
                frames: VecDeque::new(),
                surface: None,
                root,
                audio_entity,
                position: Duration::ZERO,
                paused: start_paused,
                started: false,
                decoder_ended: false,
                last_timestamp: Duration::ZERO,
                audio_clock: audio,
                age: Duration::ZERO,
                decoder: stream,
                fade_out: pending.fade_out,
                exit_elapsed: None,
            },
        );
    }
}

fn apply_video_controls(
    redraw: Option<Res<Messages<bevy::window::RequestRedraw>>>,
    mut commands: Commands,
    mut player: ResMut<VideoPlayer>,
    mut active: NonSendMut<ActiveVideo>,
    sinks: Query<&AudioSink>,
    mut events: MessageWriter<VideoEvent>,
    mut uploads: ResMut<VideoUploads>,
) {
    if !player.controls.is_empty() && redraw.is_some() {
        commands.write_message(bevy::window::RequestRedraw);
    }
    while let Some(control) = player.controls.pop_front() {
        match control {
            PlaybackControl::Pause(id) => {
                if let Some(playback) = active.0.get_mut(&id) {
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
                if let Some(playback) = active.0.get_mut(&id) {
                    playback.paused = false;
                    if let Some(audio_entity) = playback.audio_entity
                        && let Ok(sink) = sinks.get(audio_entity)
                    {
                        if playback.started && !player.suspended.contains(&id) {
                            sink.play();
                        }
                    }
                    player.states.insert(id, VideoPlaybackState::Playing);
                } else if player.pending.iter().any(|pending| pending.id == id) {
                    player.states.insert(id, VideoPlaybackState::Loading);
                }
            }
            PlaybackControl::Skip(id) => {
                if let Some(playback) = active.0.remove(&id) {
                    if let Some(audio_entity) = playback.audio_entity
                        && let Ok(sink) = sinks.get(audio_entity)
                    {
                        sink.stop();
                    }
                    cleanup_playback(&mut commands, &playback);
                    player.states.insert(id, VideoPlaybackState::Skipped);
                    if player.active == Some(id) {
                        player.active = None;
                    }
                    uploads.0.remove(&id);
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
    redraw: Option<Res<Messages<bevy::window::RequestRedraw>>>,
    mut commands: Commands,
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<Yuv420Material>>,
    mut nodes: Query<&mut Node>,
    sinks: Query<&AudioSink>,
    mut player: ResMut<VideoPlayer>,
    mut active: NonSendMut<ActiveVideo>,
    mut events: MessageWriter<VideoEvent>,
    mut uploads: ResMut<VideoUploads>,
    material_nodes: Query<(
        Option<&MaterialNode<Yuv420Material>>,
        Option<&MeshMaterial3d<Yuv420Material>>,
    )>,
    mut image_nodes: Query<&mut ImageNode>,
    mut backgrounds: Query<&mut BackgroundColor>,
    roots: Query<Entity>,
    mut audio_assets: ResMut<Assets<VideoAudio>>,
) {
    active.0.retain(|id, playback| {
        let paused = playback.paused || player.suspended.contains(id);
        if !roots.contains(playback.root) {
            if let Some(audio_entity) = playback.audio_entity
                && let Ok(sink) = sinks.get(audio_entity)
            {
                sink.stop();
            }
            cleanup_playback(&mut commands, playback);
            if player.active == Some(*id) {
                player.active = None;
            }
            player.states.insert(*id, VideoPlaybackState::Skipped);
            events.write(VideoEvent::Skipped { id: *id });
            return false;
        }
        let mut video_upload = uploads.0.entry(*id).or_default();
        playback.age += time.delta();
        if (!paused || !playback.started) && redraw.is_some() {
            commands.write_message(bevy::window::RequestRedraw);
        }
        playback.decoder.poll();
        if playback
            .audio_entity
            .is_some_and(|audio_entity| sinks.get(audio_entity).is_err())
            && playback.age >= AUDIO_SINK_TIMEOUT
        {
            let id = playback.id;
            cleanup_playback(&mut commands, playback);
            if player.active == Some(id) {
                player.active = None;
            }
            fail_playback(
                id,
                "Bevy did not create an audio sink for the movie".into(),
                &mut player,
                &mut events,
            );
            return false;
        }
        if let Some(result) = drain_ready_frames(&playback.receiver, &mut playback.frames) {
            match result {
                Ok(()) => playback.decoder_ended = true,
                Err(error) => {
                    let id = playback.id;
                    cleanup_playback(&mut commands, playback);
                    if player.active == Some(id) {
                        player.active = None;
                    }
                    fail_playback(id, error, &mut player, &mut events);
                    return false;
                }
            }
        }
        if !playback.frames.is_empty() {
            playback.awaiting_restart = false;
        }
        if !playback.started && !playback.frames.is_empty() {
            playback.started = true;
            events.write(VideoEvent::Started { id: playback.id });
        }
        if playback.started {
            player.states.insert(
                playback.id,
                if paused {
                    VideoPlaybackState::Paused
                } else {
                    VideoPlaybackState::Playing
                },
            );
        }
        if playback.started && !paused && !playback.awaiting_restart {
            if let Some(audio_entity) = playback.audio_entity
                && let Ok(sink) = sinks.get(audio_entity)
                && sink.is_paused()
            {
                sink.play();
            }
            playback.position = playback
                .audio_entity
                .and_then(|audio_entity| sinks.get(audio_entity).ok())
                .map(|sink| {
                    if sink.empty() {
                        playback.position + time.delta()
                    } else {
                        playback.audio_clock.position()
                    }
                })
                .unwrap_or_else(|| playback.position + time.delta());
        }
        // Only the newest due frame can be visible this render tick. Do not create/update
        // surfaces or publish GPU uploads for frames that have already been superseded.
        let mut due_frame = None;
        while playback.frames.front().is_some_and(|frame| {
            Duration::from_micros(frame.timestamp.max(0) as u64) <= playback.position
        }) {
            let frame = playback
                .frames
                .pop_front()
                .expect("the checked frame queue must not be empty");
            let timestamp = Duration::from_micros(frame.timestamp.max(0) as u64);
            let step = timestamp.saturating_sub(playback.last_timestamp);
            if !step.is_zero() {
                playback.frame_step = step;
            }
            playback.last_timestamp = timestamp;
            due_frame = Some(frame);
        }
        if let Some(frame) = due_frame {
            // Packed-alpha AMV loops may contain a fully transparent encoder
            // lead frame. Presenting it replaces the held final frame with a
            // transparent surface and exposes the black backing each cycle.
            if !playback.skip_blank_lead
                || !packed_alpha_frame_is_blank(&frame, playback.alpha_layout)
            {
                playback.skip_blank_lead = false;
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
        }
        if paused
            && let Some(audio_entity) = playback.audio_entity
            && let Ok(sink) = sinks.get(audio_entity)
            && !sink.is_paused()
        {
            sink.pause();
        }
        let audio_finished = audio_completed(
            playback.audio_entity.is_some(),
            playback
                .audio_entity
                .and_then(|entity| sinks.get(entity).ok())
                .is_some_and(AudioSinkPlayback::empty),
        );
        if playback.decoder_ended
            && playback.frames.is_empty()
            && playback.position >= playback.last_timestamp + playback.frame_step
            && audio_finished
        {
            if paused {
                return true;
            }
            if let Some((asset, settings)) = &playback.repeat {
                match MediaDecoder::new(&asset.media, settings.clone()) {
                    Ok(stream) => {
                        if let Some(entity) = playback.audio_entity {
                            if let Ok(sink) = sinks.get(entity) {
                                sink.stop();
                            }
                            commands.entity(entity).try_despawn();
                        }
                        playback.audio_clock =
                            VideoAudio::new(stream.audio.clone(), asset.metadata);
                        playback.audio_entity = if asset.metadata.channels == 0 {
                            None
                        } else {
                            spawn_movie_audio(
                                &mut commands,
                                &mut audio_assets,
                                playback.audio_clock.clone(),
                            )
                        };
                        playback.receiver = stream.video.clone();
                        playback.decoder = stream;
                        playback.decoder_ended = false;
                        playback.position = Duration::ZERO;
                        playback.last_timestamp = Duration::ZERO;
                        playback.age = Duration::ZERO;
                        playback.awaiting_restart = true;
                        playback.skip_blank_lead = true;
                        // Keep the old surface and upload alive until frame zero
                        // is decoded: repetition must not introduce a blank frame.
                        return true;
                    }
                    Err(error) => {
                        cleanup_playback(&mut commands, playback);
                        if player.active == Some(*id) {
                            player.active = None;
                        }
                        fail_playback(*id, error.to_string(), &mut player, &mut events);
                        return false;
                    }
                }
            }
            let elapsed = playback.exit_elapsed.get_or_insert(Duration::ZERO);
            if !paused {
                *elapsed += time.delta();
            }
            let opacity = exit_opacity(*elapsed, playback.fade_out);
            if let Ok(mut background) = backgrounds.get_mut(playback.root) {
                if background.0.alpha() > 0.0 {
                    background.0.set_alpha(opacity);
                }
            }
            if let Some(surface) = &playback.surface {
                let image_entity = match surface {
                    VideoSurface::YuvI420 { image_entity, .. }
                    | VideoSurface::YuvNv12 { image_entity, .. }
                    | VideoSurface::Rgba { image_entity, .. } => *image_entity,
                };
                if let Ok((ui, world)) = material_nodes.get(image_entity)
                    && let Some(handle) =
                        ui.map(|node| &node.0).or_else(|| world.map(|mesh| &mesh.0))
                    && let Some(mut material) = materials.get_mut(handle)
                {
                    material.opacity = opacity;
                }
                if let Ok(mut image) = image_nodes.get_mut(image_entity) {
                    image.color.set_alpha(opacity);
                }
            }
            if opacity > 0.0 {
                return true;
            }
            let id = playback.id;
            cleanup_playback(&mut commands, playback);
            if player.active == Some(id) {
                player.active = None;
            }
            player.states.insert(id, VideoPlaybackState::Finished);
            events.write(VideoEvent::Finished { id });
            return false;
        }
        true
    });
    uploads.0.retain(|id, _| active.0.contains_key(id));
    let VideoPlayer {
        suspended, states, ..
    } = &mut *player;
    suspended.retain(|id| {
        matches!(
            states.get(id),
            Some(
                VideoPlaybackState::Loading
                    | VideoPlaybackState::Playing
                    | VideoPlaybackState::Paused
            )
        )
    });
}

fn exit_opacity(elapsed: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        0.0
    } else {
        (1.0 - elapsed.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0) as f32
    }
}

fn audio_completed(has_audio: bool, sink_empty: bool) -> bool {
    !has_audio || sink_empty
}

fn packed_alpha_frame_is_blank(frame: &DecodedFrame, layout: Option<crate::AlphaLayout>) -> bool {
    let Some(layout) = layout else { return false };
    let width = frame.width as usize;
    let height = frame.height as usize;
    if layout.display_size(frame.width, frame.height).is_none() {
        return false;
    }
    let (bytes, stride, channels): (&[u8], usize, usize) = match &frame.pixels {
        DecodedPixels::I420Planar { y, .. } => (y, width, 1),
        DecodedPixels::I420Strided {
            planes, y_stride, ..
        }
        | DecodedPixels::Nv12Strided {
            planes, y_stride, ..
        } => (planes, *y_stride as usize, 1),
        DecodedPixels::Rgba(rgba) => (rgba, width * 4, 4),
    };
    let (first_row, first_col, rows, cols) = match layout {
        crate::AlphaLayout::Vertical => (height / 2, 0, height / 2, width),
        crate::AlphaLayout::Horizontal => (0, width / 2, height, width / 2),
    };
    (first_row..first_row + rows).all(|row| {
        let start = row * stride + first_col * channels;
        let end = start + cols * channels;
        bytes
            .get(start..end)
            .is_some_and(|line| line.chunks_exact(channels).all(|pixel| pixel[0] <= 2))
    })
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
    let aspect_ratio = playback.display_aspect(frame.width, frame.height);
    let (y, u, v) = match frame.pixels {
        DecodedPixels::I420Planar { y, u, v } => {
            upload.clear();
            (y, u, v)
        }
        DecodedPixels::Rgba(rgba) => {
            upload.clear();
            if let Some(VideoSurface::Rgba {
                image,
                image_entity,
            }) = playback.surface.as_ref()
            {
                replace_rgba(
                    images,
                    image,
                    frame.width,
                    frame.height,
                    rgba,
                    playback.alpha_layout.is_some() || playback.world.is_some(),
                );
                if let Ok(mut node) = nodes.get_mut(*image_entity) {
                    node.aspect_ratio = Some(aspect_ratio);
                }
                return;
            }
            replace_surface(commands, playback);
            let image = images.add(rgba_image(
                frame.width,
                frame.height,
                rgba,
                playback.alpha_layout.is_some() || playback.world.is_some(),
            ));
            let image_entity = if playback.world.is_some() {
                let material = materials.add(Yuv420Material {
                    opacity: 1.0,
                    alpha_layout: playback.alpha_layout,
                    rgba: true,
                    y: image.clone(),
                    chroma0: image.clone(),
                    chroma1: image.clone(),
                    color_transform: frame.color_transform.into(),
                    transfer: frame.transfer,
                    format: YuvPixelFormat::I420,
                });
                spawn_video_surface(
                    commands,
                    playback.root,
                    playback.world.as_ref(),
                    material,
                    aspect_ratio,
                )
            } else {
                commands
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
                    .id()
            };
            if playback.alpha_layout.is_some() && playback.world.is_none() {
                let material = materials.add(Yuv420Material {
                    opacity: 1.0,
                    alpha_layout: playback.alpha_layout,
                    rgba: true,
                    y: image.clone(),
                    chroma0: image.clone(),
                    chroma1: image.clone(),
                    color_transform: frame.color_transform.into(),
                    transfer: frame.transfer,
                    format: YuvPixelFormat::I420,
                });
                commands
                    .entity(image_entity)
                    .remove::<ImageNode>()
                    .insert(MaterialNode(material));
            }
            if playback.world.is_none() {
                commands.entity(playback.root).add_child(image_entity);
            }
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
        opacity: 1.0,
        alpha_layout: playback.alpha_layout,
        rgba: false,
        y: y_image.clone(),
        chroma0: u_image.clone(),
        chroma1: v_image.clone(),
        color_transform: frame.color_transform.into(),
        transfer: frame.transfer,
        format: YuvPixelFormat::I420,
    });
    let image_entity = spawn_video_surface(
        commands,
        playback.root,
        playback.world.as_ref(),
        material,
        aspect_ratio,
    );
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
    let aspect_ratio = playback.display_aspect(frame.width, frame.height);
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
            opacity: 1.0,
            alpha_layout: playback.alpha_layout,
            rgba: false,
            y: y_image.clone(),
            chroma0: u_image.clone(),
            chroma1: v_image.clone(),
            color_transform: frame.color_transform.into(),
            transfer: frame.transfer,
            format: YuvPixelFormat::I420,
        });
        let image_entity = spawn_video_surface(
            commands,
            playback.root,
            playback.world.as_ref(),
            material,
            aspect_ratio,
        );
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

    upload.publish(frame, [y_image, u_image, v_image]);
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
    let aspect_ratio = playback.display_aspect(frame.width, frame.height);

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
            opacity: 1.0,
            alpha_layout: playback.alpha_layout,
            rgba: false,
            y: y_image.clone(),
            chroma0: uv_image.clone(),
            chroma1: dummy_image.clone(),
            color_transform: frame.color_transform.into(),
            transfer: frame.transfer,
            format: YuvPixelFormat::Nv12,
        });

        let image_entity = spawn_video_surface(
            commands,
            playback.root,
            playback.world.as_ref(),
            material,
            aspect_ratio,
        );

        playback.surface = Some(VideoSurface::YuvNv12 {
            y_image: y_image.clone(),
            uv_image: uv_image.clone(),
            image_entity,
        });

        (y_image, uv_image, image_entity)
    };

    if let Ok(mut node) = nodes.get_mut(image_entity) {
        node.aspect_ratio = Some(aspect_ratio);
    }

    upload.publish(frame, [y_image, uv_image, Handle::default()]);
}

fn spawn_video_surface(
    commands: &mut Commands,
    root: Entity,
    world: Option<&(Handle<Mesh>, RenderLayers)>,
    material: Handle<Yuv420Material>,
    aspect_ratio: f32,
) -> Entity {
    let entity = if let Some((mesh, layers)) = world {
        commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                layers.clone(),
                Transform::default(),
                Visibility::Inherited,
                Pickable::IGNORE,
            ))
            .id()
    } else {
        commands
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
            .id()
    };
    commands.entity(root).add_child(entity);
    entity
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
    shader_decodes_transfer: bool,
) {
    if let Some(mut image) = images.get_mut(handle) {
        let size = image.texture_descriptor.size;
        if size.width == width
            && size.height == height
            && image.texture_descriptor.format
                == if shader_decodes_transfer {
                    TextureFormat::Rgba8Unorm
                } else {
                    TextureFormat::Rgba8UnormSrgb
                }
        {
            image.data = Some(data);
        } else {
            *image = rgba_image(width, height, data, shader_decodes_transfer);
        }
    }
}

fn rgba_image(width: u32, height: u32, data: Vec<u8>, shader_decodes_transfer: bool) -> Image {
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        if shader_decodes_transfer {
            TextureFormat::Rgba8Unorm
        } else {
            TextureFormat::Rgba8UnormSrgb
        },
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_packed_alpha_lead_is_skipped_but_visible_frame_is_kept() {
        let frame = |alpha: u8| DecodedFrame {
            timestamp: 0,
            width: 4,
            height: 4,
            chroma_width: 2,
            chroma_height: 2,
            color_transform: hiraku_media::YuvColorTransform::from_luma_coefficients(
                0.2126, 0.0722, false,
            ),
            transfer: hiraku_media::TransferFunction::Srgb,
            pixels: DecodedPixels::I420Planar {
                y: [vec![60; 8], vec![alpha; 8]].concat(),
                u: vec![128; 4],
                v: vec![128; 4],
            },
        };
        assert!(packed_alpha_frame_is_blank(
            &frame(0),
            Some(crate::AlphaLayout::Vertical)
        ));
        assert!(!packed_alpha_frame_is_blank(
            &frame(255),
            Some(crate::AlphaLayout::Vertical)
        ));
        assert!(!packed_alpha_frame_is_blank(&frame(0), None));
    }

    #[test]
    fn suspension_does_not_replace_manual_pause_or_affect_other_requests() {
        let mut app = App::new();
        app.init_resource::<VideoPlayer>()
            .init_resource::<VideoUploads>()
            .insert_non_send(ActiveVideo::default())
            .add_message::<VideoEvent>()
            .add_systems(Update, apply_video_controls);
        let (first, second) = {
            let mut player = app.world_mut().resource_mut::<VideoPlayer>();
            let first = player.play(Handle::default());
            let second = player.play(Handle::default());
            assert!(player.set_suspended(first, true));
            assert!(player.suspended.contains(&first));
            assert!(!player.suspended.contains(&second));
            player.pause(first);
            (first, second)
        };
        app.update();
        {
            let mut player = app.world_mut().resource_mut::<VideoPlayer>();
            assert!(player.set_suspended(first, false));
            assert_eq!(player.state(first), Some(&VideoPlaybackState::Paused));
            assert_eq!(player.state(second), Some(&VideoPlaybackState::Loading));
            assert!(player.set_suspended(first, true));
            player.resume(first);
        }
        app.update();
        let mut player = app.world_mut().resource_mut::<VideoPlayer>();
        assert!(
            player.suspended.contains(&first),
            "manual resume cannot release the host clock"
        );
        player.states.insert(first, VideoPlaybackState::Finished);
        assert!(!player.set_suspended(first, true));
        assert!(!player.set_suspended(VideoPlaybackId(u64::MAX), true));
    }

    #[test]
    fn repetition_is_configured_per_request_without_changing_other_playbacks() {
        let mut player = VideoPlayer::default();
        let first = player.play(Handle::default());
        let second = player.play(Handle::default());
        assert!(player.set_looping(first, true));
        assert!(player.pending[0].looping);
        assert!(!player.pending[1].looping);
        assert!(player.set_looping(first, false));
        assert!(!player.pending[0].looping);
        assert!(!player.set_looping(VideoPlaybackId(second.0 + 1), true));
    }

    #[test]
    fn shader_decoded_rgba_uses_unorm_to_avoid_a_second_transfer_conversion() {
        let world = rgba_image(2, 2, vec![128; 16], true);
        assert_eq!(world.texture_descriptor.format, TextureFormat::Rgba8Unorm);
        let ui = rgba_image(2, 2, vec![128; 16], false);
        assert_eq!(ui.texture_descriptor.format, TextureFormat::Rgba8UnormSrgb);
    }

    #[test]
    fn world_surface_is_a_mesh_not_a_ui_node_and_keeps_host_layers() {
        let mut world = World::new();
        let root = world
            .spawn((Transform::default(), Visibility::Inherited))
            .id();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mesh = (Handle::<Mesh>::default(), RenderLayers::layer(7));
        let entity = {
            let mut commands = Commands::new(&mut queue, &world);
            spawn_video_surface(
                &mut commands,
                root,
                Some(&mesh),
                Handle::default(),
                16.0 / 9.0,
            )
        };
        queue.apply(&mut world);
        assert!(world.get::<Mesh3d>(entity).is_some());
        assert!(
            world
                .get::<MeshMaterial3d<Yuv420Material>>(entity)
                .is_some()
        );
        assert!(world.get::<Node>(entity).is_none());
        assert_eq!(world.get::<RenderLayers>(entity), Some(&mesh.1));
        assert_eq!(world.get::<ChildOf>(entity).expect("parent").parent(), root);
        world.despawn(root);
        assert!(world.get_entity(entity).is_err());
    }

    #[test]
    fn world_video_rejects_invalid_size_and_non_spatial_parent() {
        let mut app = control_app();
        app.add_systems(Update, start_pending_video);
        let parent = app.world_mut().spawn_empty().id();
        let id = {
            let mut player = app.world_mut().resource_mut::<VideoPlayer>();
            assert!(
                player
                    .play_world(Handle::default(), parent, Vec2::ZERO)
                    .is_err()
            );
            assert!(player.pending.is_empty());
            player
                .play_world(Handle::default(), parent, Vec2::new(16.0, 9.0))
                .expect("size")
        };
        app.update();
        assert!(matches!(
            app.world().resource::<VideoPlayer>().state(id),
            Some(VideoPlaybackState::Failed(_))
        ));
    }

    fn control_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<VideoAsset>()
            .init_resource::<Assets<VideoAudio>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<VideoPlayer>()
            .init_resource::<VideoUploads>()
            .init_resource::<VideoDecodeSettings>()
            .add_message::<VideoEvent>();
        app.insert_non_send(ActiveVideo::default());
        app
    }

    #[test]
    fn independent_parent_lifetime_is_checked_even_behind_fullscreen_queue() {
        let mut app = control_app();
        app.add_systems(Update, start_pending_video);
        let parent = app.world_mut().spawn(Node::default()).id();
        let (full, child) = {
            let mut player = app.world_mut().resource_mut::<VideoPlayer>();
            let full = player.play(Handle::default());
            player.active = Some(full);
            let child = player.play_in(Handle::default(), parent);
            (full, child)
        };
        app.world_mut().despawn(parent);
        app.update();
        let player = app.world().resource::<VideoPlayer>();
        assert_eq!(player.state(child), Some(&VideoPlaybackState::Skipped));
        assert_eq!(player.active(), Some(full));
        assert_eq!(player.pending.len(), 1);
    }

    #[test]
    fn cancelling_one_surface_preserves_other_requests_and_stop_all_cancels_everyone() {
        let mut app = control_app();
        app.add_systems(Update, apply_video_controls);
        let parent = app.world_mut().spawn(Node::default()).id();
        let (first, second, full) = {
            let mut player = app.world_mut().resource_mut::<VideoPlayer>();
            let first = player.play_in(Handle::default(), parent);
            let second = player.play_in(Handle::default(), parent);
            let full = player.play(Handle::default());
            player.skip(first);
            (first, second, full)
        };
        app.update();
        let player = app.world().resource::<VideoPlayer>();
        assert_eq!(player.state(first), Some(&VideoPlaybackState::Skipped));
        assert_eq!(player.state(second), Some(&VideoPlaybackState::Loading));
        assert_eq!(player.state(full), Some(&VideoPlaybackState::Loading));
        app.world_mut().resource_mut::<VideoPlayer>().stop_all();
        app.update();
        let player = app.world().resource::<VideoPlayer>();
        assert!(player.pending.is_empty());
        assert_eq!(player.state(second), Some(&VideoPlaybackState::Skipped));
        assert_eq!(player.state(full), Some(&VideoPlaybackState::Skipped));
    }

    #[test]
    fn silent_video_can_finish_and_exit_fade_has_exact_endpoints() {
        assert!(audio_completed(false, false));
        assert!(!audio_completed(true, false));
        assert!(audio_completed(true, true));
        assert_eq!(exit_opacity(Duration::ZERO, Duration::from_secs(1)), 1.0);
        assert_eq!(
            exit_opacity(Duration::from_millis(500), Duration::from_secs(1)),
            0.5
        );
        assert_eq!(
            exit_opacity(Duration::from_secs(2), Duration::from_secs(1)),
            0.0
        );
        assert_eq!(exit_opacity(Duration::ZERO, Duration::ZERO), 0.0);
        let mut player = VideoPlayer::default();
        let id = player.play_under_ui(Handle::default());
        assert!(player.set_fade_out(id, Duration::from_secs(1)));
        assert_eq!(
            player.pending.front().expect("queued movie").fade_out,
            Duration::from_secs(1)
        );
    }

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
