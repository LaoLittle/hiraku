use bevy::{
    prelude::*,
    render::render_resource::{AsBindGroup, RenderPipelineDescriptor},
    shader::{ShaderDefVal, ShaderRef},
    ui_render::prelude::UiMaterialKey,
};

use crate::color::{TransferFunction, YuvColorTransform};

pub(crate) fn load_internal_shader(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/yuv420.wgsl");
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
