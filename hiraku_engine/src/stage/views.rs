//! On-demand spatial view targets, composed beneath curtains and declarative UI.
//! Hidden views keep their simulation state but do not render GPU frames.
use crate::render::world_sprite::WorldSprite;
use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};
use std::collections::BTreeMap;

pub(crate) fn spatial_layer() -> RenderLayers {
    RenderLayers::layer(4)
}

#[derive(Component)]
pub(super) struct ViewCamera;

struct ViewEntities {
    camera: Entity,
    surface: Entity,
    _image: Handle<Image>,
}

#[derive(Resource, Default)]
pub(super) struct StageViews {
    owner: Option<(u64, String)>,
    root: Option<Entity>,
    views: BTreeMap<String, ViewEntities>,
}

pub(super) fn sync(
    mut commands: Commands,
    shared: Res<crate::state::SceneSharedState>,
    stage: Res<super::runtime::StageRuntime>,
    canvas: Res<crate::HirakuCanvas>,
    mut images: ResMut<Assets<Image>>,
    mut rendered: ResMut<StageViews>,
    primary: Query<
        &Camera,
        (
            With<crate::render::camera::WorldCamera3d>,
            Without<ViewCamera>,
        ),
    >,
    mut cameras: Query<(&mut Camera, &mut Transform, &mut Projection, &mut bevy::camera::Exposure, Option<&AmbientLight>), With<ViewCamera>>,
    mut surfaces: Query<&mut WorldSprite>,
    mut redraw: crate::redraw::Redraw,
) {
    let state = shared.0.spatial_stage.as_ref();
    let owner = state.map(|s| (s.id, s.path.clone()));
    if owner != rendered.owner {
        if let Some(root) = rendered.root.take() {
            commands.entity(root).try_despawn();
        }
        rendered.views.clear();
        rendered.owner = owner;
    }
    let Some(state) = state else { return };
    if !stage.ready(state) {
        return;
    }
    let Ok(primary) = primary.single() else {
        return;
    };
    let root = *rendered.root.get_or_insert_with(|| {
        commands
            .spawn((Transform::default(), Visibility::default()))
            .id()
    });
    let exposure = stage.definition.as_ref().map(|d| d.camera_exposure()).unwrap_or_default();
    let ambient = stage.definition.as_ref().and_then(|d| d.ambient_brightness);
    for (name, view) in &state.views {
        let Some(preset) = &view.camera else { continue };
        let visible = view.alpha > 0.0 || view.fade.is_some();
        let next = preset.projection.projection();
        if let Some(entities) = rendered.views.get(name) {
            if let Ok((mut camera, mut pose, mut lens, mut current_exposure, current_ambient)) = cameras.get_mut(entities.camera) {
                if current_ambient.map(|light| light.brightness) != ambient {
                    if let Some(brightness) = ambient {
                        commands.entity(entities.camera).insert(AmbientLight { brightness, ..default() });
                    } else {
                        commands.entity(entities.camera).remove::<AmbientLight>();
                    }
                }
                if current_exposure.ev100 != exposure.ev100 { *current_exposure = exposure; }
                if camera.is_active != visible {
                    camera.is_active = visible;
                }
                pose.set_if_neq(preset.pose.transform());
                if super::runtime::projection_kind_values(&lens)
                    != super::runtime::projection_kind_values(&next)
                {
                    *lens = next;
                }
            }
            if let Ok(mut surface) = surfaces.get_mut(entities.surface) {
                let clip = view.clip.plane(canvas.size.as_vec2());
                if surface.clip_plane != clip { surface.clip_plane = clip; }
                let color = Color::linear_rgba(1.0, 1.0, 1.0, view.alpha);
                if surface.color != color {
                    surface.color = color;
                }
            }
            continue;
        }
        redraw.request();
        let image = images.add(Image::new_target_texture(
            canvas.size.x,
            canvas.size.y,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        ));
        let camera = commands
            .spawn((
                ViewCamera,
                Camera3d::default(),
                exposure,
                Camera {
                    order: primary.order - 1 - view.order as isize,
                    is_active: visible,
                    clear_color: ClearColorConfig::Custom(Color::BLACK),
                    ..default()
                },
                RenderTarget::Image(image.clone().into()),
                preset.pose.transform(),
                next,
                spatial_layer(),
                ChildOf(root),
            ))
            .id();
        if let Some(brightness) = ambient {
            commands.entity(camera).insert(AmbientLight { brightness, ..default() });
        }
        let mut sprite = WorldSprite::from_image(image.clone());
        sprite.clip_plane = view.clip.plane(canvas.size.as_vec2());
        sprite.custom_size = Some(canvas.size.as_vec2());
        sprite.color = Color::linear_rgba(1.0, 1.0, 1.0, view.alpha);
        let surface = commands
            .spawn((
                sprite,
                Pickable::IGNORE,
                Transform::from_xyz(0.0, 0.0, view.order as f32),
                Visibility::default(),
                crate::render::camera::scene_layer(),
                ChildOf(root),
            ))
            .id();
        rendered.views.insert(
            name.clone(),
            ViewEntities {
                camera,
                surface,
                _image: image,
            },
        );
    }
}
