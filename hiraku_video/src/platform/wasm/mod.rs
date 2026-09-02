use std::{
    collections::VecDeque,
    iter::Empty,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::Asset,
    reflect::TypePath,
};
use crossbeam_channel::{Receiver, unbounded};
use js_sys::{Promise, Reflect, Uint8Array};
use symphonia::core::codecs::video::well_known as video_codecs;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::{
    VideoAsset, VideoDecodeSettings, VideoMetadata,
    asset::open_container,
    color::{TransferFunction, YuvColorTransform},
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const WEB_CODECS_QUEUE_LIMIT: u32 = VIDEO_QUEUE_CAPACITY as u32;

#[wasm_bindgen(module = "/src/platform/wasm/video_decoder.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn hirakuCreateWebVideoDecoder(
        codec: &str,
        width: u32,
        height: u32,
        on_frame: &js_sys::Function,
        on_error: &js_sys::Function,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    fn hirakuWebVideoDecode(
        decoder: &JsValue,
        data: &Uint8Array,
        timestamp: f64,
        duration: f64,
        key: bool,
    ) -> Result<(), JsValue>;
    fn hirakuWebVideoQueueSize(decoder: &JsValue) -> u32;
    fn hirakuWebVideoFlush(decoder: &JsValue) -> Promise;
    fn hirakuWebVideoClose(decoder: &JsValue);
    fn hirakuWebYield() -> Promise;
    fn hirakuCreateWebAudio(bytes: &Uint8Array) -> JsValue;
    fn hirakuWebAudioPlay(audio: &JsValue);
    fn hirakuWebAudioPause(audio: &JsValue);
    fn hirakuWebAudioPosition(audio: &JsValue) -> f64;
    fn hirakuWebAudioEnded(audio: &JsValue) -> bool;
    fn hirakuWebAudioClose(audio: &JsValue);
}

#[derive(Debug)]
pub(crate) struct DecodedFrame {
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub rgba: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(crate) enum DecodeEvent {
    Frame(DecodedFrame),
    End,
    Error(String),
}

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

pub(crate) struct WebPlaybackBackend {
    decoder: Option<JsValue>,
    audio: JsValue,
    _frame_callback: Closure<dyn FnMut(JsValue)>,
    _error_callback: Closure<dyn FnMut(JsValue)>,
}

impl WebPlaybackBackend {
    pub fn play(&self) {
        hirakuWebAudioPlay(&self.audio);
    }
    pub fn pause(&self) {
        hirakuWebAudioPause(&self.audio);
    }
    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(hirakuWebAudioPosition(&self.audio).max(0.0))
    }
    pub fn audio_ended(&self) -> bool {
        hirakuWebAudioEnded(&self.audio)
    }
}

impl Drop for WebPlaybackBackend {
    fn drop(&mut self) {
        if let Some(decoder) = self.decoder.take() {
            hirakuWebVideoClose(&decoder);
        }
        hirakuWebAudioClose(&self.audio);
    }
}

pub(crate) struct DecodeStream {
    pub video: Receiver<DecodeEvent>,
    pub audio: VideoAudio,
    pub cancellation: Arc<AtomicBool>,
    pub queued_frames: Option<Arc<AtomicUsize>>,
    pub web_backend: WebPlaybackBackend,
}

