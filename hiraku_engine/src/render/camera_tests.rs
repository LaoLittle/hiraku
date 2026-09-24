use super::*;

#[test]
fn background_camera_moves_both_crossfade_images_without_changing_depth() {
    let mut states = CameraState::default();
    let view = states.view_mut(CameraEffectScope::Background);
    view.offset = Vec3::new(10.0, 20.0, 0.0);
    view.zoom = 2.0;
    view.rotation.z = 90.0;
    for depth in [1.0, 2.0] {
        let local = Transform::from_xyz(30.0, 50.0, depth);
        let transformed = view.transform_picture(local);
        assert!((transformed.translation - Vec3::new(60.0, -40.0, depth)).length() < 0.0001);
        assert_eq!(transformed.scale, Vec3::new(2.0, 2.0, 1.0));
    }
    assert_eq!(states.view(CameraEffectScope::World).zoom, 1.0);
}

#[test]
fn virtual_camera_picking_inverts_canvas_then_ui_with_canvas_aspect() {
    let mut states = CameraState::default();
    let size = Vec2::new(1920.0, 1080.0);
    states.view_mut(CameraEffectScope::Canvas).zoom = 2.0;
    states.view_mut(CameraEffectScope::Ui).offset = Vec3::new(192.0, 108.0, 0.0);
    let source = states.ui_source_uv(Vec2::new(0.75, 0.5), size);
    assert!((source - Vec2::new(0.725, 0.4)).length() < 0.00001);
    states.view_mut(CameraEffectScope::Ui).rotation.z = 90.0;
    let source = states.ui_source_uv(Vec2::new(0.75, 0.5), size);
    assert!((source - Vec2::new(0.6, 0.4 - 240.0 / 1080.0)).length() < 0.00001);
    assert_eq!(states.view(CameraEffectScope::World).offset, Vec3::ZERO);
}

#[test]
fn virtual_camera_blank_canvas_border_does_not_pick_ui() {
    let mut states = CameraState::default();
    states.view_mut(CameraEffectScope::Canvas).zoom = 0.5;
    states.view_mut(CameraEffectScope::Ui).zoom = 10.0;
    assert_eq!(
        states.ui_source_uv(Vec2::ZERO, Vec2::new(1920.0, 1080.0)),
        Vec2::splat(-1.0)
    );
}

fn zoom_view(
    states: &mut CameraState,
    timelines: &mut CameraTweenState,
    animations: &mut AnimationState,
    scope: CameraEffectScope,
    zoom: f32,
    seconds: f32,
    id: &str,
) {
    start_camera_tween(
        states,
        timelines,
        None,
        Some(zoom),
        false,
        None,
        None,
        None,
        scope,
        std::time::Duration::from_secs_f32(seconds),
        CharacterEase::Linear,
        Some(id.into()),
        animations,
    );
}

#[test]
fn virtual_camera_timelines_do_not_cancel_or_overwrite_other_views() {
    let mut states = CameraState::default();
    let mut timelines = CameraTweenState::default();
    let mut animations = AnimationState::default();
    zoom_view(
        &mut states,
        &mut timelines,
        &mut animations,
        CameraEffectScope::World,
        3.0,
        2.0,
        "scene",
    );
    zoom_view(
        &mut states,
        &mut timelines,
        &mut animations,
        CameraEffectScope::Ui,
        2.0,
        1.0,
        "ui",
    );
    zoom_view(
        &mut states,
        &mut timelines,
        &mut animations,
        CameraEffectScope::Canvas,
        0.5,
        0.0,
        "canvas",
    );
    for scope in [CameraEffectScope::World, CameraEffectScope::Ui] {
        tick_camera_view(
            states.view_mut(scope),
            timelines.views.get_mut(&scope).expect("timeline"),
            std::time::Duration::from_secs(1),
            &mut animations,
        );
    }
    assert_eq!(states.view(CameraEffectScope::World).zoom, 2.0);
    assert_eq!(states.view(CameraEffectScope::Ui).zoom, 2.0);
    assert_eq!(states.view(CameraEffectScope::Canvas).zoom, 0.5);
    assert!(animations.completed.contains("ui"));
    assert!(animations.completed.contains("canvas"));
    assert!(!animations.completed.contains("scene"));
    zoom_view(
        &mut states,
        &mut timelines,
        &mut animations,
        CameraEffectScope::Ui,
        1.0,
        0.0,
        "ui-reset",
    );
    assert!(!animations.completed.contains("scene"));
}

#[test]
fn virtual_camera_snapshot_resumes_at_elapsed_time_and_preserves_wait_id() {
    let mut states = CameraState::default();
    let mut timelines = CameraTweenState::default();
    let mut animations = AnimationState::default();
    zoom_view(
        &mut states,
        &mut timelines,
        &mut animations,
        CameraEffectScope::World,
        3.0,
        2.0,
        "scene",
    );
    tick_camera_view(
        states.view_mut(CameraEffectScope::World),
        timelines
            .views
            .get_mut(&CameraEffectScope::World)
            .expect("timeline"),
        std::time::Duration::from_millis(500),
        &mut animations,
    );
    let snapshot = crate::state::CameraSnapshot {
        views: states,
        timelines,
    };
    let encoded = crate::proto::CameraSnapshot::from(&snapshot);
    let mut restored = crate::state::CameraSnapshot::try_from(encoded).expect("camera roundtrip");
    assert_eq!(restored.views.view(CameraEffectScope::World).zoom, 1.5);
    tick_camera_view(
        restored.views.view_mut(CameraEffectScope::World),
        restored
            .timelines
            .views
            .get_mut(&CameraEffectScope::World)
            .expect("restored timeline"),
        std::time::Duration::from_millis(1500),
        &mut animations,
    );
    assert_eq!(restored.views.view(CameraEffectScope::World).zoom, 3.0);
    assert!(animations.completed.contains("scene"));
}
