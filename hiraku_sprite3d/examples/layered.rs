//! Procedural atlas, group opacity, three billboard modes; no external assets.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use hiraku_sprite3d::{
    Billboard, BillboardMode, BlendMode, MaskMode, Sprite3d, Sprite3dPlugin, SpriteLayer,
};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, Sprite3dPlugin))
        .insert_resource(ClearColor(Color::srgb(0.2, 0.25, 0.3)))
        .add_systems(Startup, setup)
        .add_systems(Update, animate)
        .run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 2.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    // Two atlas cells: opaque color and a soft circular translucent layer.
    let mut pixels = Vec::with_capacity(64 * 32 * 4);
    for y in 0..32 {
        for x in 0..64 {
            if x < 32 {
                pixels.extend_from_slice(&[240, 160, 60, 255]);
            } else {
                let distance = Vec2::new((x - 32) as f32 - 15.5, y as f32 - 15.5).length();
                let alpha = ((1.0 - distance / 16.0).clamp(0.0, 1.0) * 180.0) as u8;
                pixels.extend_from_slice(&[80, 170, 255, alpha]);
            }
        }
    }
    let image = images.add(Image::new(
        Extent3d {
            width: 64,
            height: 32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    for (index, mode) in [
        BillboardMode::ScreenAligned,
        BillboardMode::Spherical,
        BillboardMode::CylindricalY,
    ]
    .into_iter()
    .enumerate()
    {
        commands.spawn((
            Sprite3d {
                image: Some(image.clone()),
                custom_size: Some(Vec2::splat(2.0)),
                layers: vec![
                    SpriteLayer {
                        rect: Some(Rect::new(0.0, 0.0, 32.0, 32.0)),
                        ..default()
                    },
                    SpriteLayer {
                        rect: Some(Rect::new(32.0, 0.0, 64.0, 32.0)),
                        mask: MaskMode::Write {
                            reference: 1,
                            cutoff: 0.0,
                            visible: false,
                        },
                        ..default()
                    },
                    SpriteLayer {
                        rect: Some(Rect::new(32.0, 0.0, 64.0, 32.0)),
                        blend: BlendMode::Multiply,
                        mask: MaskMode::Read(1),
                        ..default()
                    },
                ],
                ..default()
            },
            Billboard {
                camera,
                mode,
                roll: 0.0,
            },
            Transform::from_xyz((index as f32 - 1.0) * 2.6, 0.0, 0.0),
        ));
    }
}

fn animate(
    time: Res<Time>,
    mut sprites: Query<&mut Sprite3d>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    let t = time.elapsed_secs();
    for mut sprite in &mut sprites {
        sprite.color.set_alpha(0.5 + 0.5 * (t * 0.8).cos());
    }
    for mut camera in &mut cameras {
        *camera = Transform::from_xyz(t.sin() * 3.0, 2.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
}
