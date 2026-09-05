use bevy::{
    prelude::*,
    render::render_resource::{AsBindGroup, RenderPipelineDescriptor},
    shader::{ShaderDefVal, ShaderRef},
    ui_render::prelude::UiMaterialKey,
};
use hiraku_media::{TransferFunction, YuvPixelFormat};

use crate::color::YuvColorTransform;

pub(crate) fn load_internal_shader(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/yuv420.wgsl");
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Yuv420MaterialKey {
    transfer: TransferFunction,
    format: YuvPixelFormat,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(Yuv420MaterialKey)]
pub(crate) struct Yuv420Material {
    #[texture(0)]
    #[sampler(3)]
    pub y: Handle<Image>,
    /// I420: U plane.
    /// NV12: UV plane.
    #[texture(1)]
    pub chroma0: Handle<Image>,
    /// I420: V plane.
    /// NV12: dummy binding.
    #[texture(2)]
    pub chroma1: Handle<Image>,
    #[uniform(4)]
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
    pub format: YuvPixelFormat,
}

impl From<&Yuv420Material> for Yuv420MaterialKey {
    fn from(material: &Yuv420Material) -> Self {
        Self {
            transfer: material.transfer,
            format: material.format,
        }
    }
}

impl UiMaterial for Yuv420Material {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_video/shaders/yuv420.wgsl".into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, key: UiMaterialKey<Self>) {
        let frag = descriptor
            .fragment
            .as_mut()
            .expect("YUV UI material must have a fragment shader");

        let transfer_def = match key.bind_group_data.transfer {
            TransferFunction::Linear => Some("TRANSFER_LINEAR"),
            TransferFunction::Bt1886 => None,
            TransferFunction::Srgb => Some("TRANSFER_SRGB"),
            TransferFunction::Gamma22 => Some("TRANSFER_GAMMA_22"),
            TransferFunction::Gamma28 => Some("TRANSFER_GAMMA_28"),
        };

        if let Some(transfer_def) = transfer_def {
            frag.shader_defs
                .push(ShaderDefVal::Bool(transfer_def.into(), true));
        }

        let format_def = match key.bind_group_data.format {
            YuvPixelFormat::I420 => "FORMAT_I420",
            YuvPixelFormat::Nv12 => "FORMAT_NV12",
        };

        frag.shader_defs
            .push(ShaderDefVal::Bool(format_def.into(), true));
    }
}
