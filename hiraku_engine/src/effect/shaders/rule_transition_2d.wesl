import bevy_pbr::render::forward_io::VertexOutput;
import hiraku_sprite3d::sampling::decode_color;

struct RuleTransitionMaterial {
    progress: f32,
    vague: f32,
    _padding: vec2<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: RuleTransitionMaterial;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var from_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var from_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var to_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var to_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var rule_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var rule_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv;
    let from_color = decode_color(textureSample(from_texture, from_sampler, uv), sampling.x);
    let to_color = decode_color(textureSample(to_texture, to_sampler, uv), sampling.y);
    let rule = textureSample(rule_texture, rule_sampler, uv).r;
    let vague = max(material.vague, 0.0001);
    let edge = clamp((material.progress - rule) / vague + 0.5, 0.0, 1.0);
    return mix(from_color, to_color, edge);
}
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var<uniform> sampling: vec4<u32>;
