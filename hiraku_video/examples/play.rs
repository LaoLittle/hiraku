use std::path::PathBuf;

use bevy::{asset::AssetPlugin, prelude::*};
use hiraku_video::{HirakuVideoPlugin, VideoAsset, VideoEvent, VideoPlayer};

#[derive(Resource)]
struct ExampleMovie(String);

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: cargo run -p hiraku-video --example play -- <movie.mkv|movie.webm>");
    let root = path
        .parent()
        .expect("movie path must have a parent directory")
        .to_string_lossy()
        .into_owned();
    let name = path
        .file_name()
        .expect("movie path must name a file")
        .to_string_lossy()
        .into_owned();

    App::new()
        .insert_resource(ExampleMovie(name))
        .add_plugins((
            DefaultPlugins.set(AssetPlugin {
                file_path: root,
                ..default()
            }),
            HirakuVideoPlugin,
        ))
        .add_systems(Startup, start_movie)
        .add_systems(Update, exit_when_finished)
        .run();
}

fn start_movie(
    mut commands: Commands,
    movie: Res<ExampleMovie>,
    asset_server: Res<AssetServer>,
    mut player: ResMut<VideoPlayer>,
) {
    commands.spawn((Camera2d, IsDefaultUiCamera));
    let asset: Handle<VideoAsset> = asset_server.load(movie.0.clone());
    player.play(asset);
}

fn exit_when_finished(mut events: MessageReader<VideoEvent>, mut exit: MessageWriter<AppExit>) {
    for event in events.read() {
        match event {
            VideoEvent::Started { .. } => {}
            VideoEvent::Finished { .. } | VideoEvent::Skipped { .. } => {
                exit.write(AppExit::Success);
            }
            VideoEvent::Failed { error, .. } => {
                error!("{error}");
                exit.write(AppExit::error());
            }
        }
    }
}
