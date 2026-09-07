#import bevy_pbr::forward_io::VertexOutput
struct Layer {
    rect: vec4<f32>, bounds: vec4<f32>, tint: vec4<f32>, modes: vec4<f32>, flip: vec4<f32>,
};
struct Sprite { tint: vec4<f32>, backface_tint: vec4<f32>, count: vec4<u32>, layers: array<Layer, 32>, };
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sprite: Sprite;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var atlas: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var atlas_sampler: sampler;

// Premultiplied operands, including genuinely translucent layers.
fn compose(dst: vec4<f32>, src: vec4<f32>, multiply: bool) -> vec4<f32> {
    let alpha = src.a + dst.a * (1.0 - src.a);
    if multiply {
        return vec4<f32>(src.rgb * (1.0 - dst.a) + dst.rgb * (1.0 - src.a) + src.rgb * dst.rgb, alpha);
    }
    return vec4<f32>(src.rgb + dst.rgb * (1.0 - src.a), alpha);
}
@fragment
fn fragment(mesh: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(atlas));
    var masks: array<f32, 8>;
    var result = vec4<f32>(0.0);
    for (var i = 0u; i < min(sprite.count.x, 32u); i += 1u) {
        let layer = sprite.layers[i];
        var uv = (mesh.uv - layer.bounds.xy) / layer.bounds.zw;
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { continue; }
        uv = select(uv, vec2<f32>(1.0) - uv, layer.flip.xy > vec2<f32>(0.0));
        let origin = select(layer.rect.xy, vec2<f32>(0.0), layer.rect.z <= 0.0);
        let size = select(layer.rect.zw, dimensions, layer.rect.z <= 0.0);
        // Clamp to source texel centers, avoiding adjacent atlas entries.
        let sample_uv = (origin + clamp(uv * size, vec2<f32>(0.5), max(size - 0.5, vec2<f32>(0.5)))) / dimensions;
        let sample_color = textureSampleLevel(atlas, atlas_sampler, sample_uv, 0.0) * layer.tint;
        var alpha = sample_color.a;
        let mode = u32(layer.modes.y);
        let reference = u32(layer.modes.z);
        if mode != 0u && reference >= 1u && reference <= 8u {
            if mode == 1u { alpha *= masks[reference - 1u]; }
            else {
                let coverage = select(0.0, alpha, alpha > layer.modes.w);
                masks[reference - 1u] = max(masks[reference - 1u], coverage);
                if mode == 2u { continue; }
            }
        }
        result = compose(result, vec4<f32>(sample_color.rgb * alpha, alpha), layer.modes.x > 0.0);
    }
    // Apply group opacity only after internal overlaps have been resolved.
    let tint = sprite.tint * select(sprite.backface_tint, vec4<f32>(1.0), front);
    return vec4<f32>(result.rgb * tint.rgb * tint.a, result.a * tint.a);
}
