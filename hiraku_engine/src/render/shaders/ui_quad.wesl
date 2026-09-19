import bevy_ui_render::ui_vertex_output::UiVertexOutput;
import hiraku_sprite3d::sampling::decode_color;

@group(1) @binding(0) var<uniform> tint: vec4<f32>;
@group(1) @binding(1) var<uniform> uv_u: vec4<f32>;
@group(1) @binding(2) var<uniform> uv_v: vec4<f32>;
@group(1) @binding(3) var source: texture_2d<f32>;
@group(1) @binding(4) var source_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let q = vec3<f32>(in.uv, 1.0);
    let uv = vec2<f32>(dot(uv_u.xyz, q), dot(uv_v.xyz, q));
    // Explicit gradients keep sampling valid on both sides of the discard.
    let color = decode_color(textureSampleGrad(source, source_sampler, uv, dpdx(uv), dpdy(uv)), sampling.x);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { discard; }
    return color * tint;
}
@group(1) @binding(15) var<uniform> sampling: vec4<u32>;
