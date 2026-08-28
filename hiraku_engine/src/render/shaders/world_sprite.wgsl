#import bevy_pbr::forward_io::VertexOutput

struct WorldSpriteMaterial {
    tint: vec4<f32>,
    // `[left, top, width, height]`; a zero size selects the full image.
    rect: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: WorldSpriteMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var color_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let texture_size = vec2<f32>(textureDimensions(color_texture));
    let full_image = material.rect.z <= 0.0 || material.rect.w <= 0.0;
    let uv = select(
        (material.rect.xy + mesh.uv * material.rect.zw) / texture_size,
        mesh.uv,
        full_image,
    );
    let color = textureSample(color_texture, color_sampler, uv) * material.tint;
    if color.a <= 0.0001 {
        discard;
    }
    return color;
}
