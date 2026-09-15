use crate::Sprite3dMaterial;
use bevy::prelude::*;

pub const MAX_LAYERS: usize = 32;
pub const MAX_MASKS: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum BlendMode {
    #[default]
    Normal,
    /// Multiply against earlier layers of this sprite, not the scene backdrop.
    Multiply,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum MaskMode {
    #[default]
    None,
    /// Read alpha coverage from earlier writers. No matching writer = zero.
    Read(u8),
    /// Binary coverage based on source alpha, independent of layer tint/opacity.
    /// Useful for stencil-style sprite masks; visible writers also draw normally.
    StencilWrite {
        reference: u8,
        cutoff: f32,
        visible: bool,
    },
    /// Union coverage. Alpha includes layer tint; cutoff does not make it opaque.
    Write {
        reference: u8,
        cutoff: f32,
        visible: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct SpriteLayer {
    /// Optional cell selection; otherwise inherits Sprite3d.texture_atlas.
    pub texture_atlas: Option<TextureAtlas>,
    /// Destination within the quad in normalized top-left coordinates.
    pub bounds: Rect,
    pub color: Color,
    pub blend: BlendMode,
    pub mask: MaskMode,
    pub flip_x: bool,
    pub flip_y: bool,
}
impl Default for SpriteLayer {
    fn default() -> Self {
        Self {
            texture_atlas: None,
            bounds: Rect::from_corners(Vec2::ZERO, Vec2::ONE),
            color: Color::WHITE,
            blend: BlendMode::Normal,
            mask: MaskMode::None,
            flip_x: false,
            flip_y: false,
        }
    }
}

/// Centered XY quad, front +Z. Uses standard transforms, visibility and mesh
/// picking. Unlit, no shadows. Rendering components are owned by the plugin.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
#[require(Transform, Visibility)]
pub struct Sprite3d {
    /// Optional world-space clipping, applied after the sprite's layer composition.
    pub clip: Option<crate::ClipRect>,
    pub image: Option<Handle<Image>>,
    /// Bevy atlas cell selection, inherited by layers without their own atlas.
    pub texture_atlas: Option<TextureAtlas>,
    /// Overall tint/opacity applied once AFTER layer composition.
    pub color: Color,
    /// Additional back-face tint. Black preserves the image silhouette/alpha.
    pub backface_color: Color,
    /// World-unit size before Transform scale. None = natural image pixel size,
    /// or source rectangle size for a single full-quad atlas layer.
    pub custom_size: Option<Vec2>,
    /// Back-to-front layers sharing one atlas. Empty = one full-image layer.
    pub layers: Vec<SpriteLayer>,
}
impl Default for Sprite3d {
    fn default() -> Self {
        Self {
            clip: None,
            image: None,
            texture_atlas: None,
            color: Color::WHITE,
            custom_size: None,
            backface_color: Color::WHITE,
            layers: vec![],
        }
    }
}
impl Sprite3d {
    pub fn from_image(image: Handle<Image>) -> Self {
        Self {
            image: Some(image),
            ..default()
        }
    }
    pub fn from_atlas(image: Handle<Image>, atlas: TextureAtlas) -> Self {
        Self {
            image: Some(image),
            texture_atlas: Some(atlas),
            ..default()
        }
    }
    pub fn from_color(color: Color, size: Vec2) -> Self {
        Self {
            color,
            custom_size: Some(size),
            ..default()
        }
    }
    pub fn validate(&self) -> Result<(), Sprite3dError> {
        if self.layers.len() > MAX_LAYERS {
            return Err(Sprite3dError::TooManyLayers(self.layers.len()));
        }
        if self.custom_size.is_some_and(|v| !valid_size(v))
            || !valid_color(self.color)
            || !valid_color(self.backface_color)
        {
            return Err(Sprite3dError::InvalidGeometry);
        }
        for (index, layer) in self.layers.iter().enumerate() {
            if !valid_rect(layer.bounds) || !valid_color(layer.color) {
                return Err(Sprite3dError::InvalidLayer(index));
            }
            let reference = match layer.mask {
                MaskMode::None => continue,
                MaskMode::Read(reference) => reference,
                MaskMode::Write {
                    reference, cutoff, ..
                }
                | MaskMode::StencilWrite {
                    reference, cutoff, ..
                } => {
                    if !cutoff.is_finite() || !(0.0..=1.0).contains(&cutoff) {
                        return Err(Sprite3dError::InvalidLayer(index));
                    }
                    reference
                }
            };
            if reference == 0 || reference > MAX_MASKS {
                return Err(Sprite3dError::InvalidMask(reference));
            }
        }
        Ok(())
    }
    pub(crate) fn resolve_rects(
        &self,
        atlases: &Assets<TextureAtlasLayout>,
    ) -> Result<[Option<Rect>; MAX_LAYERS], Sprite3dError> {
        self.validate()?;
        let mut rects = [None; MAX_LAYERS];
        let fallback = [SpriteLayer::default()];
        let layers = if self.layers.is_empty() {
            &fallback[..]
        } else {
            &self.layers
        };
        for (i, layer) in layers.iter().enumerate() {
            let atlas = layer.texture_atlas.as_ref().or(self.texture_atlas.as_ref());
            rects[i] = match atlas {
                None => None,
                Some(atlas) => {
                    let layout = atlases
                        .get(&atlas.layout)
                        .ok_or(Sprite3dError::MissingAtlas)?;
                    let cell = layout
                        .textures
                        .get(atlas.index)
                        .ok_or(Sprite3dError::AtlasIndex {
                            index: atlas.index,
                            length: layout.textures.len(),
                        })?
                        .as_rect();
                    if !valid_rect(cell)
                        || cell.max.x > layout.size.x as f32
                        || cell.max.y > layout.size.y as f32
                    {
                        return Err(Sprite3dError::InvalidLayer(i));
                    }
                    Some(cell)
                }
            };
        }
        Ok(rects)
    }
}
fn valid_size(v: Vec2) -> bool {
    v.is_finite() && v.min_element() > 0.0
}
fn valid_rect(r: Rect) -> bool {
    r.min.is_finite() && r.max.is_finite() && valid_size(r.size())
}
fn valid_color(color: Color) -> bool {
    let rgba = color.to_linear().to_f32_array();
    rgba.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&rgba[3])
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum Sprite3dError {
    #[error("texture atlas layout is not loaded")]
    MissingAtlas,
    #[error("texture atlas index {index} is out of range for {length} cells")]
    AtlasIndex { index: usize, length: usize },
    #[error("sprite has {0} layers; at most 32 are supported")]
    TooManyLayers(usize),
    #[error("invalid sprite size or tint: size must be positive and finite, alpha in 0..=1")]
    InvalidGeometry,
    #[error("sprite layer {0} has invalid geometry, tint or mask cutoff")]
    InvalidLayer(usize),
    #[error("mask reference {0} is outside 1..=8")]
    InvalidMask(u8),
}

#[derive(Component)]
pub(crate) struct RenderedSprite {
    size: Vec2,
    rects: [Option<Rect>; MAX_LAYERS],
}

pub(crate) fn sync_sprites(
    mut commands: Commands,
    images: Res<Assets<Image>>,
    atlases: Res<Assets<TextureAtlasLayout>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<Sprite3dMaterial>>,
    sprites: Query<(
        Entity,
        Ref<Sprite3d>,
        Option<&RenderedSprite>,
        Option<&Mesh3d>,
        Option<&MeshMaterial3d<Sprite3dMaterial>>,
    )>,
    removed: Query<Entity, (With<RenderedSprite>, Without<Sprite3d>)>,
) {
    for entity in &removed {
        commands
            .entity(entity)
            .remove::<(RenderedSprite, Mesh3d, MeshMaterial3d<Sprite3dMaterial>)>();
    }
    for (entity, sprite, rendered, mesh, material) in &sprites {
        let rects = match sprite.resolve_rects(&atlases) {
            Ok(rects) => rects,
            Err(error) => {
                if error != Sprite3dError::MissingAtlas
                    && (sprite.is_changed() || atlases.is_changed())
                {
                    warn!("invalid Sprite3d on {entity}: {error}");
                }
                commands
                    .entity(entity)
                    .remove::<(RenderedSprite, Mesh3d, MeshMaterial3d<Sprite3dMaterial>)>();
                continue;
            }
        };
        let atlas_changed = rendered.is_none_or(|old| old.rects != rects);
        let size = sprite
            .custom_size
            .or_else(|| match sprite.layers.as_slice() {
                [layer] if layer.bounds == Rect::from_corners(Vec2::ZERO, Vec2::ONE) => {
                    rects[0].map(|rect| rect.size())
                }
                [] => rects[0].map(|rect| rect.size()),
                _ => None,
            })
            .or_else(|| {
                sprite
                    .image
                    .as_ref()
                    .and_then(|h| images.get(h))
                    .map(Image::size_f32)
            });
        // Never flash a unit quad while waiting for natural image dimensions.
        let Some(size) = size.or_else(|| sprite.image.is_none().then_some(Vec2::ONE)) else {
            commands
                .entity(entity)
                .remove::<(RenderedSprite, Mesh3d, MeshMaterial3d<Sprite3dMaterial>)>();
            continue;
        };
        if rendered.is_none_or(|old| old.size != size) || mesh.is_none() {
            let geometry = Mesh::from(Rectangle::new(size.x, size.y));
            if let Some(mut mesh) = mesh.and_then(|h| meshes.get_mut(&h.0)) {
                *mesh = geometry;
            } else {
                commands.entity(entity).insert(Mesh3d(meshes.add(geometry)));
            }
        }
        if rendered.is_none_or(|old| old.size != size) || atlas_changed {
            commands
                .entity(entity)
                .insert(RenderedSprite { size, rects });
        }
        if let Some(handle) = material {
            if sprite.is_changed() || atlas_changed || !materials.contains(&handle.0) {
                if let Some(mut asset) = materials.get_mut(&handle.0) {
                    *asset = Sprite3dMaterial::from_resolved(&sprite, &rects);
                } else {
                    commands.entity(entity).insert(MeshMaterial3d(
                        materials.add(Sprite3dMaterial::from_resolved(&sprite, &rects)),
                    ));
                }
            }
        } else {
            commands.entity(entity).insert(MeshMaterial3d(
                materials.add(Sprite3dMaterial::from_resolved(&sprite, &rects)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<TextureAtlasLayout>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Sprite3dMaterial>>()
            .add_systems(Update, sync_sprites);
        app
    }

    #[test]
    fn atlas_selection_hot_reload_and_recovery_update_the_rendered_rect() {
        let mut app = app();
        let layout = app
            .world_mut()
            .resource_mut::<Assets<TextureAtlasLayout>>()
            .add(TextureAtlasLayout::from_grid(
                UVec2::new(16, 8),
                2,
                1,
                None,
                None,
            ));
        let entity = app
            .world_mut()
            .spawn(Sprite3d::from_atlas(
                Handle::default(),
                TextureAtlas {
                    layout: layout.clone(),
                    index: 0,
                },
            ))
            .id();
        app.update();
        let mesh = app.world().get::<Mesh3d>(entity).expect("mesh").0.clone();
        assert_eq!(
            app.world()
                .get::<RenderedSprite>(entity)
                .expect("resolved")
                .size,
            Vec2::new(16.0, 8.0)
        );
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .texture_atlas
            .as_mut()
            .expect("atlas")
            .index = 1;
        app.update();
        assert_eq!(
            app.world()
                .get::<RenderedSprite>(entity)
                .expect("resolved")
                .rects[0],
            Some(Rect::new(16.0, 0.0, 32.0, 8.0))
        );
        // Layout edits are visible even when Sprite3d itself has not changed.
        app.world_mut()
            .resource_mut::<Assets<TextureAtlasLayout>>()
            .get_mut(&layout)
            .expect("layout")
            .textures[1] = URect::new(20, 0, 30, 8);
        app.update();
        assert_eq!(
            app.world()
                .get::<RenderedSprite>(entity)
                .expect("resolved")
                .size,
            Vec2::new(10.0, 8.0)
        );
        assert_eq!(
            app.world()
                .get::<Mesh3d>(entity)
                .expect("same mesh asset")
                .0,
            mesh
        );
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .texture_atlas
            .as_mut()
            .expect("atlas")
            .index = 2;
        app.update();
        assert!(
            app.world().get::<Mesh3d>(entity).is_none(),
            "invalid index must not render the whole atlas"
        );
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .texture_atlas
            .as_mut()
            .expect("atlas")
            .index = 0;
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_some());
    }

    #[test]
    fn layers_can_override_the_shared_atlas() {
        let mut layouts = Assets::<TextureAtlasLayout>::default();
        let layout = layouts.add(TextureAtlasLayout::from_grid(
            UVec2::splat(16),
            2,
            1,
            None,
            None,
        ));
        let mut sprite = Sprite3d::from_atlas(
            Handle::default(),
            TextureAtlas {
                layout: layout.clone(),
                index: 0,
            },
        );
        sprite.layers = vec![
            SpriteLayer::default(),
            SpriteLayer {
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index: 1,
                }),
                ..default()
            },
        ];
        let rects = sprite.resolve_rects(&layouts).expect("resolve layer cells");
        assert_eq!(rects[0], Some(Rect::new(0.0, 0.0, 16.0, 16.0)));
        assert_eq!(rects[1], Some(Rect::new(16.0, 0.0, 32.0, 16.0)));
        layouts.get_mut(&layout).expect("layout").textures[1] = URect::new(16, 0, 33, 16);
        assert_eq!(
            sprite.resolve_rects(&layouts),
            Err(Sprite3dError::InvalidLayer(1))
        );
    }

    #[test]
    fn late_atlas_assets_do_not_flash_the_whole_image() {
        let mut app = app();
        let handle = app
            .world()
            .resource::<Assets<TextureAtlasLayout>>()
            .reserve_handle();
        let entity = app
            .world_mut()
            .spawn(Sprite3d {
                texture_atlas: Some(TextureAtlas {
                    layout: handle.clone(),
                    index: 0,
                }),
                custom_size: Some(Vec2::splat(2.0)),
                ..default()
            })
            .id();
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_none());
        app.world_mut()
            .resource_mut::<Assets<TextureAtlasLayout>>()
            .insert(
                handle.id(),
                TextureAtlasLayout::from_grid(UVec2::splat(16), 1, 1, None, None),
            )
            .expect("insert delayed layout");
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_some());
    }

    #[test]
    fn validates_layer_limits_geometry_and_mask_references() {
        let mut sprite = Sprite3d::default();
        assert!(sprite.validate().is_ok());
        sprite.layers = vec![SpriteLayer::default(); MAX_LAYERS + 1];
        assert!(matches!(
            sprite.validate(),
            Err(Sprite3dError::TooManyLayers(_))
        ));
        sprite.layers.truncate(1);
        sprite.layers[0].mask = MaskMode::Read(0);
        assert_eq!(sprite.validate(), Err(Sprite3dError::InvalidMask(0)));
        sprite.layers[0].mask = MaskMode::Read(MAX_MASKS);
        assert!(sprite.validate().is_ok());
        sprite.layers[0].bounds.max.x = f32::NAN;
        assert_eq!(sprite.validate(), Err(Sprite3dError::InvalidLayer(0)));
    }

    #[test]
    fn sync_reuses_assets_preserves_transform_and_cleans_up_removal() {
        let mut app = app();
        let transform = Transform::from_xyz(1.0, 2.0, 3.0).with_scale(Vec3::splat(2.0));
        let entity = app
            .world_mut()
            .spawn((
                Sprite3d::from_color(Color::WHITE, Vec2::splat(4.0)),
                transform,
            ))
            .id();
        app.update();
        let mesh = app.world().get::<Mesh3d>(entity).expect("quad").0.clone();
        let material = app
            .world()
            .get::<MeshMaterial3d<Sprite3dMaterial>>(entity)
            .expect("material")
            .0
            .clone();
        app.update();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
        assert_eq!(app.world().resource::<Assets<Sprite3dMaterial>>().len(), 1);
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .custom_size = Some(Vec2::splat(8.0));
        app.update();
        assert_eq!(
            app.world().get::<Mesh3d>(entity).expect("same mesh").0,
            mesh
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<Sprite3dMaterial>>(entity)
                .expect("same material")
                .0,
            material
        );
        assert_eq!(
            *app.world().get::<Transform>(entity).expect("transform"),
            transform
        );
        app.world_mut().entity_mut(entity).remove::<Sprite3d>();
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_none());
        assert!(
            app.world()
                .get::<MeshMaterial3d<Sprite3dMaterial>>(entity)
                .is_none()
        );
        assert!(app.world().get::<Transform>(entity).is_some());
    }

    #[test]
    fn invalid_edit_removes_old_rendering_until_repaired() {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn(Sprite3d::from_color(Color::WHITE, Vec2::ONE))
            .id();
        app.update();
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .custom_size = Some(Vec2::ZERO);
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_none());
        app.world_mut()
            .get_mut::<Sprite3d>(entity)
            .expect("sprite")
            .custom_size = Some(Vec2::ONE);
        app.update();
        assert!(app.world().get::<Mesh3d>(entity).is_some());
    }
}
