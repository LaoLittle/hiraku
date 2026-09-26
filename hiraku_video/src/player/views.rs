//! Non-owning spatial views of a playback. Only GPU image handles are shared;
//! each view owns its mesh/material, opacity and transform, but no decoder.
use super::*;

#[derive(Component, Clone, Debug)]
#[require(Transform, Visibility)]
pub struct VideoWorldView {
    playback: VideoPlaybackId,
    size: Vec2,
    opacity: f32,
}

impl VideoWorldView {
    pub fn new(playback: VideoPlaybackId, size: Vec2) -> Result<Self, &'static str> {
        if !size.is_finite() || size.min_element() <= 0.0 {
            return Err("video view size must be finite and positive");
        }
        Ok(Self {
            playback,
            size,
            opacity: 1.0,
        })
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<(), &'static str> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            return Err("video view opacity must be finite and within 0..=1");
        }
        self.opacity = opacity;
        Ok(())
    }

    pub fn playback(&self) -> VideoPlaybackId {
        self.playback
    }
}

#[derive(Resource, Default)]
struct WorldFrames(BTreeMap<VideoPlaybackId, (Yuv420Material, RenderLayers)>);

#[derive(Component)]
struct ViewSurface {
    child: Entity,
    material: Handle<Yuv420Material>,
    size: Vec2,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<WorldFrames>().add_systems(
        PostUpdate,
        (apply_reparents, publish_world_frames, sync_views)
            .chain()
            .before(bevy::transform::TransformSystems::Propagate),
    );
}

// Apply transfers after host Update commands but before transform propagation
// and surface publication, so the old parent never renders both primary and
// mirror for one frame. This queue only changes hierarchy, not decode controls.
fn apply_reparents(
    mut commands: Commands,
    mut player: ResMut<VideoPlayer>,
    active: NonSend<ActiveVideo>,
    parents: Query<Entity>,
) {
    for (id, parent) in player.reparents.drain(..) {
        if parents.contains(parent)
            && let Some(playback) = active.0.get(&id)
            && playback.world.is_some()
        {
            commands.entity(playback.root).try_insert(ChildOf(parent));
        }
    }
}

fn publish_world_frames(
    active: NonSend<ActiveVideo>,
    surfaces: Query<&MeshMaterial3d<Yuv420Material>>,
    materials: Res<Assets<Yuv420Material>>,
    mut frames: ResMut<WorldFrames>,
) {
    frames.0.clear();
    for (&id, playback) in &active.0 {
        let (Some((_, layers)), Some(surface)) = (&playback.world, &playback.surface) else {
            continue;
        };
        let entity = match surface {
            VideoSurface::YuvI420 { image_entity, .. }
            | VideoSurface::YuvNv12 { image_entity, .. }
            | VideoSurface::Rgba { image_entity, .. } => *image_entity,
        };
        let Ok(handle) = surfaces.get(entity) else {
            continue;
        };
        let Some(material) = materials.get(&handle.0) else {
            continue;
        };
        let mut material = material.clone();
        // Do not inherit the primary surface's host opacity. A primary fading
        // in from zero must not also fade out all its outgoing views.
        material.opacity = playback
            .exit_elapsed
            .map_or(1.0, |e| exit_opacity(e, playback.fade_out));
        frames.0.insert(id, (material, layers.clone()));
    }
}

