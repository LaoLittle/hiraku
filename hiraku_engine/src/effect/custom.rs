use bevy::{
    asset::{AssetPath, Handle},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::{Shader, ShaderRef},
    sprite_render::{AlphaMode2d, Material2d},
};

pub fn load_internal_shaders(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/custom_screen_effect.wgsl");
}

#[derive(Debug, Clone)]
pub struct CustomEffectOptions {
    pub from_path: String,
    pub to_path: String,
    pub rule_path: String,
    pub aux0_path: String,
    pub aux1_path: String,
    pub duration: std::time::Duration,
    pub mode: f32,
    pub p0: Vec4,
    pub p1: Vec4,
    pub p2: Vec4,
    pub p3: Vec4,
    pub commit_to_bg: bool,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, CustomScreenEffectUniform)]
pub struct CustomScreenEffectMaterial {
    #[texture(1)]
    #[sampler(2)]
    pub source_texture: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub target_texture: Handle<Image>,
    #[texture(5)]
    #[sampler(6)]
    pub rule_texture: Handle<Image>,
    #[texture(7)]
    #[sampler(8)]
    pub aux0_texture: Handle<Image>,
    #[texture(9)]
    #[sampler(10)]
    pub aux1_texture: Handle<Image>,
    pub progress: f32,
    pub duration: f32,
    pub time: f32,
    pub mode: f32,
    pub p0: Vec4,
    pub p1: Vec4,
    pub p2: Vec4,
    pub p3: Vec4,
}

#[derive(Clone, Debug, ShaderType)]
pub struct CustomScreenEffectUniform {
    pub progress: f32,
    pub duration: f32,
    pub time: f32,
    pub mode: f32,
    pub p0: Vec4,
    pub p1: Vec4,
    pub p2: Vec4,
    pub p3: Vec4,
}

impl From<&CustomScreenEffectMaterial> for CustomScreenEffectUniform {
    fn from(material: &CustomScreenEffectMaterial) -> Self {
        Self {
            progress: material.progress,
            duration: material.duration,
            time: material.time,
            mode: material.mode,
            p0: material.p0,
            p1: material.p1,
            p2: material.p2,
            p3: material.p3,
        }
    }
}

impl Material2d for CustomScreenEffectMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/effect/shaders/custom_screen_effect.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

#[derive(Component)]
pub struct CustomScreenEffectPlayer {
    pub material: Handle<CustomScreenEffectMaterial>,
    pub timer: Timer,
    pub target_path: Option<String>,
    pub target_image: Option<Handle<Image>>,
    pub previous_background: Option<Entity>,
    pub animation_id: Option<String>,
    pub done: Option<std::sync::mpsc::Sender<crate::script::ScriptResponse>>,
}
