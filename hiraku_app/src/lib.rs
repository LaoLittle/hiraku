use bevy::{
    asset::{AssetMetaCheck, AssetPlugin},
    camera::{ScalingMode, visibility::RenderLayers},
    picking::pointer::PointerId,
    prelude::*,
    sprite::{SpritePickingCamera, SpritePickingMode, SpritePickingSettings},
    window::WindowPlugin,
};
use hiraku_engine::{
    HirakuCanvas, HirakuPluginGroup, RuntimeLaunchConfig, configure_runtime_app,
    input::{HirakuAction, HirakuActionInput, HirakuPointerInput, HirakuPointerPhase},
};

const PRESENTATION_LAYER: usize = 31;

#[derive(Component)]
struct CanvasPresentation;

#[derive(Component)]
struct PresentationCamera;

/// Presents Hiraku's fixed-resolution canvas in a window and forwards Bevy physical pointer
/// picking into the engine's virtual pointer boundary.
pub struct HirakuPresentationPlugin;

impl Plugin for HirakuPresentationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SpritePickingSettings {
            require_markers: true,
            picking_mode: SpritePickingMode::BoundingBox,
        })
        .add_systems(Update, (present_hiraku_canvas, bridge_host_actions));
    }
}

pub fn run_app(config: RuntimeLaunchConfig) {
    build_app(config).run();
}

pub fn build_app(config: RuntimeLaunchConfig) -> App {
    let asset_root = config.asset_root.clone();
    let window_title = config.window_title.clone();
    let mut app = App::new();

    configure_runtime_app(&mut app, config);
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: asset_root,
                meta_check: AssetMetaCheck::Never,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: window_title,
                    present_mode: bevy::window::PresentMode::AutoVsync,
                    ..default()
                }),
                ..default()
            }),
    );
    app.add_plugins(HirakuPluginGroup);
    app.add_plugins(HirakuPresentationPlugin);

    app
}

fn bridge_host_actions(
    keys: Res<ButtonInput<KeyCode>>,
    mut actions: MessageWriter<HirakuActionInput>,
) {
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::Space]) {
        actions.write(HirakuActionInput(HirakuAction::NextDialogue));
    }
    for (key, index) in [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
        (KeyCode::Digit5, 4),
        (KeyCode::Digit6, 5),
        (KeyCode::Digit7, 6),
        (KeyCode::Digit8, 7),
        (KeyCode::Digit9, 8),
        (KeyCode::Numpad1, 0),
        (KeyCode::Numpad2, 1),
        (KeyCode::Numpad3, 2),
        (KeyCode::Numpad4, 3),
        (KeyCode::Numpad5, 4),
        (KeyCode::Numpad6, 5),
        (KeyCode::Numpad7, 6),
        (KeyCode::Numpad8, 7),
        (KeyCode::Numpad9, 8),
    ] {
        if keys.just_pressed(key) {
            actions.write(HirakuActionInput(HirakuAction::Choice(index)));
        }
    }
}

fn host_pointer_id(id: PointerId) -> Option<u64> {
    match id {
        PointerId::Mouse => Some(0),
        PointerId::Touch(id) => Some(id + 1),
        PointerId::Custom(_) => None,
    }
}

fn canvas_uv(
    hit: &bevy::picking::backend::HitData,
    transform: &GlobalTransform,
    size: Vec2,
) -> Option<Vec2> {
    let local = transform.affine().inverse().transform_point3(hit.position?);
    let uv = Vec2::new(local.x / size.x + 0.5, 0.5 - local.y / size.y);
    (uv.cmpge(Vec2::ZERO).all() && uv.cmple(Vec2::ONE).all()).then_some(uv)
}

fn forward_pointer(
    pointer_id: PointerId,
    target: Entity,
    hit: &bevy::picking::backend::HitData,
    phase: HirakuPointerPhase,
    targets: &Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: &Option<Res<HirakuCanvas>>,
    output: &mut MessageWriter<HirakuPointerInput>,
) -> bool {
    let (Some(pointer), Some(canvas), Ok(transform)) = (
        host_pointer_id(pointer_id),
        canvas.as_ref(),
        targets.get(target),
    ) else {
        return false;
    };
    let Some(uv) = canvas_uv(hit, transform, canvas.size.as_vec2()) else {
        return false;
    };
    output.write(HirakuPointerInput { pointer, uv, phase });
    true
}

fn forward_canvas_move(
    mut event: On<Pointer<Move>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuPointerInput>,
) {
    if forward_pointer(
        event.pointer_id,
        event.event_target(),
        &event.hit,
        HirakuPointerPhase::Move,
        &targets,
        &canvas,
        &mut output,
    ) {
        event.propagate(false);
    }
}

fn forward_canvas_press(
    mut event: On<Pointer<Press>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuPointerInput>,
) {
    if event.button == PointerButton::Primary
        && forward_pointer(
            event.pointer_id,
            event.event_target(),
            &event.hit,
            HirakuPointerPhase::Press,
            &targets,
            &canvas,
            &mut output,
        )
    {
        event.propagate(false);
    }
}

fn forward_canvas_release(
    mut event: On<Pointer<Release>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuPointerInput>,
) {
    if event.button == PointerButton::Primary
        && forward_pointer(
            event.pointer_id,
            event.event_target(),
            &event.hit,
            HirakuPointerPhase::Release,
            &targets,
            &canvas,
            &mut output,
        )
    {
        event.propagate(false);
    }
}

fn present_hiraku_canvas(
    mut commands: Commands,
    canvas: Option<Res<HirakuCanvas>>,
    presentation: Query<(), With<CanvasPresentation>>,
) {
    if !presentation.is_empty() {
        return;
    }
    let Some(canvas) = canvas else { return };
    let layer = RenderLayers::layer(PRESENTATION_LAYER);
    let surface = commands
        .spawn((
            CanvasPresentation,
            Sprite::from_image(canvas.image.clone()),
            Pickable::default(),
            layer.clone(),
        ))
        .id();
    commands
        .entity(surface)
        .observe(forward_canvas_move)
        .observe(forward_canvas_press)
        .observe(forward_canvas_release);
    commands.spawn((
        PresentationCamera,
        SpritePickingCamera,
        Camera2d,
        Camera {
            order: 0,
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::AutoMin {
                min_width: canvas.size.x as f32,
                min_height: canvas.size.y as f32,
            },
            ..OrthographicProjection::default_2d()
        }),
        layer,
    ));
}
