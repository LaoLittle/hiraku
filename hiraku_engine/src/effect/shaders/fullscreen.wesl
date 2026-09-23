import hiraku_engine::effect::shaders::input::{EffectInput, sample_premultiplied};

struct VertexOutput { @builtin(position) position: vec4<f32>, }
@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(3) var ui_texture: texture_2d<f32>;

fn texture_size() -> vec2<f32> { return vec2<f32>(textureDimensions(screen_texture)); }
fn source(input: VertexOutput) -> EffectInput {
    let size = texture_size();
    let uv = input.position.xy / size;
    let low = vec2<f32>(0.5) / size;
    return EffectInput(uv, uv, low, 1.0 - low, 0u, true);
}
fn sample_source(input: EffectInput, uv: vec2<f32>) -> vec4<f32> {
    if constants::EFFECT_STAGE == 1u {
        return sample_premultiplied(ui_texture, screen_sampler, input, uv);
    }
    return sample_premultiplied(screen_texture, screen_sampler, input, uv);
}
fn finish(input: VertexOutput, value: vec4<f32>) -> vec4<f32> {
    let color = vec4<f32>(value.rgb * value.a, value.a);
    if constants::EFFECT_STAGE == 1u {
        let scene = textureSampleLevel(screen_texture, screen_sampler, input.position.xy / texture_size(), 0.0);
        return color + scene * (1.0 - color.a);
    }
    return color;
}
