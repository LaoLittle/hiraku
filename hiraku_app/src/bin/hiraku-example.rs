use std::{env, fs, process};

use bevy::{camera::RenderTarget, window::WindowRef};

fn main() {
    let mut args = env::args().skip(1);
    let Some(example_name) = args.next() else {
        print_usage_and_exit();
    };

    if example_name == "--list" {
        print_examples();
        return;
    }

    let example_root = format!("examples/{example_name}");
    let metadata = fs::metadata(&example_root).unwrap_or_else(|_| {
        eprintln!("example not found: {example_name}");
        print_examples();
        process::exit(1);
    });

    if !metadata.is_dir() {
        eprintln!("example is not a directory: {example_root}");
        process::exit(1);
    }

    hiraku_app::run_app(hiraku_engine::RuntimeLaunchConfig {
        asset_root: example_root,
        settings_path: "settings.rhai".to_string(),
        default_startup_script: "startup.rhai".to_string(),
        window_title: format!("hiraku example: {example_name}"),
        render_target: RenderTarget::Window(WindowRef::Primary),
        camera_order: 0,
        camera_clear_color: bevy::camera::ClearColorConfig::Default,
    });
}

fn print_usage_and_exit() -> ! {
    eprintln!("usage: cargo run --bin hiraku-example -- <example-name>");
    eprintln!("       cargo run --bin hiraku-example -- --list");
    print_examples();
    process::exit(1);
}

fn print_examples() {
    let Ok(entries) = fs::read_dir("examples") else {
        eprintln!("no examples directory found");
        return;
    };

    eprintln!("available examples:");
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            eprintln!("- {}", entry.file_name().to_string_lossy());
        }
    }
}
