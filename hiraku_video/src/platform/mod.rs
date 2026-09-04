use std::sync::Arc;

use bevy::{asset::Handle, image::Image};

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

#[derive(Clone)]
#[allow(dead_code, reason = "the strided upload payload is native-only")]
pub(crate) enum VideoFrameUpload {
    I420(VideoFrameI420),
    Nv12(VideoFrameNv12),
}

#[derive(Clone)]
pub(crate) struct VideoFrameI420 {
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

#[derive(Clone)]
pub(crate) struct VideoFrameNv12 {
    pub y_image: Handle<Image>,
    pub uv_image: Handle<Image>,

    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,

    pub planes: Arc<[u8]>,

    pub uv_offset: usize,

    pub y_stride: u32,
    pub uv_stride: u32,
}
