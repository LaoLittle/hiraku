//! Unlit atlas sprites in 3D. No story, character, or camera creation policy.
mod billboard;
mod clip;
mod material;
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
impl Plugin for Sprite3dPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sprite3d.wgsl");
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
    }
}
