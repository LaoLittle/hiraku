use bevy::{
    camera::{ClearColorConfig, RenderTarget},
    prelude::*,
    render::render_resource::TextureFormat,
};

const TARGET_WIDTH: u32 = 1280;
const TARGET_HEIGHT: u32 = 720;

#[derive(Resource)]
struct HirakuRenderTexture(Handle<Image>);

fn main() {
    let mut app = hiraku_app::build_app(hiraku_engine::RuntimeLaunchConfig {
        asset_root: "examples/atri_3d_overlay".to_string(),
        settings_path: "settings.rhai".to_string(),
        default_startup_script: "startup.rhai".to_string(),
        window_title: "hiraku: image render target".to_string(),
        render_target: RenderTarget::Window(bevy::window::WindowRef::Primary),
        camera_order: 0,
        camera_clear_color: ClearColorConfig::Default,
    });

    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            TARGET_WIDTH,
            TARGET_HEIGHT,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        ));
    app.world_mut()
        .resource_mut::<hiraku_engine::RuntimeLaunchConfig>()
        .render_target = RenderTarget::Image(target.clone().into());
    app.insert_resource(HirakuRenderTexture(target));
    app.add_systems(Startup, setup_preview);
    app.run();
}

fn setup_preview(mut commands: Commands, target: Res<HirakuRenderTexture>) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.03, 0.05)),
            ..default()
        },
    ));
    commands.spawn((
        Sprite::from_image(target.0.clone()),
        Transform::from_scale(Vec3::splat(0.75)),
    ));
}
