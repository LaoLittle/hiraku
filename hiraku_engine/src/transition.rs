use bevy::{
    asset::{Handle, load_internal_asset, uuid_handle},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::{Shader, ShaderRef},
    sprite_render::{AlphaMode2d, Material2d},
};

pub const RULE_TRANSITION_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("f0f375d4-0833-4924-bd99-0186ce8b2a10");

pub fn load_internal_shaders(app: &mut App) {
    load_internal_asset!(
        app,
        RULE_TRANSITION_SHADER_HANDLE,
        "shaders/rule_transition_2d.wgsl",
        Shader::from_wgsl
    );
}

#[derive(Resource, Clone)]
pub struct RuleTransitionMesh(pub Handle<Mesh>);

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, RuleTransitionUniform)]
pub struct RuleTransitionMaterial {
    #[texture(1)]
    #[sampler(2)]
    pub from_texture: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub to_texture: Handle<Image>,
    #[texture(5)]
    #[sampler(6)]
    pub rule_texture: Handle<Image>,
    pub progress: f32,
    pub vague: f32,
}

#[derive(Clone, Debug, ShaderType)]
pub struct RuleTransitionUniform {
    pub progress: f32,
    pub vague: f32,
    pub _padding: Vec2,
}

impl From<&RuleTransitionMaterial> for RuleTransitionUniform {
    fn from(material: &RuleTransitionMaterial) -> Self {
        Self {
            progress: material.progress,
            vague: material.vague,
            _padding: Vec2::ZERO,
        }
    }
}

impl Material2d for RuleTransitionMaterial {
    fn fragment_shader() -> ShaderRef {
        RULE_TRANSITION_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

#[derive(Component)]
pub struct RuleTransitionPlayer {
    pub material: Handle<RuleTransitionMaterial>,
    pub target_path: String,
    pub target_image: Handle<Image>,
    pub previous_background: Entity,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: Option<std::sync::mpsc::Sender<crate::script::ScriptResponse>>,
}
