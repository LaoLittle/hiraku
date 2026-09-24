//! Exercise WESL linking without a window or a GPU. The vertex interfaces are
//! fixtures; rendering integration remains the embedding application's test.
use bevy::shader::{ShaderCache, ShaderCacheSource, ShaderDefVal};

#[test]
fn native_ui_sampling_links_against_bevys_real_shader_interfaces() {
    use bevy::prelude::*;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Shader>()
        .init_asset_loader::<bevy::shader::ShaderLoader>()
        .init_asset::<Image>()
        .init_asset::<TextureAtlasLayout>()
        .add_plugins((
            bevy::ui_render::UiRenderPlugin,
            bevy::ui_render::ui_texture_slice_pipeline::UiTextureSlicerPlugin,
        ));
    let server = app.world().resource::<AssetServer>();
    let handles: Vec<Handle<Shader>> = [
        "embedded://bevy_ui_render/ui.wesl",
        "embedded://bevy_ui_render/ui_texture_slice.wesl",
    ]
    .into_iter()
    .map(|path| server.load(path))
    .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.update();
        if handles
            .iter()
            .all(|h| app.world().resource::<Assets<Shader>>().contains(h))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "embedded Bevy shaders did not load"
        );
        std::thread::yield_now();
    }
    let mut cache = ShaderCache::new((), |_, source, _| match source {
        ShaderCacheSource::Wgsl(source) => Ok(source),
        _ => panic!("expected WGSL"),
    });
    for handle in handles {
        cache.set_shader(
            handle.id(),
            app.world()
                .resource::<Assets<Shader>>()
                .get(&handle)
                .expect("shader")
                .clone(),
        );
    }
    let id = |n| AssetId::Uuid {
        uuid: uuid::Uuid::from_u128(n),
    };
    cache.set_shader(
        id(1),
        Shader::from_wesl(
            "struct View { clip_from_world: mat4x4<f32>, };",
            "embedded://bevy_render/view.wesl",
        ),
    );
    cache.set_shader(
        id(2),
        Shader::from_wesl(
            "struct Globals { time: f32, };",
            "embedded://bevy_render/globals.wesl",
        ),
    );
    cache.set_shader(
        id(3),
        Shader::from_wesl(
            include_str!("../../../hiraku_sprite3d/src/sampling.wesl"),
            "embedded://hiraku_sprite3d/sampling.wesl",
        ),
    );
    for (index, (name, source)) in [
        ("color_ui", include_str!("shaders/color_ui.wesl")),
        (
            "color_ui_slice",
            include_str!("shaders/color_ui_slice.wesl"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        cache.set_shader(
            id(10),
            Shader::from_wesl(source, format!("embedded://fixture/{name}.wesl")),
        );
        for mode in 0..=5 {
            for aa in [false, true] {
                cache
                    .get(
                        index,
                        id(10),
                        &[
                            ShaderDefVal::UInt("COLOR_SAMPLING".into(), mode),
                            ShaderDefVal::Bool("ANTI_ALIAS".into(), aa),
                        ],
                    )
                    .unwrap_or_else(|error| panic!("{name}, mode {mode}: {error}"));
            }
        }
    }
}

#[test]
fn embedded_wesl_modules_link_for_material_variants() {
    use bevy::prelude::*;
    // Use the production plugin's asset registration. Do not manually inject
    // these modules into the cache: that hid unloaded library dependencies.
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Shader>()
        .init_asset_loader::<bevy::shader::ShaderLoader>()
        .init_asset::<Image>()
        .add_plugins(crate::effect::post_process::PostProcessPlugin);
    let required = ["input", "material", "fullscreen"];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.update();
        let shaders = app.world().resource::<Assets<Shader>>();
        if required.iter().all(|name| {
            shaders.iter().any(|(_, shader)| {
                shader.path == format!("embedded://hiraku_engine/effect/shaders/{name}.wesl")
            })
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "effect plugin did not load its shader libraries"
        );
        std::thread::yield_now();
    }
    let mut cache = ShaderCache::new((), |_, source, _| match source {
        ShaderCacheSource::Wgsl(source) => {
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
            Ok(source)
        }
        _ => panic!("expected linked WGSL"),
    });
    let id = |n| AssetId::Uuid {
        uuid: uuid::Uuid::from_u128(n),
    };
    for (id, shader) in app.world().resource::<Assets<Shader>>().iter() {
        cache.set_shader(id, shader.clone());
    }
    cache.set_shader(
        id(3),
        Shader::from_wesl(
            include_str!("../../../hiraku_sprite3d/src/sampling.wesl"),
            "embedded://hiraku_sprite3d/sampling.wesl",
        ),
    );
    cache.set_shader(id(1), Shader::from_wesl(
        "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32>, };",
        "embedded://bevy_pbr/render/forward_io.wesl"));
    cache.set_shader(id(2), Shader::from_wesl(
        "struct UiVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, };",
        "embedded://bevy_ui_render/ui_vertex_output.wesl"));
    for (index, (name, source)) in [
        ("standard", include_str!("../effect/shaders/standard.wesl")),
        ("alpha_mask", include_str!("shaders/alpha_mask.wesl")),
        ("multiply", include_str!("shaders/multiply.wesl")),
        ("ui_quad", include_str!("shaders/ui_quad.wesl")),
        (
            "rule_transition",
            include_str!("../effect/shaders/rule_transition_2d.wesl"),
        ),
        (
            "custom_screen",
            include_str!("../effect/shaders/custom_screen_effect.wesl"),
        ),
        ("blur", include_str!("../effect/shaders/blur.wesl")),
        (
            "sprite3d",
            include_str!("../../../hiraku_sprite3d/src/sprite3d.wesl"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let shader = id(index as u128 + 10);
        cache.set_shader(
            shader,
            Shader::from_wesl(source, format!("embedded://fixture/{name}.wesl")),
        );
        for (material, multiply, stage) in [
            (false, false, 0),
            (false, false, 1),
            (false, false, 2),
            (false, false, 3),
            (true, false, 0),
            (true, true, 0),
        ] {
            cache
                .get(
                    index,
                    shader,
                    &[
                        ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 3),
                        ShaderDefVal::UInt("EFFECT_STAGE".into(), stage),
                        ShaderDefVal::Bool("MASK_MULTIPLY".into(), multiply),
                        ShaderDefVal::Bool("HIRAKU_MATERIAL".into(), material),
                        ShaderDefVal::UInt(
                            "EFFECT_BINDING_GROUP".into(),
                            if material { 3 } else { 0 },
                        ),
                    ],
                )
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    // One unchanged third-party entry, no knowledge of blur, scopes or bindings.
    // The same shader handle is specialized for every layer and material.
    let source = r#"
        import constants::EFFECT_BINDING_GROUP;
        import hiraku::render::{VertexOutput, FragmentOutput, source, sampleColor, finish};
        struct InvertUniform { strength: f32, }
        @group(EFFECT_BINDING_GROUP) @binding(7) var<uniform> settings: InvertUniform;
        @fragment
        fn fragment(input: VertexOutput) -> FragmentOutput {
            let surface = source(input);
            let color = sampleColor(surface, surface.uv);
            return finish(input, vec4<f32>(mix(color.rgb, vec3<f32>(1.0) - color.rgb, settings.strength), color.a));
        }
    "#;
    let program = crate::EffectShader::from_wesl(
        &mut app.world_mut().resource_mut::<Assets<Shader>>(),
        source,
    );
    for (id, shader) in app.world().resource::<Assets<Shader>>().iter() {
        cache.set_shader(id, shader.clone());
    }
    assert_eq!(
        app.world()
            .resource::<Assets<Shader>>()
            .get(program.source())
            .expect("author shader")
            .source
            .as_str(),
        source
    );
    for material in [true, false] {
        for stage in 0..3 {
            let linked = cache
                .get(
                    100 + stage as usize,
                    program.source().id(),
                    &[
                        ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 3),
                        ShaderDefVal::UInt("EFFECT_STAGE".into(), stage),
                        ShaderDefVal::Bool("HIRAKU_MATERIAL".into(), material),
                        ShaderDefVal::UInt(
                            "EFFECT_BINDING_GROUP".into(),
                            if material { 3 } else { 0 },
                        ),
                    ],
                )
                .expect("author fragment must link and validate for every target");
            let module = naga::front::wgsl::parse_str(&linked).expect("validated WGSL");
            assert_eq!(module.entry_points.len(), 1, "no generated entry points");
            assert_eq!(module.entry_points[0].stage, naga::ShaderStage::Fragment);
            let expected_group = if material { 3 } else { 0 };
            for (_, variable) in module.global_variables.iter() {
                if let Some(binding) = &variable.binding {
                    assert_eq!(
                        binding.group, expected_group,
                        "target bindings must not leak between specializations"
                    );
                }
            }
        }
    }
}
