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
    clip_bounds: Vec4,
    clip_axes: Vec4,
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
    /// Resolve layout assets without altering the authoring component.
    pub fn from_sprite(
        sprite: &Sprite3d,
        atlases: &Assets<TextureAtlasLayout>,
    ) -> Result<Self, crate::Sprite3dError> {
        let rects = sprite.resolve_rects(atlases)?;
        Ok(Self::from_resolved(sprite, &rects))
    }
    pub(crate) fn from_resolved(sprite: &Sprite3d, rects: &[Option<Rect>; MAX_LAYERS]) -> Self {
        let mut layers = [LayerUniform::default(); MAX_LAYERS];
        let fallback = [SpriteLayer::default()];
        let source = if sprite.layers.is_empty() {
            &fallback[..]
        } else {
            &sprite.layers
        };
        for ((out, layer), rect) in layers.iter_mut().zip(source).zip(rects) {
            let (mode, reference, cutoff) = match layer.mask {
                MaskMode::None => (0, 0, 0.0),
                MaskMode::Read(reference) => (1, reference, 0.0),
                MaskMode::StencilWrite {
                    reference,
                    cutoff,
                    visible,
                } => (if visible { 5 } else { 4 }, reference, cutoff),
                MaskMode::Write {
                    reference,
                    cutoff,
                    visible,
                } => (if visible { 3 } else { 2 }, reference, cutoff),
            };
            *out = LayerUniform {
                rect: rect.map_or(Vec4::ZERO, |r| r.min.extend(r.width()).extend(r.height())),
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
                clip_bounds: sprite
                    .clip
                    .map_or(Vec4::ZERO, |clip| clip.shader_parameters()[0]),
                clip_axes: sprite
                    .clip
                    .map_or(Vec4::ZERO, |clip| clip.shader_parameters()[1]),
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
        Self::from_sprite(sprite, &Assets::default())
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
