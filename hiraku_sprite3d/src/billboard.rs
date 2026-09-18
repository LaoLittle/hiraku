use bevy::{prelude::*, transform::TransformSystems};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum BillboardMode {
    /// Parallel to the camera image plane, suitable for orthographic cameras.
    #[default]
    ScreenAligned,
    /// Front points toward camera position, world +Y up.
    Spherical,
    /// Rotate only around world +Y.
    CylindricalY,
}

/// Owns rotation, never translation/scale. Works on any mesh, not just Sprite3d.
/// Camera selection is explicit. Rotated parents are supported; ancestors must
/// have positive uniform scale (no shear).
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component)]
#[require(Transform)]
pub struct Billboard {
    pub camera: Entity,
    pub mode: BillboardMode,
    /// Local Z rotation in radians.
    pub roll: f32,
}

impl Billboard {
    pub fn new(camera: Entity) -> Self {
        Self {
            camera,
            mode: BillboardMode::ScreenAligned,
            roll: 0.0,
        }
    }
}

pub struct BillboardPlugin;
impl Plugin for BillboardPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Billboard>()
            .register_type::<BillboardMode>()
            .add_systems(
                PostUpdate,
                orient_billboards.before(TransformSystems::Propagate),
            );
    }
}

fn facing(mode: BillboardMode, position: Vec3, camera: &GlobalTransform) -> Option<Quat> {
    if mode == BillboardMode::ScreenAligned {
        return Some(camera.rotation());
    }
    let mut forward = camera.translation() - position;
    if mode == BillboardMode::CylindricalY {
        forward.y = 0.0;
    }
    let forward = forward.try_normalize()?;
    let up = if forward.dot(Vec3::Y).abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let right = up.cross(forward).try_normalize()?;
    Some(Quat::from_mat3(&Mat3::from_cols(
        right,
        forward.cross(right),
        forward,
    )))
}

fn orient_billboards(
    billboards: Query<(Entity, &Billboard, Option<&ChildOf>)>,
    cameras: Query<(), With<Camera>>,
    mut transforms: ParamSet<(TransformHelper, Query<&mut Transform>)>,
    mut updates: Local<Vec<(Entity, Quat)>>,
) {
    updates.clear();
    {
        let helper = transforms.p0();
        for (entity, billboard, parent) in &billboards {
            if !billboard.roll.is_finite() || !cameras.contains(billboard.camera) {
                continue;
            }
            let (Ok(camera), Ok(current)) = (
                helper.compute_global_transform(billboard.camera),
                helper.compute_global_transform(entity),
            ) else {
                continue;
            };
            let Some(rotation) = facing(billboard.mode, current.translation(), &camera) else {
                continue;
            };
            let parent_rotation = match parent {
                Some(parent) => match helper.compute_global_transform(parent.parent()) {
                    Ok(t) => t.rotation(),
                    Err(_) => continue,
                },
                None => Quat::IDENTITY,
            };
            updates.push((
                entity,
                parent_rotation.inverse() * rotation * Quat::from_rotation_z(billboard.roll),
            ));
        }
    }
    let mut writable = transforms.p1();
    for (entity, rotation) in updates.drain(..) {
        if let Ok(mut transform) = writable.get_mut(entity) {
            if !transform.rotation.abs_diff_eq(rotation, 1e-6) {
                transform.rotation = rotation;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facing_modes_and_degenerate_positions() {
        let camera = GlobalTransform::from(Transform::from_xyz(4.0, 3.0, 5.0));
        let rotation = facing(BillboardMode::Spherical, Vec3::ZERO, &camera).expect("direction");
        assert!((rotation * Vec3::Z).abs_diff_eq(camera.translation().normalize(), 1e-6));
        let rotation = facing(BillboardMode::CylindricalY, Vec3::ZERO, &camera).expect("direction");
        assert!((rotation * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-6));
        assert!(facing(BillboardMode::Spherical, camera.translation(), &camera).is_none());
        let overhead = GlobalTransform::from(Transform::from_xyz(0.0, 5.0, 0.0));
        assert!(
            facing(BillboardMode::Spherical, Vec3::ZERO, &overhead)
                .expect("pole fallback")
                .is_finite()
        );
    }

    #[test]
    fn rotated_parent_and_camera_movement_use_current_frame_transforms() {
        let mut app = App::new();
        app.add_plugins((bevy::transform::TransformPlugin, BillboardPlugin));
        let camera = app
            .world_mut()
            .spawn((
                Camera::default(),
                Transform::from_rotation(Quat::from_rotation_y(0.6)),
            ))
            .id();
        let parent = app
            .world_mut()
            .spawn(Transform::from_rotation(Quat::from_rotation_z(0.4)))
            .id();
        let local_position = Vec3::new(1.0, 2.0, 3.0);
        let entity = app
            .world_mut()
            .spawn((
                Billboard::new(camera),
                Transform::from_translation(local_position),
                ChildOf(parent),
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<GlobalTransform>(entity)
                .expect("world transform")
                .rotation()
                .abs_diff_eq(Quat::from_rotation_y(0.6), 1e-5)
        );
        app.world_mut()
            .get_mut::<Transform>(camera)
            .expect("camera")
            .rotation = Quat::from_rotation_x(0.3);
        app.update();
        assert!(
            app.world()
                .get::<GlobalTransform>(entity)
                .expect("world transform")
                .rotation()
                .abs_diff_eq(Quat::from_rotation_x(0.3), 1e-5)
        );
        assert_eq!(
            app.world()
                .get::<Transform>(entity)
                .expect("local transform")
                .translation,
            local_position
        );
        app.world_mut().entity_mut(camera).despawn();
        app.update(); // A removed camera must not panic or despawn the sprite.
        assert!(app.world().get_entity(entity).is_ok());
    }
}
