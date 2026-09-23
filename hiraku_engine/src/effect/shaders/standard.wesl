// Engine default, implemented with exactly the same API as author shaders.
import constants::EFFECT_BINDING_GROUP;
import hiraku::render::{VertexOutput, FragmentOutput, source, finish};
import hiraku::effects::kawase::blur;
import hiraku::effects::color_grade::grade;

struct StandardUniform {
    blur_radius: f32,
    zoom: f32,
    exposure: f32,
    saturation: f32,
    tint: vec4<f32>,
    vignette: vec4<f32>,
}
@group(EFFECT_BINDING_GROUP) @binding(7) var<uniform> settings: StandardUniform;

@fragment
fn fragment(input: VertexOutput) -> FragmentOutput {
    var sample = source(input);
    let zoom = max(settings.zoom, 0.01);
    let local_uv = (sample.local_uv - 0.5) / zoom + 0.5;
    sample.uv += (local_uv - sample.local_uv) * (sample.high - sample.low);
    sample.local_uv = local_uv;
    return finish(input, grade(blur(sample, settings.blur_radius), local_uv,
        settings.exposure, settings.saturation, settings.tint, settings.vignette));
}
