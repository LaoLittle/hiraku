import bevy_pbr::render::forward_io::VertexOutput;
import hiraku_sprite3d::sampling::decode_color;

struct MultiplyMaterial {
    tint: vec4<f32>,
    rect: vec4<f32>,
    opacity: f32,
    _padding: vec3<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: MultiplyMaterial;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var color_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var color_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let texture_size = vec2<f32>(textureDimensions(color_texture));
    let uv = (material.rect.xy + mesh.uv * material.rect.zw) / texture_size;
    var color = decode_color(textureSample(color_texture, color_sampler, uv), sampling.x) * material.tint;
    color.a *= material.opacity;
    if color.a <= 0.0001 {
        discard;
    }
    return vec4(color.rgb * color.a, color.a);
}
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var<uniform> sampling: vec4<u32>;
