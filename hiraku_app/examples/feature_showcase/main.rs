use hiraku_engine::RuntimeLaunchConfig;

fn main() {
    let mut config = RuntimeLaunchConfig::directory(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/feature_showcase/assets"
    ));
    config.window_title = "Hiraku — UI and Glossary".to_string();
    hiraku_app::run_app(config);
}
