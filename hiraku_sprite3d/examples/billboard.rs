//! A centered bird spinning in 3D like a coin, with a black reverse side.
//! `cargo run -p hiraku-sprite3d --example billboard` in the workspace.
use bevy::prelude::*;
use hiraku_sprite3d::{Sprite3d, Sprite3dPlugin};

#[derive(Component)]
struct SpinningBird;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Sprite3d — coin rotation".into(),
                        ..default()
                    }),
                    ..default()
                }),
            Sprite3dPlugin,
        ))
        .insert_resource(ClearColor(Color::srgb(0.12, 0.14, 0.18)))
        .add_systems(Startup, setup)
        .add_systems(Update, rotate)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 0.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Sprite3d {
            custom_size: Some(Vec2::splat(2.0)),
            backface_color: Color::BLACK,
            ..Sprite3d::from_image(assets.load("bevy_bird.png"))
        },
        SpinningBird,
        Transform::default(),
    ));
}

fn rotate(time: Res<Time>, mut birds: Query<&mut Transform, With<SpinningBird>>) {
    for mut transform in &mut birds {
        // No Billboard component: camera-facing rotation would cancel this spin.
        transform.rotate_y(time.delta_secs() * 60.0_f32.to_radians());
    }
}