pub(crate) fn spawn_decoder(asset: &VideoAsset, _settings: &VideoDecodeSettings) -> DecodeStream {
    // The JavaScript side bounds decoder and pending-copy work. An unbounded
    // Rust channel ensures terminal and error events can never be dropped.
    let (sender, receiver) = unbounded();
    let cancellation = Arc::new(AtomicBool::new(false));
    let queued_frames = Arc::new(AtomicUsize::new(0));
    let frame_sender = sender.clone();
    let frame_cancellation = cancellation.clone();
    let callback_queued_frames = queued_frames.clone();
    let frame_callback = Closure::wrap(Box::new(move |value: JsValue| {
        if frame_cancellation.load(Ordering::Relaxed) {
            return;
        }
        match decoded_frame_from_js(&value) {
            Ok(frame) => {
                if frame_sender.send(DecodeEvent::Frame(frame)).is_ok() {
                    callback_queued_frames.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => {
                let _ = frame_sender.send(DecodeEvent::Error(error));
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    let error_sender = sender.clone();
    let error_callback = Closure::wrap(Box::new(move |value: JsValue| {
        let _ = error_sender.send(DecodeEvent::Error(format!(
            "WebCodecs AV1 decode failed: {}",
            js_error(&value)
        )));
    }) as Box<dyn FnMut(JsValue)>);

    let decoder = hirakuCreateWebVideoDecoder(
        "av01.0.12M.08",
        asset.metadata.width,
        asset.metadata.height,
        frame_callback.as_ref().unchecked_ref(),
        error_callback.as_ref().unchecked_ref(),
    );
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
    let media_bytes = Uint8Array::from(asset.bytes.as_ref());
    let audio = hirakuCreateWebAudio(&media_bytes);

    if let Some(decoder) = decoder.clone() {
        let bytes = asset.bytes.clone();
        let worker_cancellation = cancellation.clone();
        let worker_queued_frames = queued_frames.clone();
        spawn_local(async move {
            if let Err(error) =
                feed_video_packets(bytes, &decoder, &worker_cancellation, &worker_queued_frames)
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
        web_backend: WebPlaybackBackend {
            decoder,
            audio,
            _frame_callback: frame_callback,
            _error_callback: error_callback,
        },
    }
}

async fn feed_video_packets(
    bytes: Arc<[u8]>,
    decoder: &JsValue,
    cancellation: &AtomicBool,
    queued_frames: &AtomicUsize,
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
        while hirakuWebVideoQueueSize(decoder) >= WEB_CODECS_QUEUE_LIMIT
            || queued_frames.load(Ordering::Relaxed) >= VIDEO_QUEUE_CAPACITY
        {
            JsFuture::from(hirakuWebYield())
                .await
                .map_err(|error| format!("browser decode yield failed: {}", js_error(&error)))?;
            if cancellation.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        let timestamp = timestamp_micros_signed(packet.pts.get(), time_base);
        let duration = timestamp_micros(packet.dur.get(), time_base);
        let data = Uint8Array::from(packet.data.as_ref());
        hirakuWebVideoDecode(decoder, &data, timestamp, duration, first_packet)
            .map_err(|error| format!("WebCodecs rejected AV1 packet: {}", js_error(&error)))?;
        first_packet = false;
    }

    JsFuture::from(hirakuWebVideoFlush(decoder))
        .await
        .map_err(|error| format!("WebCodecs flush failed: {}", js_error(&error)))?;
    Ok(())
}

fn timestamp_micros(value: u64, time_base: (u32, u32)) -> f64 {
    value as f64 * f64::from(time_base.0) / f64::from(time_base.1) * 1_000_000.0
}

fn timestamp_micros_signed(value: i64, time_base: (u32, u32)) -> f64 {
    value.max(0) as f64 * f64::from(time_base.0) / f64::from(time_base.1) * 1_000_000.0
}

fn decoded_frame_from_js(value: &JsValue) -> Result<DecodedFrame, String> {
    let number = |name: &str| -> Result<f64, String> {
        Reflect::get(value, &JsValue::from_str(name))
            .map_err(|error| js_error(&error))?
            .as_f64()
            .ok_or_else(|| format!("WebCodecs frame field `{name}` is not numeric"))
    };
    let string = |name: &str| -> Option<String> {
        Reflect::get(value, &JsValue::from_str(name))
            .ok()
            .and_then(|value| value.as_string())
    };
    let bytes = |name: &str| -> Result<Vec<u8>, String> {
        let value =
            Reflect::get(value, &JsValue::from_str(name)).map_err(|error| js_error(&error))?;
        let array = Uint8Array::new(&value);
        let mut bytes = vec![0; array.length() as usize];
        array.copy_to(&mut bytes);
        Ok(bytes)
    };
    let optional_bytes = |name: &str| -> Option<Vec<u8>> {
        let value = Reflect::get(value, &JsValue::from_str(name)).ok()?;
        (!value.is_undefined() && !value.is_null()).then(|| {
            let array = Uint8Array::new(&value);
            let mut bytes = vec![0; array.length() as usize];
            array.copy_to(&mut bytes);
            bytes
        })
    };
    let width = number("width")? as u32;
    let height = number("height")? as u32;
    let full_range = Reflect::get(value, &JsValue::from_str("fullRange"))
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let (kr, kb) = match string("matrix").as_deref() {
        Some("smpte170m") | Some("bt470bg") => (0.299, 0.114),
        Some("bt2020-ncl") => (0.2627, 0.0593),
        _ => (0.2126, 0.0722),
    };
    let transfer = match string("transfer").as_deref() {
        Some("linear") => TransferFunction::Linear,
        Some("iec61966-2-1") => TransferFunction::Srgb,
        Some("gamma22curve") => TransferFunction::Gamma22,
        Some("gamma28curve") => TransferFunction::Gamma28,
        Some("pq") | Some("hlg") => {
            return Err(
                "HDR WebCodecs video requires tone mapping, which is not implemented".into(),
            );
        }
        _ => TransferFunction::Bt1886,
    };
    let rgba = optional_bytes("rgba");
    let (y, u, v) = if rgba.is_some() {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        (bytes("y")?, bytes("u")?, bytes("v")?)
    };
    Ok(DecodedFrame {
        timestamp: Duration::from_secs_f64((number("timestamp")? / 1_000_000.0).max(0.0)),
        width,
        height,
        chroma_width: width.div_ceil(2),
        chroma_height: height.div_ceil(2),
        color_transform: YuvColorTransform::from_luma_coefficients(kr, kb, !full_range),
        transfer,
        y,
        u,
        v,
        rgba,
    })
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
