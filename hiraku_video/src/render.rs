use bevy::{
    mesh::MeshVertexBufferLayoutRef,
    pbr::{Material, MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::{ShaderDefVal, ShaderRef},
    ui_render::prelude::UiMaterialKey,
};
use hiraku_media::{TransferFunction, YuvPixelFormat};

use crate::color::YuvColorTransform;

pub(crate) fn load_internal_shader(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/yuv420.wesl");
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Yuv420MaterialKey {
    transfer: TransferFunction,
    format: YuvPixelFormat,
    alpha_layout: Option<crate::AlphaLayout>,
    rgba: bool,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(Yuv420MaterialKey)]
pub(crate) struct Yuv420Material {
    #[uniform(5)]
    pub opacity: f32,
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
    pub alpha_layout: Option<crate::AlphaLayout>,
    pub rgba: bool,
}

impl From<&Yuv420Material> for Yuv420MaterialKey {
    fn from(material: &Yuv420Material) -> Self {
        Self {
            transfer: material.transfer,
            format: material.format,
            alpha_layout: material.alpha_layout,
            rgba: material.rgba,
        }
    }
}

impl UiMaterial for Yuv420Material {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_video/shaders/yuv420.wesl".into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, key: UiMaterialKey<Self>) {
        specialize_video(descriptor, key.bind_group_data);
    }
}

impl Material for Yuv420Material {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_video/shaders/yuv420.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        specialize_video(descriptor, key.bind_group_data);
        descriptor
            .fragment
            .as_mut()
            .expect("video fragment")
            .shader_defs
            .push("WORLD_VIDEO".into());
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

fn specialize_video(descriptor: &mut RenderPipelineDescriptor, key: Yuv420MaterialKey) {
    let frag = descriptor
        .fragment
        .as_mut()
        .expect("video material must have a fragment shader");

    let transfer_def = match key.transfer {
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

    let format_def = match key.format {
        YuvPixelFormat::I420 => "FORMAT_I420",
        YuvPixelFormat::Nv12 => "FORMAT_NV12",
    };
    if key.rgba {
        frag.shader_defs
            .push(ShaderDefVal::Bool("FORMAT_RGBA".into(), true));
    }
    if let Some(layout) = key.alpha_layout {
        frag.shader_defs.push(ShaderDefVal::Bool(
            match layout {
                crate::AlphaLayout::Vertical => "ALPHA_VERTICAL",
                crate::AlphaLayout::Horizontal => "ALPHA_HORIZONTAL",
            }
            .into(),
            true,
        ));
    }

    frag.shader_defs
        .push(ShaderDefVal::Bool(format_def.into(), true));
}

#[cfg(test)]
mod tests {
    use bevy::{
        asset::Assets,
        shader::{Shader, ShaderCache, ShaderCacheSource, ShaderDefVal},
    };

    #[test]
    fn packed_alpha_shader_links_for_every_surface_format_and_layout() {
        let mut assets = Assets::<Shader>::default();
        let vertex = assets.add(Shader::from_wesl(
            "struct UiVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };",
            "embedded://bevy_ui_render/ui_vertex_output.wesl"));
        let fragment = assets.add(Shader::from_wesl(
            include_str!("shaders/yuv420.wesl"),
            "embedded://hiraku_video/shaders/yuv420.wesl",
        ));
        let world_vertex = assets.add(Shader::from_wesl(
            "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };",
            "embedded://bevy_pbr/render/forward_io.wesl"));
        let mut cache = ShaderCache::new((), |_, source, _| match source {
            ShaderCacheSource::Wgsl(source) => Ok(source),
            _ => panic!("expected WGSL"),
        });
        for handle in [&vertex, &world_vertex, &fragment] {
            cache.set_shader(
                handle.id(),
                assets.get(handle).expect("shader fixture").clone(),
            );
        }
        for (index, format) in ["FORMAT_I420", "FORMAT_NV12", "FORMAT_RGBA"]
            .into_iter()
            .enumerate()
        {
            for layout in [None, Some("ALPHA_VERTICAL"), Some("ALPHA_HORIZONTAL")] {
                let mut defs = vec![
                    ShaderDefVal::Bool(format.into(), true),
                    ShaderDefVal::Bool("TRANSFER_SRGB".into(), true),
                ];
                if format == "FORMAT_RGBA" {
                    defs.push(ShaderDefVal::Bool("FORMAT_I420".into(), true));
                }
                if let Some(layout) = layout {
                    defs.push(ShaderDefVal::Bool(layout.into(), true));
                }
                cache
                    .get(index, fragment.id(), &defs)
                    .unwrap_or_else(|error| panic!("{format}/{layout:?}: {error}"));
                defs.push(ShaderDefVal::Bool("WORLD_VIDEO".into(), true));
                defs.push(ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 3));
                cache
                    .get(index + 10, fragment.id(), &defs)
                    .unwrap_or_else(|error| panic!("world {format}/{layout:?}: {error}"));
            }
        }
    }
}
