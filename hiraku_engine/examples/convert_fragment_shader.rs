fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: convert_fragment_shader <fragment.glsl>")?;
    let source = std::fs::read_to_string(path)?;
    let module = naga::front::glsl::Frontend::default()
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
            &source,
        )
        .map_err(|error| error.emit_to_string(&source))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)?;
    let output = naga::back::wgsl::write_string(
        &module,
        &info,
        naga::back::wgsl::WriterFlags::EXPLICIT_TYPES,
    )?;
    print!("{output}");
    Ok(())
}
