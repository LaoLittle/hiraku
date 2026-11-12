use bevy::{
    asset::AssetId,
    shader::{Shader, ShaderCache, ShaderCacheError, ShaderCacheSource, ShaderDefVal},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: check_stage_effect_shader <shader.wesl>")?;
    let source = std::fs::read_to_string(&path)?;
    let interfaces = [
        (
            "bevy_pbr/render/forward_io",
            "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32> }; struct FragmentOutput { @location(0) color: vec4<f32> };",
        ),
        (
            "bevy_pbr/prepass/utils",
            "fn prepass_depth(position: vec4<f32>, sample: u32) -> f32 { return position.z; }",
        ),
        (
            "bevy_pbr/render/view_transformations",
            "fn depth_ndc_to_view_z(depth: f32) -> f32 { return -1.0 / depth; }",
        ),
        (
            "bevy_render/color_operations",
            "fn srgb_to_linear_rgb(color: vec3<f32>) -> vec3<f32> { return color; }",
        ),
        ("stage_effect/check", source.as_str()),
    ];
    let mut cache = ShaderCache::new((), |_, source, _| {
        let ShaderCacheSource::Wgsl(source) = source else {
            return Err(ShaderCacheError::CreateShaderModule(
                "expected WGSL".to_owned(),
            ));
        };
        let module = naga::front::wgsl::parse_str(&source)
            .map_err(|error| ShaderCacheError::CreateShaderModule(error.emit_to_string(&source)))?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .map_err(|error| ShaderCacheError::CreateShaderModule(error.emit_to_string(&source)))?;
        Ok(source)
    });
    for (index, (name, source)) in interfaces.iter().enumerate() {
        cache.set_shader(
            AssetId::Uuid {
                uuid: uuid::Uuid::from_u128(index as u128 + 1),
            },
            Shader::from_wesl((*source).to_owned(), format!("embedded://{name}.wesl")),
        );
    }
    cache
        .get(
            0,
            AssetId::Uuid {
                uuid: uuid::Uuid::from_u128(interfaces.len() as u128),
            },
            &[ShaderDefVal::UInt("EFFECT_BINDING_GROUP".into(), 3)],
        )
        .map_err(|error| format!("stage shader interface check failed: {error:?}"))?;
    println!(
        "Stage effect shader linked and passed Naga validation against the mesh/depth interface fixtures. GPU pipeline and visual verification are not performed."
    );
    if let Some(path) = arguments.next() {
        let path = std::path::Path::new(&path);
        let definition: hiraku_engine::stage::StageDefinition =
            hiraku_script::hson::from_slice(&std::fs::read(path)?)?;
        definition.validate()?;
        for effect in definition.effects.values() {
            for dependency in std::iter::once(&effect.shader).chain(&effect.textures) {
                if !path
                    .parent()
                    .ok_or("stage descriptor has no parent")?
                    .join(dependency)
                    .is_file()
                {
                    return Err(format!("missing stage effect dependency: {dependency}").into());
                }
            }
        }
        println!("Stage descriptor and effect dependency paths validated.");
    }
    Ok(())
}
