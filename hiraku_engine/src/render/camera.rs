use crate::{
    HirakuCanvas, RuntimeLaunchConfig,
    effect::blur::BlurSettings,
    scene::{AnimationState, apply_character_ease, complete_missing_animation, tween_fraction},
    script::CameraEffectScope,
};
use crate::{
    effect::{custom::CustomScreenEffectPlayer, transition::RuleTransitionPlayer},
    scene::{
        BackgroundLayer, ChoiceUi, DialogueRoot, FrontendRoot, OverlayMarker, PauseMenuRoot,
        SpriteActor,
    },
    script::CharacterEase,
    ui::ScreenUiRoot,
};
use bevy::{
    camera::{ClearColorConfig, RenderTarget, visibility::RenderLayers},
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
const WORLD_COMPOSITOR_LAYER: usize = 29;
const FINAL_COMPOSITOR_LAYER: usize = 30;

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

#[derive(Component)]
pub struct BackgroundCamera;

#[derive(Component)]
pub struct SceneCamera;

#[derive(Component)]
pub struct FocusCamera;

#[derive(Component)]
pub struct UiCamera;

/// Cameras transformed together by story-level zoom, pan and shake.
#[derive(Component)]
pub struct WorldCamera;

/// The internal camera receiving post-processing after background and scene
/// have been composed. Focus and UI are added later and therefore stay sharp.
#[derive(Component)]
pub struct WorldEffectCamera;

/// The final four-layer compositor. Canvas-scoped effects are applied here.
#[derive(Component)]
pub struct CanvasEffectCamera;

#[derive(Resource, Default)]
pub struct CameraShakeState {
    pub active: Option<CameraShake>,
}

#[derive(Resource)]
pub struct CameraState {
    pub blur_intensity: f32,
    pub zoom: f32,
    pub center: Vec2,
    pub effect_scope: CameraEffectScope,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            blur_intensity: 0.0,
            zoom: 1.0,
            center: Vec2::ZERO,
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
    pub center: Option<CameraPositionTween>,
    pub completions: Vec<CameraTweenCompletion>,
}

pub struct CameraTweenCompletion {
    pub blur: bool,
    pub zoom: bool,
    pub center: bool,
    pub animation_id: Option<String>,
}

pub struct CameraScalarTween {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub ease: CharacterEase,
}

pub struct CameraPositionTween {
    pub from: Vec2,
    pub to: Vec2,
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
    let mut target_image = || {
        images.add(Image::new_target_texture(
            canvas_size.x,
            canvas_size.y,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        ))
    };
    let background_image = target_image();
    let scene_image = target_image();
    let focus_image = target_image();
    let ui_image = target_image();
    let world_image = target_image();
    let canvas_image = target_image();
    commands.insert_resource(HirakuCanvas {
        image: canvas_image.clone(),
        size: canvas_size,
    });
    commands.insert_resource(crate::HirakuInputTarget(ui_image.clone()));

    spawn_world_camera(
        commands,
        background_image.clone(),
        config.camera_order,
        config.camera_clear_color.clone(),
        background_layer(),
        BackgroundCamera,
    );
    spawn_world_camera(
        commands,
        scene_image.clone(),
        config.camera_order + 1,
        transparent_clear(),
        scene_layer(),
        SceneCamera,
    );
    spawn_world_camera(
        commands,
        focus_image.clone(),
        config.camera_order + 2,
        transparent_clear(),
        focus_layer(),
        FocusCamera,
    );
    commands.spawn((
        Camera2d,
        IsDefaultUiCamera,
        Projection::Orthographic(OrthographicProjection::default_2d()),
        Camera {
            order: config.camera_order + 3,
            clear_color: transparent_clear(),
            ..default()
        },
        RenderTarget::Image(ui_image.clone().into()),
        ui_layer(),
        UiCamera,
    ));

    // Background and scene must be combined before world-level effects. Applying
    // blur to both source cameras independently changes the result at layer
    // boundaries and breaks effects which sample the already-composed world.
    let world_compositor_layer = RenderLayers::layer(WORLD_COMPOSITOR_LAYER);
    for (index, image) in [background_image, scene_image].into_iter().enumerate() {
        commands.spawn((
            Sprite::from_image(image),
            Transform::from_xyz(0.0, 0.0, index as f32),
            world_compositor_layer.clone(),
        ));
    }
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection::default_2d()),
        BlurSettings::default(),
        Camera {
            order: config.camera_order + 4,
            clear_color: config.camera_clear_color.clone(),
            ..default()
        },
        RenderTarget::Image(world_image.clone().into()),
        world_compositor_layer,
        WorldEffectCamera,
    ));

    // Focus and UI are intentionally composed after world post-processing.
    let final_compositor_layer = RenderLayers::layer(FINAL_COMPOSITOR_LAYER);
    for (index, image) in [world_image, focus_image, ui_image].into_iter().enumerate() {
        commands.spawn((
            Sprite::from_image(image),
            Transform::from_xyz(0.0, 0.0, index as f32),
            final_compositor_layer.clone(),
        ));
    }
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection::default_2d()),
        BlurSettings::default(),
        Camera {
            order: config.camera_order + 5,
            clear_color: config.camera_clear_color.clone(),
            ..default()
        },
        RenderTarget::Image(canvas_image.into()),
        final_compositor_layer,
        CanvasEffectCamera,
    ));
}

fn transparent_clear() -> ClearColorConfig {
    ClearColorConfig::Custom(Color::srgba(0.0, 0.0, 0.0, 0.0))
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
    actors: Query<Entity, Added<SpriteActor>>,
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
    for entity in &actors {
        commands.entity(entity).try_insert(scene_layer());
    }
    for entity in &overlays {
        commands.entity(entity).try_insert(focus_layer());
    }
    for entity in &ui_roots {
        commands.entity(entity).try_insert(ui_layer());
    }
}

