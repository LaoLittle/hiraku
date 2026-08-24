#import bevy_sprite::mesh2d_vertex_output::VertexOutput

struct MultiplyMaterial {
    tint: vec4<f32>,
    rect: vec4<f32>,
    opacity: f32,
    _padding: vec3<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: MultiplyMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var color_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let texture_size = vec2<f32>(textureDimensions(color_texture));
    let uv = (material.rect.xy + mesh.uv * material.rect.zw) / texture_size;
    var color = textureSample(color_texture, color_sampler, uv) * material.tint;
    color.a *= material.opacity;
    if color.a <= 0.0001 {
        discard;
    }
    return vec4(color.rgb * color.a, color.a);
}
