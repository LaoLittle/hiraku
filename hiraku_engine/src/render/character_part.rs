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
        let target = descriptor
            .fragment
            .as_mut()
            .and_then(|fragment| fragment.targets.first_mut())
            .and_then(Option::as_mut)
            .expect("Material2d must provide a color target");
        target.blend = Some(BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Dst,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::OVER,
        });
        Ok(())
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
