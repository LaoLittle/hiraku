//! A Rust WebCodecs-style API for platform-independent codec processing.
//!
//! Feed encoded chunks and poll decoded frames. Containers, asset loading,
//! playback clocks and rendering belong to consumers of this crate.

mod codec;
mod platform;
pub use codec::*;

use std::sync::Arc;

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
    pub timestamp: i64,
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

#[derive(Clone, Debug)]
pub struct AudioData {
    pub timestamp: i64,
    pub sample_rate: u32,
    pub number_of_channels: u16,
    /// Interleaved f32 PCM. Clone retains the same allocation.
    pub samples: Arc<[f32]>,
}

impl AudioData {
    pub fn number_of_frames(&self) -> usize {
        self.samples.len() / usize::from(self.number_of_channels.max(1))
    }
}
