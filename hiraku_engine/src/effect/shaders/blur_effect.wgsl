struct BlurSettings {
    radius: vec4<f32>,
};

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: BlurSettings;

const GOLDEN_ANGLE: f32 = 2.39996323; 
const SAMPLES: i32 = 24; 

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let r = settings.radius.x;
    let texel = 1.0 / vec2<f32>(textureDimensions(screen_texture));
    let uv = position.xy * texel;

    if (r <= 0.01) {
        return textureSample(screen_texture, screen_sampler, uv);
    }

    var total_color = vec4<f32>(0.0);
    var total_weight = 0.0;

    for (var i = 0; i < SAMPLES; i++) {
        let fi = f32(i);
        let dist = sqrt(fi / f32(SAMPLES)) * r; 
        let angle = fi * GOLDEN_ANGLE;
        
        let offset = vec2<f32>(cos(angle), sin(angle)) * dist * texel;
        let sample_color = textureSample(screen_texture, screen_sampler, uv + offset);

        let luminance = dot(sample_color.rgb, vec3<f32>(0.299, 0.587, 0.114));
        let bokeh_weight = 1.0 + pow(luminance, 3.0) * settings.radius.y;

        total_color += sample_color * bokeh_weight;
        total_weight += bokeh_weight;
    }

    return total_color / total_weight;
}