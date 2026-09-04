//! Platform-independent media inspection and asynchronous decoding.
//!
//! This crate owns codecs and container parsing. Consumers receive timestamped
//! PCM and video frames and decide how to play or render them.

mod container;
mod platform;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Duration,
};

pub use container::{MediaError, inspect_media};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaMetadata {
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug)]
pub struct EncodedMedia {
    pub bytes: Arc<[u8]>,
    pub metadata: MediaMetadata,
}

impl EncodedMedia {
    pub fn inspect(bytes: impl Into<Arc<[u8]>>, extension: &str) -> Result<Self, MediaError> {
        let bytes = bytes.into();
        let metadata = inspect_media(&bytes, extension)?;
        Ok(Self { bytes, metadata })
    }
}

#[derive(Clone, Debug, Default)]
pub struct DecodeSettings {
    pub decoder_threads: Option<u32>,
    pub max_frame_delay: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransferFunction {
    Linear,
    Bt1886,
    Srgb,
    Gamma22,
    Gamma28,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum YuvPixelFormat {
    I420,
    Nv12,
}

#[derive(Clone, Copy, Debug)]
pub struct YuvColorTransform {
    pub row_r: [f32; 4],
    pub row_g: [f32; 4],
    pub row_b: [f32; 4],
}

impl YuvColorTransform {
    pub fn from_luma_coefficients(kr: f32, kb: f32, limited_range: bool) -> Self {
        let kg = 1.0 - kr - kb;
        let red_v = 2.0 * (1.0 - kr);
        let blue_u = 2.0 * (1.0 - kb);
        let green_u = -2.0 * kb * (1.0 - kb) / kg;
        let green_v = -2.0 * kr * (1.0 - kr) / kg;
        let (yo, ys, co, cs) = if limited_range {
            (16.0 / 255.0, 255.0 / 219.0, 128.0 / 255.0, 255.0 / 224.0)
        } else {
            (0.0, 1.0, 0.5, 1.0)
        };
        let offset = |u: f32, v: f32| -ys * yo - cs * co * (u + v);
        Self {
            row_r: [ys, 0.0, red_v * cs, offset(0.0, red_v)],
            row_g: [ys, green_u * cs, green_v * cs, offset(green_u, green_v)],
            row_b: [ys, blue_u * cs, 0.0, offset(blue_u, 0.0)],
        }
    }
}

#[derive(Debug)]
pub struct VideoFrame {
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
    pub pixels: VideoPixels,
}

#[derive(Debug)]
#[allow(dead_code, reason = "decoder backends produce different pixel layouts")]
pub enum VideoPixels {
    I420Strided {
        planes: Arc<[u8]>,
        u_offset: usize,
        v_offset: usize,
        y_stride: u32,
        chroma_stride: u32,
    },
    I420Planar {
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    },
    Nv12Strided {
        planes: Arc<[u8]>,
        uv_offset: usize,
        y_stride: u32,
        uv_stride: u32,
    },
    Rgba(Vec<u8>),
}

#[derive(Debug)]
pub enum VideoEvent {
    Frame(VideoFrame),
    End,
    Error(String),
}

#[derive(Debug)]
pub enum AudioEvent {
    Samples(Vec<f32>),
    End,
    Error(String),
}

pub struct DecodeStream {
    pub video: crossbeam_channel::Receiver<VideoEvent>,
    pub audio: crossbeam_channel::Receiver<AudioEvent>,
    pub metadata: MediaMetadata,
    pub cancellation: Arc<AtomicBool>,
    pub queued_frames: Option<Arc<AtomicUsize>>,
    pub handle: DecoderHandle,
}

#[allow(dead_code)]
pub struct DecoderHandle(platform::DecoderHandle);

pub fn decode(media: &EncodedMedia, settings: &DecodeSettings) -> DecodeStream {
    platform::decode(media, settings)
}
