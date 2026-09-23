// Public author API. Target selection is a pipeline specialization, never a
// source rewrite or replacement of a shared module at runtime.
@if(HIRAKU_MATERIAL)
import hiraku_engine::effect::shaders::material as target;
@if(!HIRAKU_MATERIAL)
import hiraku_engine::effect::shaders::fullscreen as target;
import hiraku_engine::effect::shaders::input::{EffectInput, straight};

alias VertexOutput = target::VertexOutput;
alias Source = EffectInput;
struct FragmentOutput { @location(0) color: vec4<f32>, }

// Source UVs already include atlas/nine-slice mapping. Offsets are in source
// texture texels; sampling clamps to the active atlas rectangle.
fn source(input: VertexOutput) -> Source { return target::source(input); }
fn texelSize() -> vec2<f32> { return 1.0 / target::texture_size(); }
fn samplePremultiplied(input: Source, uv: vec2<f32>) -> vec4<f32> {
    return target::sample_source(input, uv);
}
fn straightAlpha(color: vec4<f32>) -> vec4<f32> { return straight(color); }
fn sampleColor(input: Source, uv: vec2<f32>) -> vec4<f32> {
    return straightAlpha(samplePremultiplied(input, uv));
}
// Effects return linear, straight-alpha colors. The target owns tint, clipping,
// rule transitions, premultiplication and UI-over-scene composition.
fn finish(input: VertexOutput, color: vec4<f32>) -> FragmentOutput {
    return FragmentOutput(target::finish(input, color));
}
