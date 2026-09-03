mod bindgen;

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    iter::Empty,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::{Asset, Assets, Commands, Entity},
    reflect::TypePath,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use js_sys::{Array, Date, Promise, Uint8Array};
use symphonia::core::codecs::video::well_known as video_codecs;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use bindgen::{EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkType, HardwareAcceleration, VideoDecoder, VideoDecoderConfig, VideoDecoderInit};
use web_sys::{
    Blob, BlobPropertyBag, CanvasRenderingContext2d, HtmlAudioElement,
    HtmlCanvasElement, PlaneLayout, Url,
    VideoFrame, VideoFrameCopyToOptions, VideoMatrixCoefficients, VideoPixelFormat,
    VideoTransferCharacteristics,
};

use crate::{
    VideoAsset, VideoDecodeSettings, VideoMetadata,
    asset::open_container,
    color::{TransferFunction, YuvColorTransform},
    platform::{DecodeEvent, DecodedFrame, DecodedPixels},
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const WEB_CODECS_QUEUE_LIMIT: u32 = VIDEO_QUEUE_CAPACITY as u32;
const AV1_CODEC: &str = "av01.0.12M.08";

mod upload;
pub(crate) use upload::{VideoUpload, install_video_upload};

#[derive(Asset, Clone, TypePath)]
pub(crate) struct VideoAudio {
    metadata: VideoMetadata,
}

pub(crate) struct EmptyVideoAudio {
    samples: Empty<f32>,
    metadata: VideoMetadata,
}

impl Iterator for EmptyVideoAudio {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        self.samples.next()
    }
}

impl Source for EmptyVideoAudio {
    fn current_span_len(&self) -> Option<usize> {
        Some(0)
    }

    fn channels(&self) -> ChannelCount {
        std::num::NonZeroU16::new(self.metadata.channels)
            .expect("validated video channels are non-zero")
    }

    fn sample_rate(&self) -> SampleRate {
        std::num::NonZeroU32::new(self.metadata.sample_rate)
            .expect("validated video sample rate is non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::ZERO)
    }
}

impl Decodable for VideoAudio {
    type Decoder = EmptyVideoAudio;

    fn decoder(&self) -> Self::Decoder {
        EmptyVideoAudio {
            samples: std::iter::empty(),
            metadata: self.metadata,
        }
    }
}

struct WebAudio {
    element: HtmlAudioElement,
    object_url: String,
    fallback: Rc<Cell<bool>>,
    playing: Cell<bool>,
    clock_seconds: Cell<f64>,
    last_tick: Cell<f64>,
}

impl WebAudio {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        let bytes = Uint8Array::from(bytes);
        let parts = Array::new();
        parts.push(&bytes);
        let options = BlobPropertyBag::new();
        options.set_type("audio/webm; codecs=opus");
        let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|error| format!("failed to create movie audio blob: {}", js_error(&error)))?;
        let object_url = Url::create_object_url_with_blob(&blob).map_err(|error| {
            format!(
                "failed to create movie audio object URL: {}",
                js_error(&error)
            )
        })?;
        let element = HtmlAudioElement::new_with_src(&object_url).map_err(|error| {
            let _ = Url::revoke_object_url(&object_url);
            format!("failed to create movie audio element: {}", js_error(&error))
        })?;
        element.set_preload("auto");
        Ok(Self {
            element,
            object_url,
            fallback: Rc::new(Cell::new(false)),
            playing: Cell::new(false),
            clock_seconds: Cell::new(0.0),
            last_tick: Cell::new(Date::now()),
        })
    }

    fn play(&self) {
        if !self.playing.replace(true) {
            self.last_tick.set(Date::now());
        }
        let fallback = self.fallback.clone();
        match self.element.play() {
            Ok(promise) => spawn_local(async move {
                if JsFuture::from(promise).await.is_err() {
                    fallback.set(true);
                }
            }),
            Err(_) => self.fallback.set(true),
        }
    }

    fn pause(&self) {
        if self.playing.replace(false) {
            self.clock_seconds.set(
                self.clock_seconds.get() + (Date::now() - self.last_tick.get()) / 1_000.0,
            );
        }
        let _ = self.element.pause();
    }

    fn position(&self) -> Duration {
        let seconds = if !self.fallback.get() && self.element.current_time() > 0.0 {
            self.element.current_time()
        } else {
            self.clock_seconds.get()
                + if self.playing.get() {
                    (Date::now() - self.last_tick.get()) / 1_000.0
                } else {
                    0.0
                }
        };
        Duration::from_secs_f64(seconds.max(0.0))
    }

    fn ended(&self) -> bool {
        self.fallback.get() || self.element.ended()
    }
}

