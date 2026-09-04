mod bindgen;

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use crossbeam_channel::{Sender, unbounded};
use js_sys::{Float32Array, Promise, Uint8Array};
use symphonia::core::codecs::{
    audio::well_known as audio_codecs, video::well_known as video_codecs,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, PlaneLayout, VideoFrame,
    VideoFrameCopyToOptions, VideoMatrixCoefficients, VideoPixelFormat,
    VideoTransferCharacteristics,
};

use self::bindgen::{
    AudioData, AudioDataCopyToOptions, AudioDecoder, AudioDecoderConfig, AudioDecoderInit,
    AudioSampleFormat, EncodedAudioChunk, EncodedAudioChunkInit, EncodedAudioChunkType,
    EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkType, HardwareAcceleration,
    VideoDecoder, VideoDecoderConfig, VideoDecoderInit,
};
use crate::{
    AudioEvent, DecodeSettings, DecodeStream, EncodedMedia, TransferFunction, VideoEvent,
    VideoFrame as DecodedFrame, VideoPixels, YuvColorTransform, container::open_container,
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const VIDEO_DECODE_QUEUE_LIMIT: u32 = VIDEO_QUEUE_CAPACITY as u32;
const AUDIO_DECODE_QUEUE_LIMIT: u32 = 24;
const AV1_CODEC: &str = "av01.0.12M.08";
const OPUS_CODEC: &str = "opus";

struct FrameCopyState {
    queue: RefCell<VecDeque<VideoFrame>>,
    running: Cell<bool>,
    pending: Cell<u32>,
    first_timestamp: Cell<Option<f64>>,
    storage: RefCell<Option<Uint8Array>>,
    sender: Sender<VideoEvent>,
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
                    if self.sender.send(VideoEvent::Frame(frame)).is_ok() {
                        self.queued_frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) => {
                    let _ = self.sender.send(VideoEvent::Error(error));
                }
            }
        }
    }

    fn storage(&self, byte_len: u32) -> Uint8Array {
        let mut storage = self.storage.borrow_mut();
        if storage.as_ref().is_none_or(|storage| storage.length() < byte_len) {
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

pub(crate) struct DecoderHandle {
    video_decoder: Option<VideoDecoder>,
    audio_decoder: Option<AudioDecoder>,
    copy_state: Rc<FrameCopyState>,
    _video_output: Closure<dyn FnMut(VideoFrame)>,
    _video_error: Closure<dyn FnMut(JsValue)>,
    _audio_output: Closure<dyn FnMut(AudioData)>,
    _audio_error: Closure<dyn FnMut(JsValue)>,
}

impl Drop for DecoderHandle {
    fn drop(&mut self) {
        self.copy_state.cancellation.store(true, Ordering::Relaxed);
        if let Some(decoder) = self.video_decoder.take() {
            let _ = decoder.close();
        }
        if let Some(decoder) = self.audio_decoder.take() {
            let _ = decoder.close();
        }
    }
}

pub(crate) fn decode(media: &EncodedMedia, _settings: &DecodeSettings) -> DecodeStream {
    let (video_sender, video_receiver) = unbounded();
    let (audio_sender, audio_receiver) = unbounded();
    let cancellation = Arc::new(AtomicBool::new(false));
    let queued_frames = Arc::new(AtomicUsize::new(0));
    let copy_state = Rc::new(FrameCopyState {
        queue: RefCell::new(VecDeque::new()),
        running: Cell::new(false),
        pending: Cell::new(0),
        first_timestamp: Cell::new(None),
        storage: RefCell::new(None),
        sender: video_sender.clone(),
        cancellation: cancellation.clone(),
        queued_frames: queued_frames.clone(),
    });

    let output_state = copy_state.clone();
    let video_output = Closure::wrap(Box::new(move |frame: VideoFrame| {
        output_state.enqueue(frame);
    }) as Box<dyn FnMut(VideoFrame)>);
    let video_error_sender = video_sender.clone();
    let video_error = Closure::wrap(Box::new(move |error: JsValue| {
        let _ = video_error_sender.send(VideoEvent::Error(format!(
            "WebCodecs AV1 decode failed: {}",
            js_error(&error)
        )));
    }) as Box<dyn FnMut(JsValue)>);

    let audio_output_sender = audio_sender.clone();
    let audio_output = Closure::wrap(Box::new(move |data: AudioData| {
        match audio_data_to_pcm(&data) {
            Ok(samples) => {
                let _ = audio_output_sender.send(AudioEvent::Samples(samples));
            }
            Err(error) => {
                let _ = audio_output_sender.send(AudioEvent::Error(error));
            }
        }
        data.close();
    }) as Box<dyn FnMut(AudioData)>);
    let audio_error_sender = audio_sender.clone();
    let audio_error = Closure::wrap(Box::new(move |error: JsValue| {
        let _ = audio_error_sender.send(AudioEvent::Error(format!(
            "WebCodecs Opus decode failed: {}",
            js_error(&error)
        )));
    }) as Box<dyn FnMut(JsValue)>);

    let video_decoder = create_video_decoder(media, &video_output, &video_error)
        .inspect_err(|error| {
            let _ = video_sender.send(VideoEvent::Error(error.clone()));
        })
        .ok();
    let audio_decoder = create_audio_decoder(media, &audio_output, &audio_error)
        .inspect_err(|error| {
            let _ = audio_sender.send(AudioEvent::Error(error.clone()));
        })
        .ok();

    if let (Some(video_decoder), Some(audio_decoder)) =
        (video_decoder.clone(), audio_decoder.clone())
    {
        let bytes = media.bytes.clone();
        let worker_cancellation = cancellation.clone();
        let worker_queued_frames = queued_frames.clone();
        let worker_copy_state = copy_state.clone();
        spawn_local(async move {
            match feed_packets(
                bytes,
                &video_decoder,
                &audio_decoder,
                &worker_cancellation,
                &worker_queued_frames,
                &worker_copy_state,
            )
            .await
            {
                Ok(()) if !worker_cancellation.load(Ordering::Relaxed) => {
                    let _ = video_sender.send(VideoEvent::End);
                    let _ = audio_sender.send(AudioEvent::End);
                }
                Err(error) if !worker_cancellation.load(Ordering::Relaxed) => {
                    let _ = video_sender.send(VideoEvent::Error(error.clone()));
                    let _ = audio_sender.send(AudioEvent::Error(error));
                }
                _ => {}
            }
        });
    }

    DecodeStream {
        video: video_receiver,
        audio: audio_receiver,
        metadata: media.metadata,
        cancellation,
        queued_frames: Some(queued_frames),
        handle: crate::DecoderHandle(super::DecoderHandle::WebCodecs(
            DecoderHandle {
                video_decoder,
                audio_decoder,
                copy_state,
                _video_output: video_output,
                _video_error: video_error,
                _audio_output: audio_output,
                _audio_error: audio_error,
            }
        )),
    }
}

fn create_video_decoder(
    media: &EncodedMedia,
    output: &Closure<dyn FnMut(VideoFrame)>,
    error: &Closure<dyn FnMut(JsValue)>,
) -> Result<VideoDecoder, String> {
    let init = VideoDecoderInit::new(error.as_ref().unchecked_ref(), output.as_ref().unchecked_ref());
    let decoder = VideoDecoder::new(&init)
        .map_err(|error| format!("WebCodecs AV1 decoder is unavailable: {}", js_error(&error)))?;
    let config = VideoDecoderConfig::new(AV1_CODEC);
    config.set_coded_width(media.metadata.width);
    config.set_coded_height(media.metadata.height);
    config.set_hardware_acceleration(HardwareAcceleration::PreferHardware);
    config.set_optimize_for_latency(true);
    decoder
        .configure(&config)
        .map_err(|error| format!("failed to configure WebCodecs AV1: {}", js_error(&error)))?;
    Ok(decoder)
}

fn create_audio_decoder(
    media: &EncodedMedia,
    output: &Closure<dyn FnMut(AudioData)>,
    error: &Closure<dyn FnMut(JsValue)>,
) -> Result<AudioDecoder, String> {
    let init = AudioDecoderInit::new(error.as_ref().unchecked_ref(), output.as_ref().unchecked_ref());
    let decoder = AudioDecoder::new(&init)
        .map_err(|error| format!("WebCodecs Opus decoder is unavailable: {}", js_error(&error)))?;
    let config = AudioDecoderConfig::new(
        OPUS_CODEC,
        u32::from(media.metadata.channels),
        media.metadata.sample_rate,
    );
    decoder
        .configure(&config)
        .map_err(|error| format!("failed to configure WebCodecs Opus: {}", js_error(&error)))?;
    Ok(decoder)
}

fn audio_data_to_pcm(data: &AudioData) -> Result<Vec<f32>, String> {
    let options = AudioDataCopyToOptions::new(0);
    options.set_format(AudioSampleFormat::F32);
    let byte_len = data
        .allocation_size(&options)
        .map_err(|error| format!("WebCodecs PCM allocation failed: {}", js_error(&error)))?;
    if byte_len % 4 != 0 {
        return Err(format!("WebCodecs returned a non-f32-aligned PCM size ({byte_len} bytes)"));
    }
    let storage = Float32Array::new_with_length(byte_len / 4);
    data.copy_to_with_buffer_source(storage.unchecked_ref(), &options)
        .map_err(|error| format!("WebCodecs PCM copy failed: {}", js_error(&error)))?;
    let mut samples = vec![0.0; storage.length() as usize];
    storage.copy_to(&mut samples);
    Ok(samples)
}

async fn feed_packets(
    bytes: Arc<[u8]>,
    video_decoder: &VideoDecoder,
    audio_decoder: &AudioDecoder,
    cancellation: &AtomicBool,
    queued_frames: &AtomicUsize,
    copy_state: &FrameCopyState,
) -> Result<(), String> {
    let mut format = open_container(bytes, "webm").map_err(|error| error.to_string())?;
    let mut video_track = None;
    let mut audio_track = None;
    let mut video_time_base = (1, 1);
    let mut audio_time_base = (1, 1);
    for track in format.tracks() {
        let Some(parameters) = &track.codec_params else { continue };
        if let Some(video) = parameters.video()
            && video.codec == video_codecs::CODEC_ID_AV1
        {
            video_track = Some(track.id);
            if let Some(base) = track.time_base {
                video_time_base = (base.numer.get(), base.denom.get());
            }
        }
        if let Some(audio) = parameters.audio()
            && audio.codec == audio_codecs::CODEC_ID_OPUS
        {
            audio_track = Some(track.id);
            if let Some(base) = track.time_base {
                audio_time_base = (base.numer.get(), base.denom.get());
            }
        }
    }
    let video_track = video_track.ok_or_else(|| "AV1 video track disappeared".to_string())?;
    let audio_track = audio_track.ok_or_else(|| "Opus audio track disappeared".to_string())?;
    let mut first_video_packet = true;

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Ok(());
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => return Err(format!("Matroska demux failed: {error}")),
        };
        if packet.track_id == video_track {
            while video_decoder.decode_queue_size() + copy_state.pending.get() >= VIDEO_DECODE_QUEUE_LIMIT
                || queued_frames.load(Ordering::Relaxed) >= VIDEO_QUEUE_CAPACITY
            {
                yield_to_browser().await?;
                if cancellation.load(Ordering::Relaxed) { return Ok(()) }
            }
            let data = Uint8Array::from(packet.data.as_ref());
            let kind = if first_video_packet { EncodedVideoChunkType::Key } else { EncodedVideoChunkType::Delta };
            let init = EncodedVideoChunkInit::new(data.unchecked_ref(), 0, kind);
            init.set_timestamp_f64(timestamp_micros_signed(packet.pts.get(), video_time_base));
            init.set_duration_f64(timestamp_micros(packet.dur.get(), video_time_base));
            let chunk = EncodedVideoChunk::new(&init)
                .map_err(|error| format!("failed to create WebCodecs AV1 packet: {}", js_error(&error)))?;
            video_decoder.decode(&chunk)
                .map_err(|error| format!("WebCodecs rejected AV1 packet: {}", js_error(&error)))?;
            first_video_packet = false;
        } else if packet.track_id == audio_track {
            while audio_decoder.decode_queue_size() >= AUDIO_DECODE_QUEUE_LIMIT {
                yield_to_browser().await?;
                if cancellation.load(Ordering::Relaxed) { return Ok(()) }
            }
            let data = Uint8Array::from(packet.data.as_ref());
            let init = EncodedAudioChunkInit::new(data.unchecked_ref(), 0, EncodedAudioChunkType::Key);
            init.set_timestamp_f64(timestamp_micros_signed(packet.pts.get(), audio_time_base));
            init.set_duration_f64(timestamp_micros(packet.dur.get(), audio_time_base));
            let chunk = EncodedAudioChunk::new(&init)
                .map_err(|error| format!("failed to create WebCodecs Opus packet: {}", js_error(&error)))?;
            audio_decoder.decode(&chunk)
                .map_err(|error| format!("WebCodecs rejected Opus packet: {}", js_error(&error)))?;
        }
    }

    JsFuture::from(video_decoder.flush()).await
        .map_err(|error| format!("WebCodecs video flush failed: {}", js_error(&error)))?;
    JsFuture::from(audio_decoder.flush()).await
        .map_err(|error| format!("WebCodecs audio flush failed: {}", js_error(&error)))?;
    while copy_state.pending.get() != 0 { yield_to_browser().await?; }
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
        Some(VideoMatrixCoefficients::Smpte170m | VideoMatrixCoefficients::Bt470bg) => (0.299, 0.114),
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
    let pixels = if frame.format() == Some(VideoPixelFormat::I420) {
        match copy_i420(frame, state, width, height).await {
            Ok((y, u, v)) => VideoPixels::I420Planar { y, u, v },
            Err(_) => VideoPixels::Rgba(copy_rgba(frame, state, width, height).await?),
        }
    } else {
        VideoPixels::Rgba(copy_rgba(frame, state, width, height).await?)
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
    let y_len = width.checked_mul(height).ok_or_else(|| "WebCodecs Y plane size overflowed".to_string())?;
    let u_len = chroma_width.checked_mul(chroma_height).ok_or_else(|| "WebCodecs chroma plane size overflowed".to_string())?;
    let v_offset = y_len.checked_add(u_len).ok_or_else(|| "WebCodecs plane offset overflowed".to_string())?;
    let options = VideoFrameCopyToOptions::new();
    options.set_layout(&[
        PlaneLayout::new(0, width),
        PlaneLayout::new(y_len, chroma_width),
        PlaneLayout::new(v_offset, chroma_width),
    ]);
    let byte_len = frame.allocation_size_with_options(&options)
        .map_err(|error| format!("WebCodecs I420 allocation failed: {}", js_error(&error)))?;
    let storage = state.storage(byte_len);
    JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options)).await
        .map_err(|error| format!("WebCodecs I420 copy failed: {}", js_error(&error)))?;
    Ok((
        uint8_array_to_vec(&storage.subarray(0, y_len)),
        uint8_array_to_vec(&storage.subarray(y_len, v_offset)),
        uint8_array_to_vec(&storage.subarray(v_offset, byte_len)),
    ))
}

