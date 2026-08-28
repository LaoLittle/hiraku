use crate::{
    HirakuCanvas, RuntimeLaunchConfig,
    effect::blur::BlurSettings,
    scene::{AnimationState, apply_character_ease, complete_missing_animation, tween_fraction},
    script::{CameraEffectScope, CameraProjectionMode},
};
use crate::{
    effect::{custom::CustomScreenEffectPlayer, transition::RuleTransitionPlayer},
    scene::{
        BackgroundLayer, ChoiceUi, DialogueRoot, FocusedActorPart, FrontendRoot, OverlayMarker,
        PauseMenuRoot, SpriteActor,
    },
    script::CharacterEase,
    ui::ScreenUiRoot,
};
use bevy::{
    camera::{RenderTarget, ScalingMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};

/// Static background artwork and background-only effects such as rain or fog.
pub const BACKGROUND_LAYER: usize = 0;
/// Normal scene actors and props.
pub const SCENE_LAYER: usize = 1;
/// Isolated actors/props that must remain separable for focus effects.
pub const FOCUS_LAYER: usize = 2;
/// Engine UI. This layer is intentionally unaffected by world post-processing.
pub const UI_LAYER: usize = 3;

pub fn background_layer() -> RenderLayers {
    RenderLayers::layer(BACKGROUND_LAYER)
}

pub fn scene_layer() -> RenderLayers {
    RenderLayers::layer(SCENE_LAYER)
}

pub fn focus_layer() -> RenderLayers {
    RenderLayers::layer(FOCUS_LAYER)
}

pub fn ui_layer() -> RenderLayers {
    RenderLayers::layer(UI_LAYER)
}

/// Cameras transformed together by story-level zoom, pan and shake.
#[derive(Component)]
pub struct WorldCamera;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World3dLayer {
    Background,
    Scene,
    Focus,
}

/// Marks a 3D mesh for one of Hiraku's semantic render phases.
#[derive(Component, Clone, Copy, Debug)]
pub struct World3dObject {
    pub layer: World3dLayer,
}

/// The single primary 3D camera owned by Hiraku.
#[derive(Component)]
pub struct WorldCamera3d {
    orthographic_height: f32,
}

#[derive(Resource, Default)]
pub struct CameraShakeState {
    pub active: Option<CameraShake>,
}

#[derive(Resource, Clone, PartialEq)]
pub struct CameraState {
    pub blur_intensity: f32,
    pub zoom: f32,
    pub offset: Vec3,
    pub rotation: Vec3,
    pub projection: CameraProjectionMode,
    pub effect_scope: CameraEffectScope,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            blur_intensity: 0.0,
            zoom: 1.0,
            offset: Vec3::ZERO,
            rotation: Vec3::ZERO,
            projection: CameraProjectionMode::Orthographic,
            effect_scope: CameraEffectScope::World,
        }
    }
}

#[derive(Resource, Default)]
pub struct CameraTweenState {
    pub active: Option<CameraTween>,
}

pub struct CameraTween {
    pub blur: Option<CameraScalarTween>,
    pub zoom: Option<CameraScalarTween>,
    pub offset: Option<CameraVectorTween>,
    pub rotation: Option<CameraVectorTween>,
    pub completions: Vec<CameraTweenCompletion>,
}

pub struct CameraTweenCompletion {
    pub blur: bool,
    pub zoom: bool,
    pub offset: bool,
    pub rotation: bool,
    pub animation_id: Option<String>,
}

pub struct CameraScalarTween {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub ease: CharacterEase,
}

pub struct CameraVectorTween {
    pub from: Vec3,
    pub to: Vec3,
    pub timer: Timer,
    pub ease: CharacterEase,
}

pub struct CameraShake {
    pub timer: Timer,
    pub amplitude: f32,
    pub animation_id: Option<String>,
}