impl Drop for WebAudio {
    fn drop(&mut self) {
        let _ = self.element.pause();
        self.element.set_src("");
        self.element.load();
        let _ = Url::revoke_object_url(&self.object_url);
    }
}

struct FrameCopyState {
    queue: RefCell<VecDeque<VideoFrame>>,
    running: Cell<bool>,
    pending: Cell<u32>,
    first_timestamp: Cell<Option<f64>>,
    storage: RefCell<Option<Uint8Array>>,
    sender: Sender<DecodeEvent>,
    cancellation: Arc<AtomicBool>,
    queued_frames: Arc<AtomicUsize>,
}

impl FrameCopyState {
    fn enqueue(self: &Rc<Self>, frame: VideoFrame) {
        self.pending.set(self.pending.get().saturating_add(1));
        self.queue.borrow_mut().push_back(frame);
        if !self.running.replace(true) {
            let state = self.clone();
            spawn_local(async move { state.process().await });
        }
    }

    async fn process(self: Rc<Self>) {
        loop {
            let Some(frame) = self.queue.borrow_mut().pop_front() else {
                self.running.set(false);
                return;
            };
            if self.cancellation.load(Ordering::Relaxed) {
                frame.close();
                self.pending.set(self.pending.get().saturating_sub(1));
                continue;
            }
            let decoded = decode_frame(&frame, &self).await;
            frame.close();
            self.pending.set(self.pending.get().saturating_sub(1));
            match decoded {
                Ok(frame) => {
                    if self.sender.send(DecodeEvent::Frame(frame)).is_ok() {
                        self.queued_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) => {
                    let _ = self.sender.send(DecodeEvent::Error(error));
                }
            }
        }
    }

    fn storage(&self, byte_len: u32) -> Uint8Array {
        let mut storage = self.storage.borrow_mut();
        if storage
            .as_ref()
            .is_none_or(|storage| storage.length() < byte_len)
        {
            *storage = Some(Uint8Array::new_with_length(byte_len));
        }
        storage
            .as_ref()
            .expect("frame storage must be allocated")
            .subarray(0, byte_len)
    }
}

impl Drop for FrameCopyState {
    fn drop(&mut self) {
        for frame in self.queue.get_mut().drain(..) {
            frame.close();
        }
    }
}

pub(crate) struct PlaybackBackend {
    decoder: Option<VideoDecoder>,
    audio: Option<WebAudio>,
    copy_state: Rc<FrameCopyState>,
    _output_callback: Closure<dyn FnMut(VideoFrame)>,
    _error_callback: Closure<dyn FnMut(JsValue)>,
}

impl PlaybackBackend {
    pub fn play(&self) {
        if let Some(audio) = &self.audio {
            audio.play();
        }
    }

    pub fn pause(&self) {
        if let Some(audio) = &self.audio {
            audio.pause();
        }
    }

    pub fn position(&self) -> Option<Duration> {
        Some(self.audio
            .as_ref()
            .map(WebAudio::position)
            .unwrap_or(Duration::ZERO))
    }

    pub fn audio_ended(&self) -> Option<bool> {
        Some(self.audio.as_ref().is_none_or(WebAudio::ended))
    }

