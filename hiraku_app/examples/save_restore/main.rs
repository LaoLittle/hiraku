use hiraku_engine::RuntimeLaunchConfig;

fn main() {
    let mut config = RuntimeLaunchConfig::directory(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/save_restore/assets"
    ));
    config.window_title = "Hiraku — Save and Restore".to_string();
    hiraku_app::run_app(config);
}
