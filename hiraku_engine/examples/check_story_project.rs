//! Offline story compilation using the engine's actual project discovery path.
//! Does not open a window, decode assets or execute story functions.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [root, settings, entry] = arguments.as_slice() else {
        return Err("usage: check_story_project <asset-root> <settings-path> <entry-path>".into());
    };
    hiraku_engine::validate_story_project(std::path::Path::new(root), settings, entry)?;
    println!("Story project compiled and linked successfully.");
    Ok(())
}