pub fn setup_stage_cameras(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    config: &RuntimeLaunchConfig,
) {
    let canvas_size = config.canvas_size.max(UVec2::ONE);
    let canvas_image = images.add(Image::new_target_texture(
        canvas_size.x,
        canvas_size.y,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));
    commands.insert_resource(HirakuCanvas {
        image: canvas_image.clone(),
        size: canvas_size,
    });
    commands.insert_resource(crate::HirakuInputTarget(canvas_image.clone()));

    let mut projection = OrthographicProjection::default_3d();
    projection.scaling_mode = ScalingMode::FixedVertical {
        viewport_height: canvas_size.y as f32,
    };
    projection.near = -2000.0;
    projection.far = 2000.0;
    commands.spawn((
        Camera3d::default(),
        IsDefaultUiCamera,
        Projection::Orthographic(projection),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        BlurSettings::default(),
        Camera {
            order: config.camera_order,
            clear_color: config.camera_clear_color.clone(),
            ..default()
        },
        RenderTarget::Image(canvas_image.into()),
        RenderLayers::from_layers(&[BACKGROUND_LAYER, SCENE_LAYER, FOCUS_LAYER]),
        WorldCamera,
        WorldCamera3d {
            orthographic_height: canvas_size.y as f32,
        },
    ));
}

/// Assigns engine visual entities to their semantic render layer.
///
/// Centralizing this rule keeps command handlers and snapshot restoration from
/// having to remember camera implementation details on every spawn path.
pub fn assign_render_layers(
    mut commands: Commands,
    backgrounds: Query<
        Entity,
        Or<(
            Added<BackgroundLayer>,
            Added<RuleTransitionPlayer>,
            Added<CustomScreenEffectPlayer>,
        )>,
    >,
    actors: Query<(Entity, Option<&FocusedActorPart>), Added<SpriteActor>>,
    world_3d_objects: Query<(Entity, &World3dObject), Added<World3dObject>>,
    overlays: Query<Entity, Added<OverlayMarker>>,
    ui_roots: Query<
        Entity,
        Or<(
            Added<DialogueRoot>,
            Added<ChoiceUi>,
            Added<PauseMenuRoot>,
            Added<FrontendRoot>,
            Added<ScreenUiRoot>,
        )>,
    >,
) {
    for entity in &backgrounds {
        commands.entity(entity).try_insert(background_layer());
    }
    for (entity, focused) in &actors {
        commands.entity(entity).try_insert(if focused.is_some() {
            focus_layer()
        } else {
            scene_layer()
        });
    }
    for (entity, object) in &world_3d_objects {
        let layer = match object.layer {
            World3dLayer::Background => background_layer(),
            World3dLayer::Scene => scene_layer(),
            World3dLayer::Focus => focus_layer(),
        };
        commands.entity(entity).try_insert(layer);
    }
    for entity in &overlays {
        commands.entity(entity).try_insert(focus_layer());
    }
    for entity in &ui_roots {
        commands.entity(entity).try_insert(ui_layer());
    }
}

