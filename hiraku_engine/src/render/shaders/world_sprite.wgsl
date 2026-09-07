#import bevy_pbr::forward_io::VertexOutput

struct WorldSpriteMaterial {
    effects: vec4<f32>,
    dissolve: vec4<f32>,
    tint: vec4<f32>,
    // `[left, top, width, height]`; a zero size selects the full image.
    rect: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: WorldSpriteMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var dissolve_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var dissolve_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let texture_size = vec2<f32>(textureDimensions(color_texture));
    let full_image = material.rect.z <= 0.0 || material.rect.w <= 0.0;
    let uv = select(
        (material.rect.xy + mesh.uv * material.rect.zw) / texture_size,
        mesh.uv,
        full_image,
    );
    var sampled = textureSample(color_texture, color_sampler, uv);
    if material.effects.x > 0.0 {
        let origin = select(material.rect.xy, vec2<f32>(0.0), full_image);
        let size = select(material.rect.zw, texture_size, full_image);
        let low = (origin + vec2<f32>(0.5)) / texture_size;
        let high = (origin + size - vec2<f32>(0.5)) / texture_size;
        let weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
        var sum = vec4<f32>(0.0);
        for (var y = 0u; y < 5u; y += 1u) {
            for (var x = 0u; x < 5u; x += 1u) {
                let offset = (vec2<f32>(f32(x), f32(y)) - vec2<f32>(2.0)) * material.effects.x * 0.5 / texture_size;
                let tap = textureSampleLevel(color_texture, color_sampler, clamp(uv + offset, low, high), 0.0);
                // Filter premultiplied color to avoid dark fringes at transparent edges.
                sum += vec4<f32>(tap.rgb * tap.a, tap.a) * weights[x] * weights[y];
            }
        }
        let alpha = sum.a / 256.0;
        sampled = vec4<f32>(sum.rgb / max(sum.a, 0.000001), alpha);
    }
    var color = sampled * material.tint;
    if material.dissolve.x != 0.0 {
        // Screen coordinates keep a full-canvas mask independent of the large
        // scene-covering quad and of camera zoom/projection.
        let threshold = textureSample(dissolve_texture, dissolve_sampler, mesh.position.xy / material.dissolve.zw).r;
        // Both entering and leaving traverse the mask from low to high.
        let reversed = material.dissolve.x < 0.0;
        let progress = select(material.tint.a, 1.0 - material.tint.a, reversed);
        let softness = material.dissolve.y;
        var coverage = step(threshold, progress);
        if softness > 0.0 {
            coverage = smoothstep(threshold, threshold + softness, progress * (1.0 + softness));
        }
        // Ensure exact endpoints even when the mask contains black or white.
        coverage = select(coverage, 0.0, progress <= 0.0);
        coverage = select(coverage, 1.0, progress >= 1.0);
        color.a = select(coverage, 1.0 - coverage, reversed);
    }
    if color.a <= 0.0001 {
        discard;
    }
    return color;
}
