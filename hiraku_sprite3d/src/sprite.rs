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

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Sprite3dMaterial>>()
            .add_systems(Update, sync_sprites);
        app
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum MaskMode {
    #[default]
    None,
    /// Read alpha coverage from earlier writers. No matching writer = zero.
    Read(u8),
    /// Union coverage. Alpha includes layer tint; cutoff does not make it opaque.
    Write {
        reference: u8,
        cutoff: f32,
        visible: bool,
    },
}

#[derive(Clone, Debug, Reflect)]
pub struct SpriteLayer {
    /// Source atlas rectangle in pixels, top-left origin. None = full image.
    pub rect: Option<Rect>,
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
            rect: None,
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
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
#[require(Transform, Visibility)]
pub struct Sprite3d {
    pub image: Option<Handle<Image>>,
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
            image: None,
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
    pub fn from_atlas(image: Handle<Image>, rect: Rect) -> Self {
        Self {
            image: Some(image),
            layers: vec![SpriteLayer {
                rect: Some(rect),
                ..default()
            }],
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
            if !valid_rect(layer.bounds)
                || layer
                    .rect
                    .is_some_and(|r| !valid_rect(r) || r.min.min_element() < 0.0)
                || !valid_color(layer.color)
            {
                return Err(Sprite3dError::InvalidLayer(index));
            }
            let reference = match layer.mask {
                MaskMode::None => continue,
                MaskMode::Read(reference) => reference,
                MaskMode::Write {
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
}

pub(crate) fn sync_sprites(
    mut commands: Commands,
    images: Res<Assets<Image>>,
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
        if let Err(error) = sprite.validate() {
            if sprite.is_changed() {
                warn!("invalid Sprite3d on {entity}: {error}");
            }
            commands
                .entity(entity)
                .remove::<(RenderedSprite, Mesh3d, MeshMaterial3d<Sprite3dMaterial>)>();
            continue;
        }
        let size = sprite
            .custom_size
            .or_else(|| match sprite.layers.as_slice() {
                [layer] if layer.bounds == Rect::from_corners(Vec2::ZERO, Vec2::ONE) => {
                    layer.rect.map(|rect| rect.size())
                }
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
            commands.entity(entity).insert(RenderedSprite { size });
        }
        if let Some(handle) = material {
            if sprite.is_changed() || !materials.contains(&handle.0) {
                if let Some(mut asset) = materials.get_mut(&handle.0) {
                    *asset = Sprite3dMaterial::from_sprite(&sprite);
                } else {
                    commands.entity(entity).insert(MeshMaterial3d(
                        materials.add(Sprite3dMaterial::from_sprite(&sprite)),
                    ));
                }
            }
        } else {
            commands.entity(entity).insert(MeshMaterial3d(
                materials.add(Sprite3dMaterial::from_sprite(&sprite)),
            ));
        }
    }
}
