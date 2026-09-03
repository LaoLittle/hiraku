cfg_select! {
    target_arch = "wasm32" => {
        mod wasm;
        pub(crate) use wasm::*;
    },
    _ => {
        mod native;
        pub(crate) use native::*;
    }
}

use std::{sync::Arc, time::Duration};

use bevy::{asset::Handle, image::Image};

use crate::color::{TransferFunction, YuvColorTransform};

#[derive(Debug)]
pub(crate) struct DecodedFrame {
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
    pub pixels: DecodedPixels,
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "each platform constructs only its decoded pixel layouts"
)]
pub(crate) enum DecodedPixels {
    StridedYuv {
        planes: Arc<[u8]>,
        u_offset: usize,
        v_offset: usize,
        y_stride: u32,
        chroma_stride: u32,
    },
    PlanarYuv {
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    },
    Rgba(Vec<u8>),
}

#[derive(Debug)]
pub(crate) enum DecodeEvent {
    Frame(DecodedFrame),
    End,
    Error(String),
}

#[derive(Clone)]
#[allow(dead_code, reason = "the strided upload payload is native-only")]
pub(crate) struct VideoFrameUpload {
    pub y_image: Handle<Image>,
    pub u_image: Handle<Image>,
    pub v_image: Handle<Image>,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub planes: Arc<[u8]>,
    pub u_offset: usize,
    pub v_offset: usize,
    pub y_stride: u32,
    pub chroma_stride: u32,
}
