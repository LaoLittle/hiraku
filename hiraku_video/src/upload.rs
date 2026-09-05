use hiraku_media::{VideoFrame, VideoPixels};
use std::sync::Arc;

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

#[derive(Clone, Default, Resource, ExtractResource)]
pub(crate) struct VideoUpload {
    generation: u64,
    // Extraction clones only handles and the Arc, never the decoded pixels.
    frame: Option<Arc<VideoFrame>>,
    images: [Handle<Image>; 3],
}

impl VideoUpload {
    pub fn clear(&mut self) {
        if self.frame.take().is_some() {
            self.generation = self.generation.wrapping_add(1).max(1);
            self.images = Default::default();
        }
    }

    /// Targets are Y/U/V for I420 and Y/UV/unused for NV12.
    pub fn publish(&mut self, frame: VideoFrame, images: [Handle<Image>; 3]) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.frame = Some(Arc::new(frame));
        self.images = images;
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

    let planes: [(&[u8], u32); 3];
    let count;
    match &frame.pixels {
        VideoPixels::I420Strided {
            planes: data,
            u_offset,
            v_offset,
            y_stride,
            chroma_stride,
        } => {
            planes = [
                (&data[..*u_offset], *y_stride),
                (&data[*u_offset..*v_offset], *chroma_stride),
                (&data[*v_offset..], *chroma_stride),
            ];
            count = 3;
        }
        VideoPixels::Nv12Strided {
            planes: data,
            uv_offset,
            y_stride,
            uv_stride,
        } => {
            planes = [
                (&data[..*uv_offset], *y_stride),
                (&data[*uv_offset..], *uv_stride),
                (&[], 0),
            ];
            count = 2;
        }
        // Packed frames currently use Bevy Image asset updates in the presenter.
        VideoPixels::I420Planar { .. } | VideoPixels::Rgba(_) => return,
    }
    // Wait until every target is prepared before uploading any plane.
    let targets = upload.images.each_ref().map(|image| gpu_images.get(image));
    if targets[..count].iter().any(Option::is_none) {
        return;
    }
    for (index, (bytes, stride)) in planes[..count].iter().enumerate() {
        let (width, height) = if index == 0 {
            (frame.width, frame.height)
        } else {
            (frame.chroma_width, frame.chroma_height)
        };
        write_plane(
            &render_queue,
            targets[index].expect("upload targets were checked"),
            bytes,
            *stride,
            width,
            height,
        );
    }
    *uploaded_generation = upload.generation;
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
