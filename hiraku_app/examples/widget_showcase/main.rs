use hiraku_engine::RuntimeLaunchConfig;

fn main() {
    let mut config = RuntimeLaunchConfig::directory(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/widget_showcase/assets"
    ));
    config.window_title = "Hiraku — Widget Showcase".to_string();
    config.storage_namespace = "hiraku-example-widget_showcase".to_string();
    hiraku_app::run_app(config);
}
