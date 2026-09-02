use bevy::{
    prelude::*,
    render::render_resource::{AsBindGroup, RenderPipelineDescriptor},
    shader::{ShaderDefVal, ShaderRef},
    ui_render::prelude::UiMaterialKey,
};

#[cfg(not(target_arch = "wasm32"))]
use bevy::render::{
    Render, RenderApp, RenderSystems,
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    render_asset::RenderAssets,
    render_resource::{Extent3d, TexelCopyBufferLayout},
    renderer::RenderQueue,
    texture::GpuImage,
};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use crate::color::{TransferFunction, YuvColorTransform};

pub(crate) fn load_internal_shader(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/yuv420.wgsl");
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub(crate) struct NativeVideoFrameUpload {
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

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default, Resource, ExtractResource)]
pub(crate) struct NativeVideoUpload {
    generation: u64,
    frame: Option<NativeVideoFrameUpload>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeVideoUpload {
    pub fn publish(&mut self, frame: NativeVideoFrameUpload) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.frame = Some(frame);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn install_native_video_upload(app: &mut App) {
    app.init_resource::<NativeVideoUpload>()
        .add_plugins(ExtractResourcePlugin::<NativeVideoUpload>::default());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            upload_native_video_frame
                .in_set(RenderSystems::PrepareResources)
                .after(RenderSystems::PrepareAssets),
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn upload_native_video_frame(
    upload: Res<NativeVideoUpload>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    render_queue: Res<RenderQueue>,
    mut uploaded_generation: Local<u64>,
) {
    if upload.generation == *uploaded_generation {
        return;
    }
    let Some(frame) = upload.frame.as_ref() else {
        *uploaded_generation = upload.generation;
        return;
    };
    let (Some(y_image), Some(u_image), Some(v_image)) = (
        gpu_images.get(&frame.y_image),
        gpu_images.get(&frame.u_image),
        gpu_images.get(&frame.v_image),
    ) else {
        // The Image assets are prepared in the same render frame. Retry next
        // frame if this backend has not published their GPU resources yet.
        return;
    };

    write_plane(
        &render_queue,
        y_image,
        &frame.planes[..frame.u_offset],
        frame.y_stride,
        frame.width,
        frame.height,
    );
    write_plane(
        &render_queue,
        u_image,
        &frame.planes[frame.u_offset..frame.v_offset],
        frame.chroma_stride,
        frame.chroma_width,
        frame.chroma_height,
    );
    write_plane(
        &render_queue,
        v_image,
        &frame.planes[frame.v_offset..],
        frame.chroma_stride,
        frame.chroma_width,
        frame.chroma_height,
    );
    *uploaded_generation = upload.generation;
}

#[cfg(not(target_arch = "wasm32"))]
fn write_plane(
    render_queue: &RenderQueue,
    gpu_image: &GpuImage,
    bytes: &[u8],
    stride: u32,
    width: u32,
    height: u32,
) {
    render_queue.write_texture(
        gpu_image.texture.as_image_copy(),
        bytes,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Yuv420MaterialKey {
    transfer: TransferFunction,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(Yuv420MaterialKey)]
pub(crate) struct Yuv420Material {
    #[texture(0)]
    #[sampler(3)]
    pub y: Handle<Image>,
    #[texture(1)]
    pub u: Handle<Image>,
    #[texture(2)]
    pub v: Handle<Image>,
    #[uniform(4)]
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
}

impl From<&Yuv420Material> for Yuv420MaterialKey {
    fn from(material: &Yuv420Material) -> Self {
        Self {
            transfer: material.transfer,
        }
    }
}

impl UiMaterial for Yuv420Material {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_video/shaders/yuv420.wgsl".into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, key: UiMaterialKey<Self>) {
        let shader_def = match key.bind_group_data.transfer {
            TransferFunction::Linear => Some("TRANSFER_LINEAR"),
            TransferFunction::Bt1886 => None,
            TransferFunction::Srgb => Some("TRANSFER_SRGB"),
            TransferFunction::Gamma22 => Some("TRANSFER_GAMMA_22"),
            TransferFunction::Gamma28 => Some("TRANSFER_GAMMA_28"),
        };
        if let Some(shader_def) = shader_def {
            descriptor
                .fragment
                .as_mut()
                .expect("YUV UI material must have a fragment shader")
                .shader_defs
                .push(ShaderDefVal::Bool(shader_def.into(), true));
        }
    }
}