fn sync_views(
    mut commands: Commands,
    frames: Res<WorldFrames>,
    views: Query<
        (
            Entity,
            Option<&VideoWorldView>,
            Option<&ViewSurface>,
            Option<&RenderLayers>,
        ),
        Or<(With<VideoWorldView>, With<ViewSurface>)>,
    >,
    children: Query<Entity>,
    mut materials: ResMut<Assets<Yuv420Material>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for (entity, view, current, own_layers) in &views {
        let source = view.and_then(|view| frames.0.get(&view.playback).map(|frame| (view, frame)));
        let Some((view, (source, layers))) = source else {
            if let Some(current) = current {
                commands.entity(current.child).try_despawn();
                commands.entity(entity).try_remove::<ViewSurface>();
            }
            continue;
        };
        let mut value = source.clone();
        value.opacity *= view.opacity;
        let layers = own_layers.unwrap_or(layers).clone();
        if let Some(current) = current.filter(|current| children.contains(current.child)) {
            if let Some(mut material) = materials.get_mut(&current.material) {
                if *material != value {
                    *material = value;
                }
            } else {
                // A manually removed material is rebuilt through the same path.
                commands.entity(current.child).try_despawn();
                commands.entity(entity).try_remove::<ViewSurface>();
                continue;
            }
            if current.size != view.size {
                commands
                    .entity(current.child)
                    .try_insert(Mesh3d(meshes.add(Rectangle::from_size(view.size))));
                commands.entity(entity).try_insert(ViewSurface {
                    child: current.child,
                    material: current.material.clone(),
                    size: view.size,
                });
            }
            commands.entity(current.child).try_insert(layers);
        } else {
            let material = materials.add(value);
            let child = commands
                .spawn((
                    Mesh3d(meshes.add(Rectangle::from_size(view.size))),
                    MeshMaterial3d(material.clone()),
                    layers,
                    Transform::default(),
                    Visibility::Inherited,
                    Pickable::IGNORE,
                    ChildOf(entity),
                ))
                .id();
            commands.entity(entity).try_insert(ViewSurface {
                child,
                material,
                size: view.size,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_share_frame_planes_but_not_opacity_or_lifetime() {
        let mut app = App::new();
        app.init_resource::<WorldFrames>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Yuv420Material>>()
            .add_systems(Update, sync_views);
        let id = VideoPlaybackId(7);
        let source = Yuv420Material {
            opacity: 0.8,
            y: Handle::default(),
            chroma0: Handle::default(),
            chroma1: Handle::default(),
            color_transform: crate::color::YuvColorTransform {
                row_r: Vec4::X,
                row_g: Vec4::Y,
                row_b: Vec4::Z,
                luma: Vec2::ONE,
            },
            transfer: hiraku_media::TransferFunction::Srgb,
            format: YuvPixelFormat::I420,
            alpha_layout: None,
            rgba: false,
        };
        app.world_mut()
            .resource_mut::<WorldFrames>()
            .0
            .insert(id, (source.clone(), RenderLayers::layer(3)));
        let alice = app
            .world_mut()
            .spawn(VideoWorldView::new(id, Vec2::ONE).expect("view"))
            .id();
        let mut bob_view = VideoWorldView::new(id, Vec2::new(2.0, 3.0)).expect("view");
        bob_view.set_opacity(0.25).expect("opacity");
        let bob = app.world_mut().spawn(bob_view).id();
        app.update();
        let first = app.world().get::<ViewSurface>(alice).expect("surface");
        let first_child = first.child;
        let first_material = first.material.clone();
        let second = app.world().get::<ViewSurface>(bob).expect("surface");
        let second_child = second.child;
        let second_material = second.material.clone();
        assert_ne!(first_material.id(), second_material.id());
        let materials = app.world().resource::<Assets<Yuv420Material>>();
        let a = materials.get(&first_material).expect("material");
        let b = materials.get(&second_material).expect("material");
        assert_eq!(a.opacity, 0.8);
        assert_eq!(b.opacity, 0.2);
        assert_eq!(
            (&a.y, &a.chroma0, &a.chroma1),
            (&b.y, &b.chroma0, &b.chroma1)
        );
        app.update();
        assert_eq!(
            app.world()
                .get::<ViewSurface>(bob)
                .expect("stable surface")
                .child,
            second_child
        );
        app.world_mut().despawn(alice);
        app.update();
        assert!(app.world().get_entity(first_child).is_err());
        assert!(app.world().get_entity(second_child).is_ok());
        assert!(app.world().resource::<WorldFrames>().0.contains_key(&id));
        app.world_mut().resource_mut::<WorldFrames>().0.clear();
        app.update();
        assert!(app.world().get_entity(second_child).is_err());
        assert!(app.world().get_entity(bob).is_ok());
    }
}