pub fn animate_camera_shake(
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    camera_state: Res<CameraState>,
    mut cameras: Query<&mut Transform, With<WorldCamera>>,
) {
    let Some(shake) = shake_state.active.as_mut() else {
        for mut camera in &mut cameras {
            camera.translation.x = camera_state.offset.x;
            camera.translation.y = camera_state.offset.y;
        }
        return;
    };

    shake.timer.tick(time.delta());
    let decay = 1.0 - tween_fraction(&shake.timer);
    let elapsed = shake.timer.elapsed_secs();
    let amplitude = shake.amplitude * decay;
    for mut camera in &mut cameras {
        camera.translation.x = camera_state.offset.x + (elapsed * 43.0).sin() * amplitude;
        camera.translation.y = camera_state.offset.y + (elapsed * 31.0).cos() * amplitude;
    }

    if shake.timer.is_finished() {
        for mut camera in &mut cameras {
            camera.translation.x = camera_state.offset.x;
            camera.translation.y = camera_state.offset.y;
        }
        if let Some(animation_id) = shake.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        shake_state.active = None;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_camera_tween(
    camera: &mut CameraState,
    tweens: &mut CameraTweenState,
    blur_intensity: Option<f32>,
    zoom: Option<f32>,
    offset: Option<Vec3>,
    rotation: Option<Vec3>,
    projection: Option<CameraProjectionMode>,
    scope: CameraEffectScope,
    duration: std::time::Duration,
    ease: CharacterEase,
    animation_id: Option<String>,
    animations: &mut AnimationState,
) {
    camera.effect_scope = scope;
    if let Some(projection) = projection {
        camera.projection = projection;
    }
    if duration.is_zero() {
        if let Some(tween) = tweens.active.as_mut() {
            cancel_camera_completions(
                tween,
                blur_intensity.is_some(),
                zoom.is_some(),
                offset.is_some(),
                rotation.is_some(),
                animations,
            );
            if blur_intensity.is_some() {
                tween.blur = None;
            }
            if zoom.is_some() {
                tween.zoom = None;
            }
            if offset.is_some() {
                tween.offset = None;
            }
            if rotation.is_some() {
                tween.rotation = None;
            }
        }
        if let Some(blur_intensity) = blur_intensity {
            camera.blur_intensity = blur_intensity;
        }
        if let Some(zoom) = zoom {
            camera.zoom = zoom;
        }
        if let Some(offset) = offset {
            camera.offset = offset;
        }
        if let Some(rotation) = rotation {
            camera.rotation = rotation;
        }
        complete_missing_animation(animations, animation_id);
        return;
    }

    let tween = tweens.active.get_or_insert_with(|| CameraTween {
        blur: None,
        zoom: None,
        offset: None,
        rotation: None,
        completions: Vec::new(),
    });
    cancel_camera_completions(
        tween,
        blur_intensity.is_some(),
        zoom.is_some(),
        offset.is_some(),
        rotation.is_some(),
        animations,
    );
    if let Some(to) = blur_intensity {
        tween.blur = Some(CameraScalarTween {
            from: camera.blur_intensity,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = zoom {
        tween.zoom = Some(CameraScalarTween {
            from: camera.zoom,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = offset {
        tween.offset = Some(CameraVectorTween {
            from: camera.offset,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = rotation {
        tween.rotation = Some(CameraVectorTween {
            from: camera.rotation,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    tween.completions.push(CameraTweenCompletion {
        blur: blur_intensity.is_some(),
        zoom: zoom.is_some(),
        offset: offset.is_some(),
        rotation: rotation.is_some(),
        animation_id,
    });
}

fn cancel_camera_completions(
    tween: &mut CameraTween,
    blur: bool,
    zoom: bool,
    offset: bool,
    rotation: bool,
    animations: &mut AnimationState,
) {
    let mut retained = Vec::new();
    for completion in tween.completions.drain(..) {
        if (blur && completion.blur)
            || (zoom && completion.zoom)
            || (offset && completion.offset)
            || (rotation && completion.rotation)
        {
            complete_missing_animation(animations, completion.animation_id);
        } else {
            retained.push(completion);
        }
    }
    tween.completions = retained;
}

pub fn animate_camera_transition(
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut camera_state: ResMut<CameraState>,
    mut tweens: ResMut<CameraTweenState>,
    mut applied_state: Local<Option<CameraState>>,
    mut world_cameras: Query<
        (
            &WorldCamera3d,
            &mut Projection,
            &mut Transform,
            &mut BlurSettings,
        ),
        With<WorldCamera>,
    >,
) {
    let mut completed = Vec::new();
    if let Some(tween) = tweens.active.as_mut() {
        if let Some(blur_tween) = tween.blur.as_mut() {
            blur_tween.timer.tick(time.delta());
            camera_state.blur_intensity = blur_tween.from.lerp(
                blur_tween.to,
                apply_character_ease(blur_tween.ease, tween_fraction(&blur_tween.timer)),
            );
        }
        if let Some(zoom_tween) = tween.zoom.as_mut() {
            zoom_tween.timer.tick(time.delta());
            camera_state.zoom = zoom_tween.from.lerp(
                zoom_tween.to,
                apply_character_ease(zoom_tween.ease, tween_fraction(&zoom_tween.timer)),
            );
        }
        if let Some(offset_tween) = tween.offset.as_mut() {
            offset_tween.timer.tick(time.delta());
            camera_state.offset = offset_tween.from.lerp(
                offset_tween.to,
                apply_character_ease(offset_tween.ease, tween_fraction(&offset_tween.timer)),
            );
        }
        if let Some(rotation_tween) = tween.rotation.as_mut() {
            rotation_tween.timer.tick(time.delta());
            camera_state.rotation = rotation_tween.from.lerp(
                rotation_tween.to,
                apply_character_ease(rotation_tween.ease, tween_fraction(&rotation_tween.timer)),
            );
        }

        let blur_finished = tween
            .blur
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let zoom_finished = tween
            .zoom
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let offset_finished = tween
            .offset
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let rotation_finished = tween
            .rotation
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let mut pending = Vec::new();
        for completion in tween.completions.drain(..) {
            if (!completion.blur || blur_finished)
                && (!completion.zoom || zoom_finished)
                && (!completion.offset || offset_finished)
                && (!completion.rotation || rotation_finished)
            {
                completed.push(completion);
            } else {
                pending.push(completion);
            }
        }
        tween.completions = pending;
    }
    for completion in completed {
        complete_missing_animation(&mut animations, completion.animation_id);
    }
    if tweens
        .active
        .as_ref()
        .is_some_and(|tween| tween.completions.is_empty())
    {
        tweens.active = None;
    }

    if applied_state.as_ref() == Some(&*camera_state) {
        return;
    }
    *applied_state = Some(camera_state.clone());

    for (camera, mut projection, mut transform, mut blur) in &mut world_cameras {
        blur.set_radius(camera_state.blur_intensity);
        blur.set_include_ui(matches!(
            camera_state.effect_scope,
            CameraEffectScope::Canvas
        ));
        // Bevy UI is a separate pass attached to this camera and does not use
        // its world projection. Camera transforms therefore always apply to the
        // 3D scene, including effects authored with canvas scope.
        let zoom = camera_state.zoom.max(0.01);
        match camera_state.projection {
            CameraProjectionMode::Orthographic => {
                if !matches!(*projection, Projection::Orthographic(_)) {
                    let mut orthographic = OrthographicProjection::default_3d();
                    orthographic.scaling_mode = ScalingMode::FixedVertical {
                        viewport_height: camera.orthographic_height,
                    };
                    orthographic.near = -2000.0;
                    orthographic.far = 2000.0;
                    *projection = Projection::Orthographic(orthographic);
                }
                if let Projection::Orthographic(orthographic) = projection.as_mut() {
                    orthographic.scale = 1.0 / zoom;
                }
            }
            CameraProjectionMode::Perspective => {
                if !matches!(*projection, Projection::Perspective(_)) {
                    *projection = Projection::Perspective(PerspectiveProjection::default());
                }
                if let Projection::Perspective(perspective) = projection.as_mut() {
                    perspective.fov = (60.0_f32.to_radians() / zoom)
                        .clamp(5.0_f32.to_radians(), 170.0_f32.to_radians());
                }
            }
        }
        transform.translation = Vec3::new(
            camera_state.offset.x,
            camera_state.offset.y,
            1000.0 + camera_state.offset.z,
        );
        let rotation = camera_state.rotation;
        transform.rotation = Quat::from_euler(
            EulerRot::XYZ,
            rotation.x.to_radians(),
            rotation.y.to_radians(),
            rotation.z.to_radians(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_layer_ids_follow_composition_order() {
        assert_eq!(
            [BACKGROUND_LAYER, SCENE_LAYER, FOCUS_LAYER, UI_LAYER],
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn semantic_entities_receive_their_render_layers() {
        let mut app = App::new();
        app.add_systems(Update, assign_render_layers);
        let background = app
            .world_mut()
            .spawn(BackgroundLayer {
                path: "background.webp".to_string(),
            })
            .id();
        let actor = app
            .world_mut()
            .spawn(SpriteActor {
                id: "alice".to_string(),
                path: "alice.webp".to_string(),
            })
            .id();
        let focused_actor = app
            .world_mut()
            .spawn((
                SpriteActor {
                    id: "bob".to_string(),
                    path: "bob.webp".to_string(),
                },
                FocusedActorPart,
            ))
            .id();
        let overlay = app.world_mut().spawn(OverlayMarker).id();

        app.update();

        let world = app.world();
        assert!(
            world
                .get::<RenderLayers>(background)
                .is_some_and(|layers| layers.intersects(&background_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(focused_actor)
                .is_some_and(|layers| layers.intersects(&focus_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(actor)
                .is_some_and(|layers| layers.intersects(&scene_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(overlay)
                .is_some_and(|layers| layers.intersects(&focus_layer()))
        );
    }

    #[test]
    fn render_layer_assignment_tolerates_same_frame_despawn() {
        fn despawn_added_actors(mut commands: Commands, actors: Query<Entity, Added<SpriteActor>>) {
            for entity in &actors {
                commands.entity(entity).try_despawn();
            }
        }

        let mut app = App::new();
        app.add_systems(
            Update,
            (despawn_added_actors, assign_render_layers).chain_ignore_deferred(),
        );
        let actor = app
            .world_mut()
            .spawn(SpriteActor {
                id: "alice".to_string(),
                path: "alice.webp".to_string(),
            })
            .id();

        app.update();

        assert!(app.world().get_entity(actor).is_err());
    }
}