async fn copy_rgba(frame: &VideoFrame, state: &FrameCopyState, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let options = VideoFrameCopyToOptions::new();
    options.set_format(VideoPixelFormat::Rgba);
    options.set_layout(&[PlaneLayout::new(0, width.saturating_mul(4))]);
    if let Ok(byte_len) = frame.allocation_size_with_options(&options) {
        let storage = state.storage(byte_len);
        if JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options)).await.is_ok() {
            return Ok(uint8_array_to_vec(&storage));
        }
    }
    copy_rgba_with_canvas(frame, width, height)
}

fn copy_rgba_with_canvas(frame: &VideoFrame, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let document = web_sys::window().and_then(|window| window.document())
        .ok_or_else(|| "browser document is unavailable".to_string())?;
    let canvas: HtmlCanvasElement = document.create_element("canvas")
        .map_err(|error| format!("failed to create fallback canvas: {}", js_error(&error)))?
        .dyn_into().map_err(|_| "fallback canvas element has the wrong type".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context: CanvasRenderingContext2d = canvas.get_context("2d")
        .map_err(|error| format!("failed to get fallback canvas: {}", js_error(&error)))?
        .ok_or_else(|| "fallback 2D canvas is unavailable".to_string())?
        .dyn_into().map_err(|_| "fallback canvas context has the wrong type".to_string())?;
    context.draw_image_with_video_frame_and_dw_and_dh(frame, 0.0, 0.0, width.into(), height.into())
        .map_err(|error| format!("fallback canvas draw failed: {}", js_error(&error)))?;
    context.get_image_data(0.0, 0.0, width as f64, height as f64)
        .map(|image| image.data().0)
        .map_err(|error| format!("fallback canvas read failed: {}", js_error(&error)))
}

async fn yield_to_browser() -> Result<(), String> {
    let promise = Promise::new(&mut |resolve, reject| {
        let Some(window) = web_sys::window() else {
            let _ = reject.call1(&JsValue::UNDEFINED, &JsValue::from_str("window is unavailable"));
            return;
        };
        if let Err(error) = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0) {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
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
