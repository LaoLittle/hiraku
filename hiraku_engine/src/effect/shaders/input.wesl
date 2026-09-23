import hiraku_sprite3d::sampling::decode_color;

// Effect ABI, independent of material bindings and camera targets.
struct EffectInput {
    uv: vec2<f32>,
    local_uv: vec2<f32>,
    low: vec2<f32>,
    high: vec2<f32>,
    sampling: u32,
    premultiplied: bool,
}

fn sample_premultiplied(image: texture_2d<f32>, image_sampler: sampler,
    input: EffectInput, uv: vec2<f32>) -> vec4<f32> {
    let c = decode_color(textureSampleLevel(image, image_sampler, clamp(uv, input.low, input.high), 0.0), input.sampling);
    return vec4<f32>(select(c.rgb * c.a, c.rgb, input.premultiplied), c.a);
}
fn straight(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb / max(color.a, 0.000001), color.a);
}
