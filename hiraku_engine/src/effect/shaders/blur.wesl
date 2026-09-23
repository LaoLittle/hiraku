// Complete Dual Kawase effect. All outputs are linear premultiplied RGBA.
// The host schedules these entry points; no pass reads its own render target.
import constants::EFFECT_BINDING_GROUP;
import hiraku_sprite3d::sampling::decode_color;

struct BlurUniform {
    source_rect: vec4<f32>,
    source_size: vec2<f32>,
    destination_size: vec2<f32>,
    sampling: u32,
    premultiplied: u32,
    blend: f32,
    padding: f32,
}
@group(EFFECT_BINDING_GROUP) @binding(0) var image: texture_2d<f32>;
@group(EFFECT_BINDING_GROUP) @binding(1) var image_sampler: sampler;
@group(EFFECT_BINDING_GROUP) @binding(2) var baseline: texture_2d<f32>;
@group(EFFECT_BINDING_GROUP) @binding(7) var<uniform> settings: BlurUniform;

fn tap(uv: vec2<f32>) -> vec4<f32> {
    let half_texel = 0.5 / vec2<f32>(textureDimensions(image));
    return textureSampleLevel(image, image_sampler, clamp(uv, half_texel, 1.0 - half_texel), 0.0);
}

// Decode once, crop the atlas entry and premultiply BEFORE any filtering.
// Pixel-aligned copies use the original texel centers, not neighboring entries.
@fragment
fn prepare(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local = position.xy / settings.destination_size;
    let low = (settings.source_rect.xy + 0.5) / settings.source_size;
    let high = (settings.source_rect.xy + settings.source_rect.zw - 0.5) / settings.source_size;
    let uv = (settings.source_rect.xy + local * settings.source_rect.zw) / settings.source_size;
    let c = decode_color(textureSampleLevel(image, image_sampler, clamp(uv, low, high), 0.0), settings.sampling);
    return vec4<f32>(select(c.rgb * c.a, c.rgb, settings.premultiplied != 0u), c.a);
}

// Center and four diagonal bilinear samples, normalized to preserve flat colors.
@fragment
fn downsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / settings.destination_size;
    let d = 0.5 / vec2<f32>(textureDimensions(image));
    return (tap(uv) * 4.0
        + tap(uv + d) + tap(uv - d)
        + tap(uv + vec2<f32>(d.x, -d.y))
        + tap(uv + vec2<f32>(-d.x, d.y))) / 8.0;
}

// Eight taps mix axial and diagonal directions, rather than reproducing four
// widely separated copies. Every pass reads the preceding filtered image.
@fragment
fn upsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / settings.destination_size;
    let d = 0.5 / vec2<f32>(textureDimensions(image));
    let color = (
        tap(uv + vec2<f32>(-2.0 * d.x, 0.0))
        + tap(uv + vec2<f32>(2.0 * d.x, 0.0))
        + tap(uv + vec2<f32>(0.0, -2.0 * d.y))
        + tap(uv + vec2<f32>(0.0, 2.0 * d.y))
        + 2.0 * (tap(uv + d) + tap(uv - d)
            + tap(uv + vec2<f32>(d.x, -d.y))
            + tap(uv + vec2<f32>(-d.x, d.y)))
    ) / 12.0;
    // Blend only the deepest reconstruction with its unblurred pyramid level.
    // A newly introduced level starts at zero weight, avoiding animation pops.
    let original = textureSampleLevel(baseline, image_sampler, uv, 0.0);
    return mix(original, color, settings.blend);
}
