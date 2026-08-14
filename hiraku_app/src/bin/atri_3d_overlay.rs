use bevy::{
    camera::{ClearColorConfig, RenderTarget},
    prelude::*,
    window::WindowRef,
};

#[derive(Component)]
struct TestObject;

fn main() {
    let mut app = hiraku_app::build_app(hiraku_engine::RuntimeLaunchConfig {
        asset_root: "examples/atri_3d_overlay".to_string(),
        settings_path: "settings.rhai".to_string(),
        default_startup_script: "startup.rhai".to_string(),
        window_title: "hiraku: Atri 3D overlay".to_string(),
        render_target: RenderTarget::Window(WindowRef::Primary),
        camera_order: 100,
        camera_clear_color: ClearColorConfig::None,
    });
    app.add_systems(Startup, setup_3d_scene);
    app.add_systems(Update, rotate_test_objects);
    app.run();
}

fn setup_3d_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 0,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.025, 0.06, 0.11)),
            ..default()
        },
        Transform::from_xyz(7.5, 5.5, 9.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.035, 0.055, 0.075))),
    ));

    for (position, color, size) in [
        (
            Vec3::new(-2.5, 1.0, 0.0),
            Color::srgb(0.18, 0.43, 0.62),
            2.0,
        ),
        (
            Vec3::new(0.0, 0.75, -1.5),
            Color::srgb(0.72, 0.30, 0.22),
            1.5,
        ),
        (
            Vec3::new(2.5, 1.25, 0.5),
            Color::srgb(0.78, 0.62, 0.24),
            2.5,
        ),
    ] {
        commands.spawn((
            TestObject,
            Mesh3d(meshes.add(Cuboid::from_size(Vec3::splat(size)))),
            MeshMaterial3d(materials.add(color)),
            Transform::from_translation(position),
        ));
    }

    commands.spawn((
        PointLight {
            intensity: 1800.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 7.0, 5.0),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 7000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, -0.6, 0.0)),
    ));
}

fn rotate_test_objects(mut objects: Query<&mut Transform, With<TestObject>>, time: Res<Time>) {
    for (index, mut transform) in objects.iter_mut().enumerate() {
        let direction = if index % 2 == 0 { 1.0 } else { -1.0 };
        transform.rotate_y(direction * time.delta_secs() * 0.45);
        transform.rotate_x(direction * time.delta_secs() * 0.18);
    }
}
