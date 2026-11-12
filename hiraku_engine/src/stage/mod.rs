//! Data-driven 3D stages. Anchors and camera presets are scene data, not
//! story-specific trial mechanics. Explicit views allocate on-demand cameras.
mod animation;
mod shake;
pub use shake::StageCameraShake;
mod definition;
pub use animation::{StageAnimation, StagePlayback};
mod effects;
mod lights;
pub use effects::{StageEffect, StageEffectPlayback, StageEffectQuad, StageEmission};
mod model;
mod shading;
pub use model::{StageAlpha, StageLight, StageLightKind, StageMaterial};
pub use shading::StageShading;
mod clip;
mod redraw;
pub use clip::ViewClip;
pub(crate) mod runtime;
pub(crate) mod views;
pub use definition::{
    StageCamera, StageCameraPose, StageDefinition, StageOrbit, StagePose, StageProjection,
};
pub use runtime::{StageCommand, StageSnapshot};

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};

pub struct StagePlugin;
impl Plugin for StagePlugin {
    fn build(&self, app: &mut App) {
        shading::register(app);
        effects::register(app);
        redraw::register(app);
        app.init_asset::<StageDefinition>()
            .init_asset_loader::<StageLoader>();
        app.init_resource::<runtime::StageRuntime>().add_systems(
            Update,
            runtime::sync
                .after(crate::render::camera::animate_camera_shake)
                .before(crate::scene::pictures::sync_pictures)
                .before(crate::scene::effect_wait::complete)
                .run_if(crate::runtime_initialized),
        );
        app.init_resource::<views::StageViews>().add_systems(
            Update,
            views::sync
                .after(runtime::sync)
                .run_if(crate::runtime_initialized),
        );
        app.add_systems(
            PostUpdate,
            runtime::prepare_surfaces.run_if(crate::runtime_initialized),
        );
        app.add_systems(
            PostUpdate,
            lights::sync
                .after(runtime::prepare_surfaces)
                .run_if(crate::runtime_initialized),
        );
        app.add_systems(
            PostUpdate,
            animation::sync
                .after(runtime::prepare_surfaces)
                .before(bevy::app::AnimationSystems)
                .run_if(crate::runtime_initialized),
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("failed to read stage: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid stage: {0}")]
    Invalid(String),
}

#[derive(Default, TypePath)]
struct StageLoader;
impl AssetLoader for StageLoader {
    type Asset = StageDefinition;
    type Settings = ();
    type Error = StageError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<StageDefinition, StageError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let mut stage: StageDefinition = hiraku_script::hson::from_slice(&bytes)
            .map_err(|error| StageError::Invalid(error.to_string()))?;
        stage.validate().map_err(StageError::Invalid)?;
        // Resolve relative to the descriptor, retaining the asset source and
        // glTF subasset label. AssetServer tracks the scene as a dependency.
        if let Some(path) = &stage.scene {
            let path = context
                .path()
                .resolve_embed_str(path)
                .map_err(|error| StageError::Invalid(error.to_string()))?;
            stage.scene_handle = Some(
                context
                    .load_builder()
                    .with_settings(|settings: &mut bevy::gltf::GltfLoaderSettings| {
                        settings.load_materials = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                    })
                    .load(path),
            );
        }
        for animation in stage.animations.values() {
            let path = context
                .path()
                .resolve_embed_str(&animation.clip)
                .map_err(|error| StageError::Invalid(error.to_string()))?;
            stage.animation_handles.push(context.load(path));
        }
        for effect in stage.effects.values_mut() {
            let shader = context
                .path()
                .resolve_embed_str(&effect.shader)
                .map_err(|error| StageError::Invalid(error.to_string()))?;
            let shader = context.load(shader);
            stage.effect_shaders.push(shader.clone());
            effect.shader_handle = Some(shader);
            for path in &effect.textures {
                let path = context
                    .path()
                    .resolve_embed_str(path)
                    .map_err(|error| StageError::Invalid(error.to_string()))?;
                let image = context
                    .load_builder()
                    .with_settings(|settings: &mut bevy::image::ImageLoaderSettings| {
                        settings.is_srgb = false;
                        settings.sampler = bevy::image::ImageSampler::Descriptor(
                            bevy::image::ImageSamplerDescriptor {
                                address_mode_u: bevy::image::ImageAddressMode::Repeat,
                                address_mode_v: bevy::image::ImageAddressMode::Repeat,
                                mag_filter: bevy::image::ImageFilterMode::Linear,
                                min_filter: bevy::image::ImageFilterMode::Linear,
                                mipmap_filter: bevy::image::ImageFilterMode::Linear,
                                ..default()
                            },
                        );
                    })
                    .load(path);
                stage.effect_images.push(image.clone());
                effect.texture_handles.push(image);
            }
        }
        Ok(stage)
    }

    fn extensions(&self) -> &[&str] {
        &["stage.hson"]
    }
}

/// One owned hierarchy: despawning this entity also removes the model and anchors.
#[derive(Component)]
pub struct StageRoot;

/// Named spatial reference; consumers use GlobalTransform after propagation.
#[derive(Component, Debug)]
pub struct StageAnchor(pub String);

impl StageDefinition {
    /// Instantiate only spatial data. Rendering continues through the embedding
    /// application's presentation camera; presets do not create view instances.
    pub fn spawn(&self, commands: &mut Commands) -> Result<Entity, StageError> {
        self.validate().map_err(StageError::Invalid)?;
        if self.scene.is_some() && self.scene_handle.is_none() {
            return Err(StageError::Invalid(
                "stage scene must be loaded through AssetServer before instantiation".into(),
            ));
        }
        if self.effects.values().any(|effect| {
            effect.shader_handle.is_none() || effect.texture_handles.len() != effect.textures.len()
        }) {
            return Err(StageError::Invalid(
                "stage effect dependencies must be loaded through AssetServer before instantiation"
                    .into(),
            ));
        }
        let root = commands
            .spawn((StageRoot, Transform::default(), Visibility::default()))
            .id();
        commands.entity(root).with_children(|children| {
            if let Some(scene) = &self.scene_handle {
                children.spawn(WorldAssetRoot(scene.clone()));
            }
            for (name, pose) in &self.anchors {
                children.spawn((
                    StageAnchor(name.clone()),
                    pose.transform(),
                    Visibility::default(),
                ));
            }
        });
        Ok(root)
    }

    pub fn anchor(&self, name: &str) -> Result<Transform, StageError> {
        self.anchors
            .get(name)
            .map(StagePose::transform)
            .ok_or_else(|| StageError::Invalid(format!("unknown anchor `{name}`")))
    }

    pub fn camera(&self, name: &str) -> Result<&StageCamera, StageError> {
        self.cameras
            .get(name)
            .ok_or_else(|| StageError::Invalid(format!("unknown camera preset `{name}`")))
    }
}