fn spawn_world_camera<M: Bundle>(
    commands: &mut Commands,
    canvas: Handle<Image>,
    order: isize,
    clear_color: ClearColorConfig,
    layers: RenderLayers,
    marker: M,
) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection::default_2d()),
        Camera {
            order,
            clear_color,
            ..default()
        },
        RenderTarget::Image(canvas.into()),
        layers,
        WorldCamera,
        marker,
    ));
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
            camera.translation.x = camera_state.center.x;
            camera.translation.y = camera_state.center.y;
        }
        return;
    };

    shake.timer.tick(time.delta());
    let decay = 1.0 - tween_fraction(&shake.timer);
    let elapsed = shake.timer.elapsed_secs();
    let amplitude = shake.amplitude * decay;
    for mut camera in &mut cameras {
        camera.translation.x = camera_state.center.x + (elapsed * 43.0).sin() * amplitude;
        camera.translation.y = camera_state.center.y + (elapsed * 31.0).cos() * amplitude;
    }

    if shake.timer.is_finished() {
        for mut camera in &mut cameras {
            camera.translation.x = camera_state.center.x;
            camera.translation.y = camera_state.center.y;
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
    scope: CameraEffectScope,
    center: Option<Vec2>,
    duration: std::time::Duration,
    ease: CharacterEase,
    animation_id: Option<String>,
    animations: &mut AnimationState,
) {
    camera.effect_scope = scope;
    if duration.is_zero() {
        if let Some(tween) = tweens.active.as_mut() {
            cancel_camera_completions(
                tween,
                blur_intensity.is_some(),
                zoom.is_some(),
                center.is_some(),
                animations,
            );
            if blur_intensity.is_some() {
                tween.blur = None;
            }
            if zoom.is_some() {
                tween.zoom = None;
            }
            if center.is_some() {
                tween.center = None;
            }
        }
        if let Some(blur_intensity) = blur_intensity {
            camera.blur_intensity = blur_intensity;
        }
        if let Some(zoom) = zoom {
            camera.zoom = zoom;
        }
        if let Some(center) = center {
            camera.center = center;
        }
        complete_missing_animation(animations, animation_id);
        return;
    }

    let tween = tweens.active.get_or_insert_with(|| CameraTween {
        blur: None,
        zoom: None,
        center: None,
        completions: Vec::new(),
    });
    cancel_camera_completions(
        tween,
        blur_intensity.is_some(),
        zoom.is_some(),
        center.is_some(),
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
    if let Some(to) = center {
        tween.center = Some(CameraPositionTween {
            from: camera.center,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    tween.completions.push(CameraTweenCompletion {
        blur: blur_intensity.is_some(),
        zoom: zoom.is_some(),
        center: center.is_some(),
        animation_id,
    });
}

fn cancel_camera_completions(
    tween: &mut CameraTween,
    blur: bool,
    zoom: bool,
    center: bool,
    animations: &mut AnimationState,
) {
    let mut retained = Vec::new();
    for completion in tween.completions.drain(..) {
        if (blur && completion.blur) || (zoom && completion.zoom) || (center && completion.center) {
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
    mut world_cameras: Query<(&mut Projection, &mut Transform), With<WorldCamera>>,
    mut effect_cameras: Query<
        &mut BlurSettings,
        (With<WorldEffectCamera>, Without<CanvasEffectCamera>),
    >,
    mut canvas_camera: Query<
        (&mut Projection, &mut Transform, &mut BlurSettings),
        (
            With<CanvasEffectCamera>,
            Without<WorldCamera>,
            Without<WorldEffectCamera>,
        ),
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
        if let Some(center_tween) = tween.center.as_mut() {
            center_tween.timer.tick(time.delta());
            camera_state.center = center_tween.from.lerp(
                center_tween.to,
                apply_character_ease(center_tween.ease, tween_fraction(&center_tween.timer)),
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
        let center_finished = tween
            .center
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let mut pending = Vec::new();
        for completion in tween.completions.drain(..) {
            if (!completion.blur || blur_finished)
                && (!completion.zoom || zoom_finished)
                && (!completion.center || center_finished)
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

    let world_active = matches!(camera_state.effect_scope, CameraEffectScope::World);
    for mut blur in &mut effect_cameras {
        blur.set_radius(if world_active {
            camera_state.blur_intensity
        } else {
            0.0
        });
    }
    for (mut projection, mut transform) in &mut world_cameras {
        if let Projection::Orthographic(projection) = projection.as_mut() {
            projection.scale = if world_active {
                1.0 / camera_state.zoom.max(0.01)
            } else {
                1.0
            };
        }
        transform.translation.x = if world_active {
            camera_state.center.x
        } else {
            0.0
        };
        transform.translation.y = if world_active {
            camera_state.center.y
        } else {
            0.0
        };
    }
    if let Ok((mut projection, mut transform, mut blur)) = canvas_camera.single_mut() {
        blur.set_radius(if world_active {
            0.0
        } else {
            camera_state.blur_intensity
        });
        if let Projection::Orthographic(projection) = projection.as_mut() {
            projection.scale = if world_active {
                1.0
            } else {
                1.0 / camera_state.zoom.max(0.01)
            };
        }
        transform.translation.x = if world_active {
            0.0
        } else {
            camera_state.center.x
        };
        transform.translation.y = if world_active {
            0.0
        } else {
            camera_state.center.y
        };
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