    pub fn requires_bevy_audio(&self) -> bool {
        false
    }
}

impl Drop for PlaybackBackend {
    fn drop(&mut self) {
        self.copy_state.cancellation.store(true, Ordering::Relaxed);
        if let Some(decoder) = self.decoder.take() {
            let _ = decoder.close();
        }
    }
}

pub(crate) struct DecodeStream {
    pub video: Receiver<DecodeEvent>,
    pub audio: VideoAudio,
    pub cancellation: Arc<AtomicBool>,
    pub queued_frames: Option<Arc<AtomicUsize>>,
    pub backend: PlaybackBackend,
}

pub(crate) fn spawn_decoder(asset: &VideoAsset, _settings: &VideoDecodeSettings) -> DecodeStream {
    let (sender, receiver) = unbounded();
    let cancellation = Arc::new(AtomicBool::new(false));
    let queued_frames = Arc::new(AtomicUsize::new(0));
    let copy_state = Rc::new(FrameCopyState {
        queue: RefCell::new(VecDeque::new()),
        running: Cell::new(false),
        pending: Cell::new(0),
        first_timestamp: Cell::new(None),
        storage: RefCell::new(None),
        sender: sender.clone(),
        cancellation: cancellation.clone(),
        queued_frames: queued_frames.clone(),
    });

    let output_state = copy_state.clone();
    let output_callback = Closure::wrap(Box::new(move |frame: VideoFrame| {
        output_state.enqueue(frame);
    }) as Box<dyn FnMut(VideoFrame)>);
    let error_sender = sender.clone();
    let error_callback = Closure::wrap(Box::new(move |error: JsValue| {
        let _ = error_sender.send(DecodeEvent::Error(format!(
            "WebCodecs AV1 decode failed: {}",
            js_error(&error)
        )));
    }) as Box<dyn FnMut(JsValue)>);

    let init = VideoDecoderInit::new(
        error_callback.as_ref().unchecked_ref(),
        output_callback.as_ref().unchecked_ref(),
    );
    let decoder = VideoDecoder::new(&init).and_then(|decoder| {
        let config = VideoDecoderConfig::new(AV1_CODEC);
        config.set_coded_width(asset.metadata.width);
        config.set_coded_height(asset.metadata.height);
        config.set_hardware_acceleration(HardwareAcceleration::PreferHardware);
        config.set_optimize_for_latency(true);
        decoder.configure(&config)?;
        Ok(decoder)
    });
    let decoder = match decoder {
        Ok(decoder) => Some(decoder),
        Err(error) => {
            let _ = sender.send(DecodeEvent::Error(format!(
                "WebCodecs AV1 decoder is unavailable: {}",
                js_error(&error)
            )));
            None
        }
    };
    let audio = match WebAudio::new(asset.bytes.as_ref()) {
        Ok(audio) => Some(audio),
        Err(error) => {
            let _ = sender.send(DecodeEvent::Error(error));
            None
        }
    };

    if let Some(decoder) = decoder.clone() {
        let bytes = asset.bytes.clone();
        let worker_cancellation = cancellation.clone();
        let worker_queued_frames = queued_frames.clone();
        let worker_copy_state = copy_state.clone();
        spawn_local(async move {
            if let Err(error) = feed_video_packets(
                bytes,
                &decoder,
                &worker_cancellation,
                &worker_queued_frames,
                &worker_copy_state,
            )
            .await
            {
                if !worker_cancellation.load(Ordering::Relaxed) {
                    let _ = sender.send(DecodeEvent::Error(error));
                }
            } else if !worker_cancellation.load(Ordering::Relaxed) {
                let _ = sender.send(DecodeEvent::End);
            }
        });
    }

    DecodeStream {
        video: receiver,
        audio: VideoAudio {
            metadata: asset.metadata,
        },
        cancellation,
        queued_frames: Some(queued_frames),
        backend: PlaybackBackend {
            decoder,
            audio,
            copy_state,
            _output_callback: output_callback,
            _error_callback: error_callback,
        },
    }
}

pub(crate) fn spawn_movie_audio(
    _commands: &mut Commands,
    _audio_assets: &mut Assets<VideoAudio>,
    _audio: VideoAudio,
) -> Option<Entity> {
    None
}

async fn feed_video_packets(
    bytes: Arc<[u8]>,
    decoder: &VideoDecoder,
    cancellation: &AtomicBool,
    queued_frames: &AtomicUsize,
    copy_state: &FrameCopyState,
) -> Result<(), String> {
    let mut format = open_container(bytes, "webm").map_err(|error| error.to_string())?;
    let video_track = format
        .tracks()
        .iter()
        .find(|track| {
            track
                .codec_params
                .as_ref()
                .and_then(|parameters| parameters.video())
                .is_some_and(|video| video.codec == video_codecs::CODEC_ID_AV1)
        })
        .ok_or_else(|| "AV1 video track disappeared".to_string())?;
    let track_id = video_track.id;
    let time_base = video_track
        .time_base
        .map(|base| (base.numer.get(), base.denom.get()))
        .unwrap_or((1, 1));
    let mut first_packet = true;

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Ok(());
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => return Err(format!("Matroska demux failed: {error}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        while decoder.decode_queue_size() + copy_state.pending.get() >= WEB_CODECS_QUEUE_LIMIT
            || queued_frames.load(Ordering::Relaxed) >= VIDEO_QUEUE_CAPACITY
        {
            yield_to_browser().await?;
            if cancellation.load(Ordering::Relaxed) {
                return Ok(());
            }
        }

        let timestamp = timestamp_micros_signed(packet.pts.get(), time_base);
        let duration = timestamp_micros(packet.dur.get(), time_base);
        let data = Uint8Array::from(packet.data.as_ref());
        let kind = if first_packet {
            EncodedVideoChunkType::Key
        } else {
            EncodedVideoChunkType::Delta
        };
        let init = EncodedVideoChunkInit::new(data.unchecked_ref(), 0, kind);
        init.set_timestamp_f64(timestamp);
        init.set_duration_f64(duration);
        let chunk = EncodedVideoChunk::new(&init).map_err(|error| {
            format!(
                "failed to create WebCodecs AV1 packet: {}",
                js_error(&error)
            )
        })?;
        decoder
            .decode(&chunk)
            .map_err(|error| format!("WebCodecs rejected AV1 packet: {}", js_error(&error)))?;
        first_packet = false;
    }

    JsFuture::from(decoder.flush())
        .await
        .map_err(|error| format!("WebCodecs flush failed: {}", js_error(&error)))?;
    while copy_state.pending.get() != 0 {
        yield_to_browser().await?;
    }
    Ok(())
}

async fn decode_frame(frame: &VideoFrame, state: &FrameCopyState) -> Result<DecodedFrame, String> {
    let width = frame.coded_width();
    let height = frame.coded_height();
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let timestamp = frame.timestamp();
    let first_timestamp = state.first_timestamp.get().unwrap_or_else(|| {
        state.first_timestamp.set(Some(timestamp));
        timestamp
    });
    let color_space = frame.color_space();
    let full_range = color_space.full_range().unwrap_or(false);
    let (kr, kb) = match color_space.matrix() {
        Some(VideoMatrixCoefficients::Smpte170m | VideoMatrixCoefficients::Bt470bg) => {
            (0.299, 0.114)
        }
        Some(VideoMatrixCoefficients::Bt2020Ncl) => (0.2627, 0.0593),
        _ => (0.2126, 0.0722),
    };
    let transfer = match color_space.transfer() {
        Some(VideoTransferCharacteristics::Linear) => TransferFunction::Linear,
        Some(VideoTransferCharacteristics::Iec6196621) => TransferFunction::Srgb,
        Some(VideoTransferCharacteristics::Pq | VideoTransferCharacteristics::Hlg) => {
            return Err("HDR WebCodecs video requires tone mapping, which is not implemented".into());
        }
        _ => TransferFunction::Bt1886,
    };

    let (y, u, v, rgba) = if frame.format() == Some(VideoPixelFormat::I420) {
        match copy_i420(frame, state, width, height).await {
            Ok((y, u, v)) => (y, u, v, None),
            Err(_) => (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(copy_rgba(frame, state, width, height).await?),
            ),
        }
    } else {
        (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(copy_rgba(frame, state, width, height).await?),
        )
    };

    let pixels = match rgba {
        Some(rgba) => DecodedPixels::Rgba(rgba),
        None => DecodedPixels::PlanarYuv { y, u, v },
    };
    Ok(DecodedFrame {
        timestamp: Duration::from_secs_f64(((timestamp - first_timestamp) / 1_000_000.0).max(0.0)),
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform: YuvColorTransform::from_luma_coefficients(kr, kb, !full_range),
        transfer,
        pixels,
    })
}

async fn copy_i420(
    frame: &VideoFrame,
    state: &FrameCopyState,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let y_len = width
        .checked_mul(height)
        .ok_or_else(|| "WebCodecs Y plane size overflowed".to_string())?;
    let u_len = chroma_width
        .checked_mul(chroma_height)
        .ok_or_else(|| "WebCodecs chroma plane size overflowed".to_string())?;
    let v_offset = y_len
        .checked_add(u_len)
        .ok_or_else(|| "WebCodecs plane offset overflowed".to_string())?;
    let options = copy_options(frame)?;
    options.set_layout(&[
        PlaneLayout::new(0, width),
        PlaneLayout::new(y_len, chroma_width),
        PlaneLayout::new(v_offset, chroma_width),
    ]);
    let byte_len = frame
        .allocation_size_with_options(&options)
        .map_err(|error| format!("WebCodecs I420 allocation failed: {}", js_error(&error)))?;
    let storage = state.storage(byte_len);
    JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options))
        .await
        .map_err(|error| format!("WebCodecs I420 copy failed: {}", js_error(&error)))?;
    let y = uint8_array_to_vec(&storage.subarray(0, y_len));
    let u = uint8_array_to_vec(&storage.subarray(y_len, v_offset));
    let v = uint8_array_to_vec(&storage.subarray(v_offset, byte_len));
    Ok((y, u, v))
}

