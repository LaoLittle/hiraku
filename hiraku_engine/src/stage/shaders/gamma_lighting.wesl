import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings::view,
};
import bevy_render::color_operations::{linear_rgb_to_srgb, srgb_to_linear_rgb};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> color_factor: vec4<f32>;

// Keep the imported textures, UVs, normals and alpha test. Lighting is evaluated
// in the authoring space, then converted once to the engine's linear target.
// This is gamma-space Bevy PBR, not a complete emulation of another engine's BRDF.
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr = pbr_input_from_standard_material(in, is_front);
    pbr.material.base_color = alpha_discard(pbr.material.flags, pbr.material.alpha_cutoff, pbr.material.base_color);
    var out: FragmentOutput;
    if (pbr.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        pbr.material.base_color = vec4(linear_rgb_to_srgb(pbr.material.base_color.rgb) * color_factor.rgb, pbr.material.base_color.a);
        pbr.material.emissive = vec4(linear_rgb_to_srgb(pbr.material.emissive.rgb), pbr.material.emissive.a);
        let lit = apply_pbr_lighting(pbr);
        // Exposure belongs to the linear presentation domain, not the gamma
        // lighting arithmetic. Stages normally use exposure 1 for this mode.
        let exposure = max(view.exposure, 0.000001);
        out.color = vec4(srgb_to_linear_rgb(max(lit.rgb / exposure, vec3(0.0))) * exposure, lit.a);
    } else {
        // Unlit assets already carry linear samples; do not decode twice.
        out.color = pbr.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr, out.color);
    return out;
}
