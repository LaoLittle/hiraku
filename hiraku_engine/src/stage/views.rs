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

#[derive(Component)]
pub(super) struct ViewSurface;

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
    mut cameras: Query<
        (
            &mut Camera,
            &mut Transform,
            &mut Projection,
            &mut bevy::camera::Exposure,
            Option<&AmbientLight>,
        ),
        With<ViewCamera>,
    >,
    mut surfaces: Query<&mut WorldSprite>,
    mut redraw: super::redraw::StageRedraw,
    camera_state: Option<Res<crate::render::camera::CameraState>>,
) {
    let state = shared.0.spatial_stage.as_ref();
    let owner = state.map(|s| (s.id, s.path.clone()));
    if owner != rendered.owner {
        redraw.request();
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
    let exposure = stage
        .definition
        .as_ref()
        .map(|d| d.camera_exposure())
        .unwrap_or_default();
    let ambient = stage.definition.as_ref().and_then(|d| d.ambient_brightness);
    for (name, view) in &state.views {
        let Some(preset) = &view.camera else { continue };
        let visible = view.alpha > 0.0 || view.fade.is_some();
        let (pose, next) = view_camera(preset, camera_state.as_deref());
        if let Some(entities) = rendered.views.get(name) {
            if let Ok((
                mut camera,
                mut current_pose,
                mut lens,
                mut current_exposure,
                current_ambient,
            )) = cameras.get_mut(entities.camera)
            {
                if current_ambient.map(|light| light.brightness) != ambient {
                    redraw.request();
                    if let Some(brightness) = ambient {
                        commands.entity(entities.camera).insert(AmbientLight {
                            brightness,
                            ..default()
                        });
                    } else {
                        commands.entity(entities.camera).remove::<AmbientLight>();
                    }
                }
                if current_exposure.ev100 != exposure.ev100 {
                    redraw.request();
                    *current_exposure = exposure;
                }
                if camera.is_active != visible {
                    redraw.request();
                    camera.is_active = visible;
                }
                if current_pose.set_if_neq(pose) {
                    redraw.request();
                }
                if super::runtime::projection_kind_values(&lens)
                    != super::runtime::projection_kind_values(&next)
                {
                    *lens = next;
                    redraw.request();
                }
            }
            if let Ok(mut surface) = surfaces.get_mut(entities.surface) {
                let clip = view.clip.plane(canvas.size.as_vec2());
                if surface.clip_plane != clip {
                    redraw.request();
                    surface.clip_plane = clip;
                }
                let color = Color::linear_rgba(1.0, 1.0, 1.0, view.alpha);
                if surface.color != color {
                    redraw.request();
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
                // This view is composed as an SDR image. Do not inherit the
                // cinematic tone mapper required by Camera3d and compress the
                // authored colors before the presentation pass samples them.
                bevy::core_pipeline::tonemapping::Tonemapping::None,
                bevy::core_pipeline::tonemapping::DebandDither::Disabled,
                exposure,
                Camera {
                    order: primary.order - 1 - view.order as isize,
                    is_active: visible,
                    clear_color: ClearColorConfig::Custom(Color::BLACK),
                    ..default()
                },
                RenderTarget::Image(image.clone().into()),
                pose,
                next,
                spatial_layer(),
                ChildOf(root),
            ))
            .id();
        if let Some(brightness) = ambient {
            commands.entity(camera).insert(AmbientLight {
                brightness,
                ..default()
            });
        }
        let mut sprite = WorldSprite::from_image(image.clone());
        sprite.clip_plane = view.clip.plane(canvas.size.as_vec2());
        sprite.custom_size = Some(canvas.size.as_vec2());
        sprite.color = Color::linear_rgba(1.0, 1.0, 1.0, view.alpha);
        let surface = commands
            .spawn((
                sprite,
                ViewSurface,
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

// Compose story camera effects with the authored stage lens, never with the
// previous frame. Canvas pictures and UI retain their presentation coordinates.
fn view_camera(
    preset: &super::StageCamera,
    state: Option<&crate::render::camera::CameraState>,
) -> (Transform, Projection) {
    let mut pose = preset.pose.transform();
    let mut projection = preset.projection.projection();
    if let Some(state) = state {
        let zoom = if matches!(state.effect_scope, crate::script::CameraEffectScope::Canvas) {
            1.0
        } else {
            state.zoom.max(0.01)
        };
        match &mut projection {
            Projection::Orthographic(lens) => lens.scale /= zoom,
            Projection::Perspective(lens) => {
                lens.fov = (lens.fov / zoom).clamp(5_f32.to_radians(), 170_f32.to_radians());
            }
            _ => {}
        }
        pose.translation += pose.rotation * state.offset;
        pose.rotation *= Quat::from_euler(
            EulerRot::XYZ,
            state.rotation.x.to_radians(),
            state.rotation.y.to_radians(),
            state.rotation.z.to_radians(),
        );
    }
    (pose, projection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_zoom_uses_authored_lens_and_canvas_zoom_is_not_applied_twice() {
        let mut preset = super::super::StageCamera {
            pose: super::super::StageCameraPose::Fixed(super::super::StagePose {
                position: (0.0, 0.0, 10.0),
                rotation: (0.0, 0.0, 0.0),
                scale: (1.0, 1.0, 1.0),
            }),
            projection: super::super::StageProjection::Perspective {
                fov: 48.0,
                near: 0.1,
                far: 1000.0,
            },
        };
        let mut state = crate::render::camera::CameraState {
            zoom: 2.0,
            ..default()
        };
        for _ in 0..3 {
            let (_, Projection::Perspective(lens)) = view_camera(&preset, Some(&state)) else {
                panic!("perspective");
            };
            assert!((lens.fov.to_degrees() - 24.0).abs() < 0.0001);
        }
        state.effect_scope = crate::script::CameraEffectScope::Canvas;
        let (_, Projection::Perspective(lens)) = view_camera(&preset, Some(&state)) else {
            panic!("perspective");
        };
        assert!((lens.fov.to_degrees() - 48.0).abs() < 0.0001);
        state.effect_scope = crate::script::CameraEffectScope::World;
        preset.projection = super::super::StageProjection::Orthographic {
            height: 12.0,
            near: 0.1,
            far: 1000.0,
        };
        let (_, Projection::Orthographic(lens)) = view_camera(&preset, Some(&state)) else {
            panic!("orthographic");
        };
        assert_eq!(lens.scale, 0.5);
    }
}
