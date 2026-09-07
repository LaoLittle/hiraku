use crate::{BlendMode, MAX_LAYERS, MaskMode, Sprite3d, SpriteLayer};
use bevy::{
    mesh::MeshVertexBufferLayoutRef,
    pbr::{Material, MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct LayerUniform {
    rect: Vec4,
    bounds: Vec4,
    tint: Vec4,
    // blend, mask mode (0 none / 1 read / 2 write / 3 visible write), reference, cutoff
    modes: Vec4,
    flip: Vec4,
}
#[derive(Clone, Debug, ShaderType)]
pub struct SpriteUniform {
    tint: Vec4,
    backface_tint: Vec4,
    count: UVec4,
    layers: [LayerUniform; MAX_LAYERS],
}
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct Sprite3dMaterial {
    #[uniform(0)]
    pub(crate) uniform: SpriteUniform,
    #[texture(1)]
    #[sampler(2)]
    pub image: Option<Handle<Image>>,
}
impl Sprite3dMaterial {
    pub(crate) fn from_sprite(sprite: &Sprite3d) -> Self {
        let mut layers = [LayerUniform::default(); MAX_LAYERS];
        let fallback = [SpriteLayer::default()];
        let source = if sprite.layers.is_empty() {
            &fallback[..]
        } else {
            &sprite.layers
        };
        for (out, layer) in layers.iter_mut().zip(source) {
            let (mode, reference, cutoff) = match layer.mask {
                MaskMode::None => (0, 0, 0.0),
                MaskMode::Read(reference) => (1, reference, 0.0),
                MaskMode::Write {
                    reference,
                    cutoff,
                    visible,
                } => (if visible { 3 } else { 2 }, reference, cutoff),
            };
            *out = LayerUniform {
                rect: layer
                    .rect
                    .map_or(Vec4::ZERO, |r| r.min.extend(r.width()).extend(r.height())),
                bounds: layer
                    .bounds
                    .min
                    .extend(layer.bounds.width())
                    .extend(layer.bounds.height()),
                tint: layer.color.to_linear().to_f32_array().into(),
                modes: Vec4::new(
                    if layer.blend == BlendMode::Multiply {
                        1.0
                    } else {
                        0.0
                    },
                    mode as f32,
                    reference as f32,
                    cutoff,
                ),
                flip: Vec4::new(
                    layer.flip_x as u8 as f32,
                    layer.flip_y as u8 as f32,
                    0.0,
                    0.0,
                ),
            };
        }
        Self {
            image: sprite.image.clone(),
            uniform: SpriteUniform {
                tint: sprite.color.to_linear().to_f32_array().into(),
                backface_tint: sprite.backface_color.to_linear().to_f32_array().into(),
                count: UVec4::new(source.len() as u32, 0, 0, 0),
                layers,
            },
        }
    }
}
impl TryFrom<&Sprite3d> for Sprite3dMaterial {
    type Error = crate::Sprite3dError;
    fn try_from(sprite: &Sprite3d) -> Result<Self, Self::Error> {
        sprite.validate()?;
        Ok(Self::from_sprite(sprite))
    }
}
impl Material for Sprite3dMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_sprite3d/sprite3d.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_parses_and_validates_without_a_gpu() {
        let source = include_str!("sprite3d.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };")
            .replace("#{MATERIAL_BIND_GROUP}", "2");
        let module = naga::front::wgsl::parse_str(&source).expect("sprite WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("sprite WGSL validates");
        // This uniform fits comfortably below WebGL2's 16 KiB guaranteed block size.
        assert!(SpriteUniform::min_size().get() < 16 * 1024);
        assert_eq!(LayerUniform::min_size().get(), 80);
    }

    #[test]
    fn overall_opacity_does_not_modify_layer_or_mask_alpha() {
        let sprite = Sprite3d {
            color: Color::linear_rgba(1.0, 1.0, 1.0, 0.25),
            layers: vec![SpriteLayer {
                color: Color::linear_rgba(0.2, 0.4, 0.8, 0.375),
                mask: MaskMode::Write {
                    reference: 1,
                    cutoff: 0.1,
                    visible: true,
                },
                ..default()
            }],
            ..default()
        };
        let material = Sprite3dMaterial::try_from(&sprite).expect("valid material");
        assert_eq!(material.uniform.tint.w, 0.25);
        assert_eq!(material.uniform.layers[0].tint.w, 0.375);
        assert_eq!(
            material.uniform.layers[0].modes,
            Vec4::new(0.0, 3.0, 1.0, 0.1)
        );
        assert_eq!(material.alpha_mode(), AlphaMode::Premultiplied);
    }
}
