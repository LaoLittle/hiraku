use hiraku_engine::RuntimeLaunchConfig;

fn main() {
    let mut config = RuntimeLaunchConfig::directory(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/rich_text/assets"
    ));
    config.window_title = "Hiraku — Rich Text".to_string();
    config.storage_namespace = "hiraku-example-rich_text".to_string();
    hiraku_app::run_app(config);
}

#[cfg(test)]
mod tests {
    #[test]
    fn scripts_typecheck_and_ui_builds_without_a_window() {
        let root = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/rich_text/assets"
        ));
        hiraku_engine::validate_story_project(root, "settings.hson", "startup.hks")
            .expect("example story");
        for path in ["ui/showcase.ui.hks", "ui/dialogue.ui.hks"] {
            hiraku_engine::validate_ui_document(root, "settings.hson", path).expect("example UI");
        }
    }
}
