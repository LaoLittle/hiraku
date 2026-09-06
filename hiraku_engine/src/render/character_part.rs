use bevy::{
    asset::Handle,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{Material, MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState,
        RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};

pub fn load_internal_shaders(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/alpha_mask.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/multiply.wgsl");
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, AlphaMaskUniform)]
#[bind_group_data(AlphaMaskKey)]
pub struct AlphaMaskMaterial {
    #[texture(1)]
    #[sampler(2)]
    pub texture: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub mask_texture: Handle<Image>,
    pub tint: Vec4,
    pub main_rect: Vec4,
    pub mask_rect: Vec4,
    /// Main and mask offsets in actor-local pixels: `(main.x, main.y, mask.x, mask.y)`.
    pub offsets: Vec4,
    pub opacity: f32,
    pub mask_enabled: f32,
    /// Blending is independent of coverage; this selects a pipeline variant.
    pub multiply: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AlphaMaskKey {
    multiply: bool,
}

impl From<&AlphaMaskMaterial> for AlphaMaskKey {
    fn from(material: &AlphaMaskMaterial) -> Self {
        Self { multiply: material.multiply }
    }
}

#[derive(Clone, Debug, ShaderType)]
pub struct AlphaMaskUniform {
    tint: Vec4,
    main_rect: Vec4,
    mask_rect: Vec4,
    offsets: Vec4,
    opacity: f32,
    mask_enabled: f32,
    _padding: Vec2,
}

impl From<&AlphaMaskMaterial> for AlphaMaskUniform {
    fn from(material: &AlphaMaskMaterial) -> Self {
        Self {
            tint: material.tint,
            main_rect: material.main_rect,
            mask_rect: material.mask_rect,
            offsets: material.offsets,
            opacity: material.opacity,
            mask_enabled: material.mask_enabled,
            _padding: Vec2::ZERO,
        }
    }
}

impl Material for AlphaMaskMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/render/shaders/alpha_mask.wgsl".into()
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
        if key.bind_group_data.multiply {
            descriptor.fragment.as_mut()
                .expect("character material requires a fragment stage")
                .shader_defs.push("MASK_MULTIPLY".into());
            specialize_multiply_blend(descriptor);
        }
        Ok(())
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, MultiplyUniform)]
pub struct MultiplyMaterial {
    #[texture(1)]
    #[sampler(2)]
    pub texture: Handle<Image>,
    pub tint: Vec4,
    pub rect: Vec4,
    pub opacity: f32,
}

#[derive(Clone, Debug, ShaderType)]
pub struct MultiplyUniform {
    tint: Vec4,
    rect: Vec4,
    opacity: f32,
    _padding: Vec3,
}

impl From<&MultiplyMaterial> for MultiplyUniform {
    fn from(material: &MultiplyMaterial) -> Self {
        Self {
            tint: material.tint,
            rect: material.rect,
            opacity: material.opacity,
            _padding: Vec3::ZERO,
        }
    }
}

impl Material for MultiplyMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/render/shaders/multiply.wgsl".into()
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
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        specialize_multiply_blend(descriptor);
        Ok(())
    }
}

/// Both multiply paths output premultiplied color. For coverage `a`, this
/// computes destination * (source * a + 1 - a), including mask/fade opacity.
fn specialize_multiply_blend(descriptor: &mut RenderPipelineDescriptor) {
    let target = descriptor
        .fragment
        .as_mut()
        .and_then(|fragment| fragment.targets.first_mut())
        .and_then(Option::as_mut)
        .expect("character material requires a color target");
    target.blend = Some(multiply_blend_state());
}

fn multiply_blend_state() -> BlendState {
    BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Dst,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::OVER,
    }
}

#[derive(Component, Clone, Debug)]
pub struct CharacterPartVisual {
    pub base_alpha: f32,
    pub rect: Option<[f32; 4]>,
}

pub fn rgba8_color(color: [u8; 4]) -> Color {
    Color::srgba_u8(color[0], color[1], color[2], color[3])
}

pub fn rgba8_linear(color: [u8; 4]) -> Vec4 {
    rgba8_color(color).to_linear().to_f32_array().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_blend_mode_is_part_of_pipeline_key_not_uniform_layout() {
        let mut material = AlphaMaskMaterial {
            texture: Handle::default(), mask_texture: Handle::default(),
            tint: Vec4::ONE, main_rect: Vec4::ONE, mask_rect: Vec4::ONE,
            offsets: Vec4::ZERO, opacity: 0.5, mask_enabled: 1.0, multiply: false,
        };
        let normal = material.bind_group_data();
        material.multiply = true;
        let multiply = material.bind_group_data();
        assert_ne!(normal, multiply);
        assert!(multiply.multiply);
        let uniform = AlphaMaskUniform::from(&material);
        assert_eq!(uniform.opacity, 0.5);
        assert_eq!(uniform.mask_enabled, 1.0);
    }

    #[test]
    fn multiply_preserves_uncovered_destination_and_composites_alpha() {
        let blend = multiply_blend_state();
        assert_eq!(blend.color.src_factor, BlendFactor::Dst);
        assert_eq!(blend.color.dst_factor, BlendFactor::OneMinusSrcAlpha);
        assert_eq!(blend.alpha, BlendComponent::OVER);
        let composite = |source: f32, destination: f32, alpha: f32| {
            source * alpha * destination + destination * (1.0 - alpha)
        };
        assert_eq!(composite(0.25, 0.8, 0.0), 0.8);
        assert_eq!(composite(0.25, 0.8, 1.0), 0.2);
        assert_eq!(composite(0.25, 0.8, 0.5), 0.5);
    }
}
