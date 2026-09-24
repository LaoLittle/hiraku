// Engine default, implemented with exactly the same API as author shaders.
import constants::EFFECT_BINDING_GROUP;
import hiraku::render::{VertexOutput, FragmentOutput, source, sampleColor, finish};
import hiraku::effects::color_grade::grade;

struct StandardUniform {
    blur_radius: f32,
    zoom: f32,
    exposure: f32,
    saturation: f32,
    tint: vec4<f32>,
    grayscale_gamma: vec4<f32>,
    vignette: vec4<f32>,
    view_transform: vec4<f32>,
}
@group(EFFECT_BINDING_GROUP) @binding(7) var<uniform> settings: StandardUniform;

@fragment
fn fragment(input: VertexOutput) -> FragmentOutput {
    var sample = source(input);
    let zoom = max(settings.zoom, 0.01);
    let aspect = select(settings.view_transform.w, 1.0, settings.view_transform.w <= 0.0);
    let p = (sample.local_uv - 0.5) * vec2<f32>(aspect, 1.0) / zoom;
    let c = cos(settings.view_transform.z);
    let s = sin(settings.view_transform.z);
    let local_uv = vec2<f32>(c * p.x + s * p.y, -s * p.x + c * p.y)
        / vec2<f32>(aspect, 1.0) + 0.5 + settings.view_transform.xy;
    // Moving a composed view exposes transparency, not stretched edge pixels.
    if settings.view_transform.w > 0.0 && (any(local_uv < vec2<f32>(0.0)) || any(local_uv > vec2<f32>(1.0))) {
        return finish(input, vec4<f32>(0.0));
    }
    sample.uv += (local_uv - sample.local_uv) * (sample.high - sample.low);
    sample.local_uv = local_uv;
    return finish(input, grade(sampleColor(sample, sample.uv), local_uv,
        settings.exposure, settings.saturation, settings.tint, settings.grayscale_gamma, settings.vignette));
}
