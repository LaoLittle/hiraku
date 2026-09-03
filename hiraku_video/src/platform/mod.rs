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
pub(crate) struct VideoFrameUpload {
    pub y_image: bevy::asset::Handle<bevy::image::Image>,
    pub u_image: bevy::asset::Handle<bevy::image::Image>,
    pub v_image: bevy::asset::Handle<bevy::image::Image>,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub planes: std::sync::Arc<[u8]>,
    pub u_offset: usize,
    pub v_offset: usize,
    pub y_stride: u32,
    pub chroma_stride: u32,
}
