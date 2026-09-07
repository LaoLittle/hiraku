//! Offline story checking. Supply paths explicitly; no bundled game fixtures.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = std::env::args().skip(1).collect::<Vec<_>>();
    if paths.first().is_some_and(|arg| arg == "--ui-project") {
        if paths.len() < 4 {
            return Err("usage: --ui-project ROOT SETTINGS UI_PATH...".into());
        }
        let root = std::path::PathBuf::from(paths.remove(1));
        let settings = paths.remove(1);
        for path in &paths[1..] {
            hiraku_engine::validate_ui_document(&root, &settings, path)?;
        }
        println!("Built {} UI documents without rendering", paths.len() - 1);
        return Ok(());
    }
    if paths.is_empty() {
        return Err("provide one or more HKS source paths".into());
    }
    for path in &paths {
        let source = std::fs::read_to_string(path)?;
        if path.ends_with(".ui.hks") {
            hiraku_engine::validate_ui_source(path, &source)?;
        } else {
            hiraku_engine::validate_story_source(path, &source)?;
        }
    }
    println!("Validated {} script sources", paths.len());
    Ok(())
}
