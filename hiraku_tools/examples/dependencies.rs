fn main() -> Result<(), hiraku_tools::ToolError> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: dependencies ASSET_ROOT")?;
    let manifest = hiraku_tools::analyze_directory(std::path::Path::new(&path))?;
    println!(
        "{} scripts, {} resident images",
        manifest.scripts.len(),
        manifest.resident.len()
    );
    for (script, images) in &manifest.scripts {
        println!(
            "{script}: {} images{}",
            images.len(),
            if manifest.conservative.contains_key(script) {
                " (conservative)"
            } else {
                ""
            }
        );
    }
    for (script, queries) in &manifest.conservative {
        println!("{script}: {queries:?}");
    }
    Ok(())
}