async fn copy_rgba(
    frame: &VideoFrame,
    state: &FrameCopyState,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let options = copy_options(frame)?;
    options.set_format(VideoPixelFormat::Rgba);
    options.set_layout(&[PlaneLayout::new(0, width.saturating_mul(4))]);
    if let Ok(byte_len) = frame.allocation_size_with_options(&options) {
        let storage = state.storage(byte_len);
        if JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options))
            .await
            .is_ok()
        {
            return Ok(uint8_array_to_vec(&storage));
        }
    }
    copy_rgba_with_canvas(frame, width, height)
}

fn copy_options(_frame: &VideoFrame) -> Result<VideoFrameCopyToOptions, String> {
    Ok(VideoFrameCopyToOptions::new())
}

fn copy_rgba_with_canvas(
    frame: &VideoFrame,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "browser document is unavailable".to_string())?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("failed to create fallback canvas: {}", js_error(&error)))?
        .dyn_into()
        .map_err(|_| "fallback canvas element has the wrong type".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|error| format!("failed to get fallback canvas: {}", js_error(&error)))?
        .ok_or_else(|| "fallback 2D canvas is unavailable".to_string())?
        .dyn_into()
        .map_err(|_| "fallback canvas context has the wrong type".to_string())?;
    context
        .draw_image_with_video_frame_and_dw_and_dh(frame, 0.0, 0.0, width.into(), height.into())
        .map_err(|error| format!("fallback canvas draw failed: {}", js_error(&error)))?;
    context
        .get_image_data(0.0, 0.0, width as f64, height as f64)
        .map(|image| image.data().0)
        .map_err(|error| format!("fallback canvas read failed: {}", js_error(&error)))
}

