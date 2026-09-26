//! A Rust WebCodecs-style API for platform-independent codec processing.
//!
//! Feed encoded chunks and poll decoded frames. Containers, asset loading,
//! playback clocks and rendering belong to consumers of this crate.

mod codec;
mod identifier;
pub use identifier::*;
mod platform;
pub use codec::*;
mod encoder;
pub use encoder::*;
mod audio_packet;
pub use audio_packet::AudioPacketDecoder;
mod color_space;
pub use color_space::*;
mod image;
pub use image::*;
mod data_api;
pub use data_api::*;

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
    Pq,
    Hlg,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum YuvPixelFormat {
    I420,
    Nv12,
}

#[derive(Clone, Copy, Debug)]
pub struct YuvColorTransform {
    /// Decoded luma to normalized coverage: `y * scale + offset`.
    /// Describes the output pixels, which a hardware decoder may range-convert.
    pub luma: [f32; 2],
    pub row_r: [f32; 4],
    pub row_g: [f32; 4],
    pub row_b: [f32; 4],
    /// Linear RGB conversion to the renderer's BT.709 working gamut.
    pub gamut: [[f32; 4]; 3],
}

impl YuvColorTransform {
    /// Primaries are independent of YUV matrix coefficients.
    pub fn with_bt2020_primaries(mut self, bt2020: bool) -> Self {
        self.gamut = if bt2020 {
            [
                [1.660491, -0.587641, -0.07285, 0.0],
                [-0.12455, 1.1329, -0.008349, 0.0],
                [-0.018151, -0.100579, 1.11873, 0.0],
            ]
        } else {
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]
        };
        self
    }
    pub fn from_luma_coefficients(kr: f32, kb: f32, limited_range: bool) -> Self {
        Self::from_luma_coefficients_depth(kr, kb, limited_range, 8)
    }

    /// Samples are normalized to their logical bit depth, independently of
    /// memory alignment (P010 stores the ten significant bits in bits 15..6).
    pub fn from_luma_coefficients_depth(
        kr: f32,
        kb: f32,
        limited_range: bool,
        bit_depth: u8,
    ) -> Self {
        let depth = bit_depth.clamp(8, 16);
        let max = ((1u32 << depth) - 1) as f32;
        let scale = (1u32 << (depth - 8)) as f32;
        let kg = 1.0 - kr - kb;
        let red_v = 2.0 * (1.0 - kr);
        let blue_u = 2.0 * (1.0 - kb);
        let green_u = -2.0 * kb * (1.0 - kb) / kg;
        let green_v = -2.0 * kr * (1.0 - kr) / kg;
        let (yo, ys, co, cs) = if limited_range {
            (
                16.0 * scale / max,
                max / (219.0 * scale),
                128.0 * scale / max,
                max / (224.0 * scale),
            )
        } else {
            // Eight-bit UNORM samples divide by 255, while neutral chroma is
            // code 128, not 127.5. Using 0.5 tints neutral pixels blue/magenta.
            (0.0, 1.0, 128.0 * scale / max, 1.0)
        };
        let offset = |u: f32, v: f32| -ys * yo - cs * co * (u + v);
        Self {
            luma: [ys, -ys * yo],
            row_r: [ys, 0.0, red_v * cs, offset(0.0, red_v)],
            row_g: [ys, green_u * cs, green_v * cs, offset(green_u, green_v)],
            row_b: [ys, blue_u * cs, 0.0, offset(blue_u, 0.0)],
            gamut: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
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

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn limited_range_neutral_black_and_white_at_every_depth() {
        for depth in [8, 10, 12] {
            let transform =
                YuvColorTransform::from_luma_coefficients_depth(0.2627, 0.0593, true, depth);
            let max = ((1u32 << depth) - 1) as f32;
            let scale = (1u32 << (depth - 8)) as f32;
            for (code, expected) in [(16.0, 0.0), (235.0, 1.0)] {
                let samples = [
                    code * scale / max,
                    128.0 * scale / max,
                    128.0 * scale / max,
                    1.0,
                ];
                for row in [transform.row_r, transform.row_g, transform.row_b] {
                    let actual: f32 = row.into_iter().zip(samples).map(|(a, b)| a * b).sum();
                    assert!((actual - expected).abs() < 0.00001);
                }
            }
            assert_eq!(
                transform.gamut[0],
                [1.0, 0.0, 0.0, 0.0],
                "matrix must not imply primaries"
            );
        }
    }
}

#[derive(Debug)]
#[allow(dead_code, reason = "decoder backends produce different pixel layouts")]
pub enum VideoPixels {
    /// Little-endian, low-bit-aligned planar samples. Chroma dimensions on
    /// VideoFrame describe 4:2:0, 4:2:2 or 4:4:4; strides are byte counts.
    Planar16 {
        planes: Arc<[u8]>,
        u_offset: usize,
        v_offset: usize,
        y_stride: u32,
        chroma_stride: u32,
        bit_depth: u8,
    },
    /// 10-bit little-endian semi-planar Y/UV, significant bits in bits 15..6.
    P010 {
        planes: Arc<[u8]>,
        uv_offset: usize,
        y_stride: u32,
        uv_stride: u32,
    },
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
