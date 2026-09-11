use bevy::{
    asset::{AssetMetaCheck, AssetPlugin}, camera::{ScalingMode, visibility::RenderLayers}, picking::pointer::PointerId, prelude::*, sprite::{SpritePickingCamera, SpritePickingMode, SpritePickingSettings}, window::WindowPlugin, winit::WinitSettings,
};
use bevy::{input::mouse::MouseScrollUnit, picking::events::Scroll};
use hiraku_engine::input::{HirakuPointerId, HirakuScrollInput, HirakuScrollUnit};
use hiraku_engine::{
    HirakuCanvas, HirakuPluginGroup, RuntimeLaunchConfig, configure_runtime_app,
    input::{HirakuAction, HirakuActionInput, HirakuPointerInput, HirakuPointerPhase},
};

const PRESENTATION_LAYER: usize = 31;

mod display;
mod platform;
pub mod text_input;

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
        .add_plugins(text_input::HirakuTextInputPlugin)
        .add_systems(Update, (present_hiraku_canvas, bridge_host_actions))
        .add_systems(
            Update,
            display::apply_preferences.run_if(resource_exists::<hiraku_engine::UserSettings>),
        );
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
    )
    .insert_resource(WinitSettings::desktop_app());
    app.add_plugins(HirakuPluginGroup);
    app.add_plugins(HirakuPresentationPlugin);

    app
}

fn bridge_host_actions(
    keys: Res<ButtonInput<KeyCode>>,
    mut actions: MessageWriter<HirakuActionInput>,
    focus: Res<hiraku_engine::input::HirakuTextFocus>,
    windows: Query<&Window>,
    mut was_fast_forward_held: Local<bool>,
) {
    let held = focus.0.is_none() && windows.iter().any(|window| window.focused)
        && keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if held != *was_fast_forward_held {
        actions.write(HirakuActionInput(HirakuAction::FastForwardHeld(held)));
        *was_fast_forward_held = held;
    }
    if focus.0.is_some() {
        return;
    }
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

fn host_pointer_id(id: PointerId) -> Option<HirakuPointerId> {
    match id {
        PointerId::Mouse => Some(HirakuPointerId::Pointer(0)),
        PointerId::Touch(id) => Some(HirakuPointerId::Touch(id)),
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

fn forward_canvas_scroll(
    mut event: On<Pointer<Scroll>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuScrollInput>,
) {
    let (Some(pointer), Some(canvas), Ok(transform)) = (
        host_pointer_id(event.pointer_id),
        canvas.as_ref(),
        targets.get(event.event_target()),
    ) else {
        return;
    };
    let Some(uv) = canvas_uv(&event.hit, transform, canvas.size.as_vec2()) else {
        return;
    };
    output.write(HirakuScrollInput {
        pointer,
        uv,
        delta: Vec2::new(event.x, event.y),
        unit: match event.unit {
            MouseScrollUnit::Line => HirakuScrollUnit::Line,
            MouseScrollUnit::Pixel => HirakuScrollUnit::Pixel,
        },
    });
    event.propagate(false);
}

fn forward_canvas_cancel(
    mut event: On<Pointer<Cancel>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuPointerInput>,
) {
    if forward_pointer(
        event.pointer_id,
        event.event_target(),
        &event.hit,
        HirakuPointerPhase::Cancel,
        &targets,
        &canvas,
        &mut output,
    ) {
        event.propagate(false);
    }
}

fn forward_canvas_out(
    mut event: On<Pointer<Out>>,
    targets: Query<&GlobalTransform, With<CanvasPresentation>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut output: MessageWriter<HirakuPointerInput>,
) {
    if matches!(event.pointer_id, PointerId::Touch(_))
        && forward_pointer(
            event.pointer_id,
            event.event_target(),
            &event.hit,
            HirakuPointerPhase::Cancel,
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
        .observe(forward_canvas_release)
        .observe(forward_canvas_cancel)
        .observe(forward_canvas_out)
        .observe(forward_canvas_scroll);
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        camera::NormalizedRenderTarget,
        picking::{backend::HitData, pointer::Location},
    };

    #[test]
    fn canvas_touch_move_and_cancel_preserve_identity_and_uv() {
        let mut app = App::new();
        app.insert_resource(HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(800, 600),
        })
        .add_message::<HirakuPointerInput>();
        let surface = app
            .world_mut()
            .spawn((CanvasPresentation, GlobalTransform::IDENTITY))
            .observe(forward_canvas_move)
            .observe(forward_canvas_cancel)
            .id();
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 800,
                height: 600,
            },
            position: Vec2::ZERO,
        };
        let hit = HitData {
            camera: surface,
            depth: 0.0,
            position: Some(Vec3::new(200.0, 150.0, 0.0)),
            normal: None,
            extra: None,
        };
        for id in [0, u64::MAX] {
            app.world_mut().trigger(Pointer::new(
                PointerId::Touch(id),
                location.clone(),
                Move {
                    hit: hit.clone(),
                    delta: Vec2::new(1.0, 2.0),
                },
                surface,
            ));
            app.world_mut().trigger(Pointer::new(
                PointerId::Touch(id),
                location.clone(),
                Cancel { hit: hit.clone() },
                surface,
            ));
        }
        let events = app
            .world_mut()
            .resource_mut::<Messages<HirakuPointerInput>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 4);
        for (pair, id) in events.chunks_exact(2).zip([0, u64::MAX]) {
            assert_eq!(pair[0].pointer, HirakuPointerId::Touch(id));
            assert_eq!(pair[1].pointer, HirakuPointerId::Touch(id));
            assert_eq!(pair[0].phase, HirakuPointerPhase::Move);
            assert_eq!(pair[1].phase, HirakuPointerPhase::Cancel);
            assert_eq!(pair[0].uv, Vec2::new(0.75, 0.25));
        }
    }

    #[test]
    fn canvas_scroll_observer_forwards_physical_input_but_not_virtual_input() {
        let mut app = App::new();
        app.insert_resource(HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(800, 600),
        })
        .add_message::<HirakuScrollInput>();
        let surface = app
            .world_mut()
            .spawn((CanvasPresentation, GlobalTransform::IDENTITY))
            .observe(forward_canvas_scroll)
            .id();
        for pointer in [PointerId::Mouse, PointerId::Touch(3)] {
            app.world_mut().trigger(Pointer::new(
                pointer,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 800,
                        height: 600,
                    },
                    position: Vec2::ZERO,
                },
                Scroll {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y: -2.0,
                    hit: HitData {
                        camera: surface,
                        depth: 0.0,
                        position: Some(Vec3::ZERO),
                        normal: None,
                        extra: None,
                    },
                    phase: bevy::input::touch::TouchPhase::Moved,
                },
                surface,
            ));
        }
        let forwarded = app
            .world_mut()
            .resource_mut::<Messages<HirakuScrollInput>>()
            .drain()
            .collect::<Vec<_>>();
        assert_eq!(forwarded.len(), 2);
        assert_eq!(forwarded[0].pointer, HirakuPointerId::Pointer(0));
        assert_eq!(forwarded[1].pointer, HirakuPointerId::Touch(3));
        for event in forwarded {
            assert_eq!(event.uv, Vec2::splat(0.5));
            assert_eq!(event.delta, Vec2::new(0.0, -2.0));
            assert_eq!(event.unit, HirakuScrollUnit::Line);
        }
        // Custom pointers belong to the embedded canvas and must not be
        // reflected back into the host-to-engine bridge.
        assert_eq!(host_pointer_id(PointerId::Custom(Default::default())), None);
    }
}