async fn yield_to_browser() -> Result<(), String> {
    let promise = Promise::new(&mut |resolve, reject| {
        let Some(window) = web_sys::window() else {
            let _ = reject.call1(&JsValue::UNDEFINED, &JsValue::from_str("window is unavailable"));
            return;
        };
        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
        {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
        }
    });
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|error| format!("browser decode yield failed: {}", js_error(&error)))
}

fn timestamp_micros(value: u64, time_base: (u32, u32)) -> f64 {
    value as f64 * f64::from(time_base.0) / f64::from(time_base.1) * 1_000_000.0
}

fn timestamp_micros_signed(value: i64, time_base: (u32, u32)) -> f64 {
    value.max(0) as f64 * f64::from(time_base.0) / f64::from(time_base.1) * 1_000_000.0
}

fn uint8_array_to_vec(array: &Uint8Array) -> Vec<u8> {
    let mut bytes = vec![0; array.length() as usize];
    array.copy_to(&mut bytes);
    bytes
}

fn js_error(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

pub(crate) fn drain_ready_frames(
    receiver: &Receiver<DecodeEvent>,
    queue: &mut VecDeque<DecodedFrame>,
) -> Option<Result<(), String>> {
    let mut terminal = None;
    while queue.len() < VIDEO_QUEUE_CAPACITY {
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
