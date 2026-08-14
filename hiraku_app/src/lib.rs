use bevy::{
    asset::{AssetMetaCheck, AssetPlugin},
    prelude::*,
    window::WindowPlugin,
};
use hiraku_engine::{HirakuPluginGroup, RuntimeLaunchConfig, configure_runtime_app};

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

    app
}
