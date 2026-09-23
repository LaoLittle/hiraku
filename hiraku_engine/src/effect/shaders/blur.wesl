// A complete author shader, usable on a picture, scene, UI or canvas.
import constants::EFFECT_BINDING_GROUP;
import hiraku::render::{VertexOutput, FragmentOutput, source, finish};
import hiraku::effects::kawase::blur;

struct BlurUniform { radius: f32, }
@group(EFFECT_BINDING_GROUP) @binding(7) var<uniform> settings: BlurUniform;

@fragment
fn fragment(input: VertexOutput) -> FragmentOutput {
    return finish(input, blur(source(input), settings.radius));
}
