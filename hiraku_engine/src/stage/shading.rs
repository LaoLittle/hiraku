//! Opt-in compatibility for assets authored with gamma-space lighting.
//! Normal scenes continue using StandardMaterial and linear-light PBR.
use bevy::{
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageShading {
    #[default]
    Linear,
    Gamma,
}

impl StageShading {
    pub(super) fn light_color(self, color: Color) -> Color {
        match self {
            Self::Linear => color,
            Self::Gamma => {
                let color = color.to_srgba();
                Color::linear_rgba(color.red, color.green, color.blue, color.alpha)
            }
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct GammaLighting {
    /// Authored sRGB factor, applied after decoding the texture sample back
    /// into the lighting space. Alpha remains on StandardMaterial.
    #[uniform(100)]
    pub color_factor: Vec4,
}

impl Default for GammaLighting {
    fn default() -> Self {
        Self {
            color_factor: Vec4::ONE,
        }
    }
}

pub type GammaStageMaterial = ExtendedMaterial<StandardMaterial, GammaLighting>;

impl MaterialExtension for GammaLighting {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/stage/shaders/gamma_lighting.wesl".into()
    }
    // Gamma lighting must be evaluated before returning to the linear target;
    // it cannot use a shared deferred lighting pass.
    fn enable_oit() -> bool {
        false
    }
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/gamma_lighting.wesl");
    app.add_plugins(MaterialPlugin::<GammaStageMaterial>::default());
}

pub(super) fn material(mut base: StandardMaterial) -> GammaStageMaterial {
    base.opaque_render_method = bevy::material::OpaqueRendererMethod::Forward;
    let color = base.base_color.to_srgba();
    let color_factor = Vec4::new(color.red, color.green, color.blue, 1.0);
    if !base.unlit {
        // Encoding a product of linear samples is NOT the product of their
        // sRGB encodings. Keep the tint out of Bevy's texture multiplication.
        base.base_color = Color::linear_rgba(1.0, 1.0, 1.0, color.alpha);
    }
    GammaStageMaterial {
        base,
        extension: GammaLighting { color_factor },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::shader::{ShaderCache, ShaderCacheSource};

    #[test]
    fn gamma_tint_is_applied_separately_from_the_linear_texture_sample() {
        let tint = Srgba::new(0.2, 0.15, 0.1, 0.6);
        let source = StandardMaterial {
            base_color: Color::Srgba(tint),
            ..default()
        };
        let adapted = material(source.clone());
        assert_eq!(
            adapted.base.base_color,
            Color::linear_rgba(1.0, 1.0, 1.0, 0.6)
        );
        assert_eq!(
            adapted.extension.color_factor,
            Vec4::new(0.2, 0.15, 0.1, 1.0)
        );
        let sample = LinearRgba::from(Srgba::new(0.5, 0.5, 0.5, 1.0));
        let correct = Srgba::from(sample).red * adapted.extension.color_factor.x;
        assert!((correct - 0.1).abs() < 1e-6);
        let old = Srgba::from(LinearRgba::new(
            sample.red * LinearRgba::from(tint).red,
            0.0,
            0.0,
            1.0,
        ))
        .red;
        assert!(old < correct * 0.8, "the old path darkened tinted midtones");
        let unlit = material(StandardMaterial {
            unlit: true,
            ..source.clone()
        });
        assert_eq!(unlit.base.base_color, source.base_color);
    }

    #[test]
    fn gamma_color_coefficients_are_opt_in() {
        let authored = Color::srgb(0.5, 0.25, 1.0);
        assert_eq!(StageShading::default().light_color(authored), authored);
        assert_eq!(
            StageShading::Gamma.light_color(authored),
            Color::linear_rgb(0.5, 0.25, 1.0)
        );
        // A gamma result of 0.5 must enter the linear compositor as ~0.214,
        // not 0.5 (which the output transform would brighten a second time).
        assert!((LinearRgba::from(Srgba::new(0.5, 0.5, 0.5, 1.0)).red - 0.21404114).abs() < 1e-6);
    }

    #[test]
    fn gamma_fragment_links_and_validates_against_its_pbr_interface() {
        // Interface fixtures keep this test independent of a window/GPU. Full
        // Bevy pipeline and original-game visual parity remain integration QA.
        let modules = [
            (
                "bevy_pbr/render/forward_io",
                "struct VertexOutput { @builtin(position) position: vec4<f32> }; struct FragmentOutput { @location(0) color: vec4<f32> };",
            ),
            (
                "bevy_pbr/render/pbr_types",
                "const STANDARD_MATERIAL_FLAGS_UNLIT_BIT: u32 = 1u; struct Material { flags: u32, alpha_cutoff: f32, base_color: vec4<f32>, emissive: vec4<f32> }; struct PbrInput { material: Material };",
            ),
            (
                "bevy_pbr/render/pbr_fragment",
                "import package::render::{forward_io::VertexOutput, pbr_types::{PbrInput, Material}}; fn pbr_input_from_standard_material(v: VertexOutput, front: bool) -> PbrInput { return PbrInput(Material(0u, 0.5, vec4(0.5), vec4(0.0))); }",
            ),
            (
                "bevy_pbr/render/pbr_functions",
                "import package::render::pbr_types::PbrInput; fn alpha_discard(flags: u32, cutoff: f32, color: vec4<f32>) -> vec4<f32> { return color; } fn apply_pbr_lighting(p: PbrInput) -> vec4<f32> { return p.material.base_color; } fn main_pass_post_lighting_processing(p: PbrInput, color: vec4<f32>) -> vec4<f32> { return color; }",
            ),
            (
                "bevy_pbr/render/mesh_view_bindings",
                "struct View { exposure: f32 }; @group(0) @binding(0) var<uniform> view: View;",
            ),
            (
                "bevy_render/color_operations",
                "fn linear_rgb_to_srgb(v: vec3<f32>) -> vec3<f32> { return v; } fn srgb_to_linear_rgb(v: vec3<f32>) -> vec3<f32> { return v; }",
            ),
            (
                "hiraku_engine/stage/shaders/gamma_lighting",
                include_str!("shaders/gamma_lighting.wesl"),
            ),
        ];
        let mut cache = ShaderCache::new((), |_, source, _| {
            let ShaderCacheSource::Wgsl(source) = source else {
                panic!("WGSL expected")
            };
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            Ok(source)
        });
        for (i, (name, source)) in modules.iter().enumerate() {
            cache.set_shader(
                AssetId::Uuid {
                    uuid: uuid::Uuid::from_u128(i as u128 + 1),
                },
                Shader::from_wesl(*source, format!("embedded://{name}.wesl")),
            );
        }
        cache
            .get(
                0,
                AssetId::Uuid {
                    uuid: uuid::Uuid::from_u128(modules.len() as u128),
                },
                &[bevy::shader::ShaderDefVal::UInt(
                    "MATERIAL_BIND_GROUP".into(),
                    3,
                )],
            )
            .expect("gamma shader links");
    }
}
