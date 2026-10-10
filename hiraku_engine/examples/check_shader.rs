fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    if paths.is_empty() {
        return Err("usage: check_shader <shader.wgsl> ...".into());
    }
    for path in paths {
        let path = std::path::Path::new(&path);
        let source = std::fs::read_to_string(path)?;
        let module = naga::front::wgsl::parse_str(&source)
            .map_err(|error| error.emit_to_string_with_path(&source, &path.to_string_lossy()))?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .map_err(|error| error.emit_to_string_with_path(&source, &path.to_string_lossy()))?;
        println!("Shader validated: {}", path.display());
    }
    Ok(())
}
