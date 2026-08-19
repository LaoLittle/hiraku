use bevy::{
    camera::{ClearColorConfig, RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};
use std::sync::mpsc;

use crate::{
    HirakuCanvas, RuntimeLaunchConfig,
    effect::blur::BlurSettings,
    script::CameraEffectScope,
};
use crate::{
    effect::{custom::CustomScreenEffectPlayer, transition::RuleTransitionPlayer},
    scene::{
        BackgroundLayer, ChoiceUi, DialogueRoot, FrontendRoot, OverlayMarker, PauseMenuRoot,
        SpriteActor,
    },
    script::{CharacterEase, ScriptResponse},
    ui::ScreenUiRoot,
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
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    pub done: Option<mpsc::Sender<ScriptResponse>>,
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
    for (index, image) in [world_image, focus_image, ui_image]
        .into_iter()
        .enumerate()
    {
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
        commands.entity(entity).insert(background_layer());
    }
    for entity in &actors {
        commands.entity(entity).insert(scene_layer());
    }
    for entity in &overlays {
        commands.entity(entity).insert(focus_layer());
    }
    for entity in &ui_roots {
        commands.entity(entity).insert(ui_layer());
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
}
