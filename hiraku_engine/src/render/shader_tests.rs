//! Exercise WESL linking without a window or a GPU. The vertex interfaces are
//! fixtures; rendering integration remains the embedding application's test.
use bevy::{
    asset::AssetId,
    shader::{Shader, ShaderCache, ShaderCacheSource, ShaderDefVal},
};

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
    let mut cache = ShaderCache::new((), |_, source, _| match source {
        ShaderCacheSource::Wgsl(source) => Ok(source),
        _ => panic!("expected linked WGSL"),
    });
    let id = |n| AssetId::Uuid {
        uuid: uuid::Uuid::from_u128(n),
    };
    cache.set_shader(
        id(3),
        Shader::from_wesl(
            include_str!("../../../hiraku_sprite3d/src/sampling.wesl"),
            "embedded://hiraku_sprite3d/sampling.wesl",
        ),
    );
    cache.set_shader(id(1), Shader::from_wesl(
        "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) uv: vec2<f32>, };",
        "embedded://bevy_pbr/render/forward_io.wesl"));
    cache.set_shader(id(2), Shader::from_wesl(
        "struct UiVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, };",
        "embedded://bevy_ui_render/ui_vertex_output.wesl"));
    for (index, (name, source)) in [
        ("world_sprite", include_str!("shaders/world_sprite.wesl")),
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
        ("blur", include_str!("../effect/shaders/blur_effect.wesl")),
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
        for multiply in [false, true] {
            cache
                .get(
                    index,
                    shader,
                    &[
                        ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 3),
                        ShaderDefVal::Bool("MASK_MULTIPLY".into(), multiply),
                    ],
                )
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }
}
