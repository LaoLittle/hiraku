//! Unlit atlas sprites in 3D. No story, character, or camera creation policy.
mod billboard;
mod clip;
mod material;
pub mod sampling;
mod sprite;

use bevy::prelude::*;
pub use billboard::{Billboard, BillboardMode, BillboardPlugin};
pub use clip::ClipRect;
pub use material::Sprite3dMaterial;
pub use sprite::{
    BlendMode, MAX_LAYERS, MAX_MASKS, MaskMode, Sprite3d, Sprite3dError, SpriteLayer,
};

pub struct Sprite3dPlugin;

/// Embeddings producing Sprite3d components can order their projection before this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sprite3dSync;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageSamplingSync;
impl Plugin for Sprite3dPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sprite3d.wesl");
        bevy::shader::load_shader_library!(app, "sampling.wesl");
        app.init_resource::<sampling::ImageSampling>();
        if !app
            .world()
            .contains_resource::<Assets<TextureAtlasLayout>>()
        {
            app.init_asset::<TextureAtlasLayout>();
        }
        app.register_type::<Sprite3d>()
            .register_type::<SpriteLayer>()
            .register_type::<BlendMode>()
            .register_type::<MaskMode>();
        app.add_plugins(bevy::pbr::MaterialPlugin::<Sprite3dMaterial>::default())
            .add_systems(
                PostUpdate,
                sprite::sync_sprites
                    .in_set(Sprite3dSync)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
        if !app.is_plugin_added::<BillboardPlugin>() {
            app.add_plugins(BillboardPlugin);
        }
        app.add_systems(
            PostUpdate,
            sync_sampling.after(Sprite3dSync).in_set(ImageSamplingSync),
        );
    }
}

fn sync_sampling(
    images: Res<Assets<Image>>,
    sampling: Res<sampling::ImageSampling>,
    mut materials: ResMut<Assets<Sprite3dMaterial>>,
) {
    let updates: Vec<_> = materials
        .iter()
        .filter_map(|(id, material)| {
            let mode = material
                .image
                .as_ref()
                .map_or(0, |image| sampling.mode(image, &images));
            (material.sampling.x != mode).then_some((id, mode))
        })
        .collect();
    for (id, mode) in updates {
        if let Some(mut material) = materials.get_mut(id) {
            material.sampling.x = mode;
        }
    }
}
