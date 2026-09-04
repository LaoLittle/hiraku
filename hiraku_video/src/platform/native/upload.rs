use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{Extent3d, TexelCopyBufferLayout},
        renderer::RenderQueue,
        texture::GpuImage,
    },
};

use crate::platform::{VideoFrameI420, VideoFrameNv12, VideoFrameUpload};

#[derive(Clone, Default, Resource, ExtractResource)]
pub(crate) struct VideoUpload {
    generation: u64,
    frame: Option<VideoFrameUpload>,
}

impl VideoUpload {
    pub fn publish(&mut self, frame: VideoFrameUpload) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.frame = Some(frame);
    }
}

pub(crate) fn install_video_upload(app: &mut App) {
    app.init_resource::<VideoUpload>()
        .add_plugins(ExtractResourcePlugin::<VideoUpload>::default());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            upload_video_frame
                .in_set(RenderSystems::PrepareResources)
                .after(RenderSystems::PrepareAssets),
        );
    }
}

fn upload_video_frame(
    upload: Res<VideoUpload>,
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

    let uploaded = match frame {
        VideoFrameUpload::I420(frame) => upload_i420_frame(frame, &gpu_images, &render_queue),
        VideoFrameUpload::Nv12(frame) => upload_nv12_frame(frame, &gpu_images, &render_queue)
    };
    
    if uploaded {
        *uploaded_generation = upload.generation;
    }
}

fn upload_i420_frame(
    frame: &VideoFrameI420,
    gpu_images: &RenderAssets<GpuImage>,
    render_queue: &RenderQueue,
) -> bool {
    let (Some(y_image), Some(u_image), Some(v_image)) = (
        gpu_images.get(&frame.y_image),
        gpu_images.get(&frame.u_image),
        gpu_images.get(&frame.v_image),
    ) else {
        return false;
    };

    write_plane(
        render_queue,
        y_image,
        &frame.planes[..frame.u_offset],
        frame.y_stride,
        frame.width,
        frame.height,
    );

    write_plane(
        render_queue,
        u_image,
        &frame.planes[frame.u_offset..frame.v_offset],
        frame.chroma_stride,
        frame.chroma_width,
        frame.chroma_height,
    );

    write_plane(
        render_queue,
        v_image,
        &frame.planes[frame.v_offset..],
        frame.chroma_stride,
        frame.chroma_width,
        frame.chroma_height,
    );

    true
}

fn upload_nv12_frame(
    frame: &VideoFrameNv12,
    gpu_images: &RenderAssets<GpuImage>,
    render_queue: &RenderQueue,
) -> bool {
    let (Some(y_image), Some(uv_image)) = (
        gpu_images.get(&frame.y_image),
        gpu_images.get(&frame.uv_image),
    ) else {
        return false;
    };

    write_plane(
        render_queue,
        y_image,
        &frame.planes[..frame.uv_offset],
        frame.y_stride,
        frame.width,
        frame.height,
    );

    write_plane(
        render_queue,
        uv_image,
        &frame.planes[frame.uv_offset..],
        frame.uv_stride,
        frame.chroma_width,
        frame.chroma_height,
    );

    true
}

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
