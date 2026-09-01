use bevy::{prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef};

use crate::color::YuvColorTransform;

pub(crate) fn load_internal_shader(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/yuv420.wgsl");
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
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
}

impl UiMaterial for Yuv420Material {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_video/shaders/yuv420.wgsl".into()
    }
}
