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

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_scripts_and_descriptors_parse_without_external_assets() {
        for source in [
            include_str!("assets/startup.hks"),
            include_str!("assets/ui/widgets.ui.hks"),
        ] {
            hiraku_script::parse_program(source).expect("example script must parse");
        }
        for source in [
            include_str!("assets/settings.hson"),
            include_str!("assets/textures/palette.texture.hson"),
        ] {
            hiraku_script::hson::from_str::<hiraku_script::hson::HsonValue>(source)
                .expect("example descriptor must parse");
        }
    }
}
