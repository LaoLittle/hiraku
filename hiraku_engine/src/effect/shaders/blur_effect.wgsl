struct BlurSettings {
    intensity: f32,
};

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: BlurSettings;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let sigma = settings.intensity;
    let texel = 1.0 / vec2<f32>(textureDimensions(screen_texture));
    let uv = position.xy * texel;

    if (sigma <= 0.01) {
        return textureSample(screen_texture, screen_sampler, uv);
    }

    var total_color = vec4<f32>(0.0);
    var total_weight = 0.0;

    let int_radius = i32(ceil(sigma * 3.0));
    let step_size = max(1, int_radius / 8);

    for (var x = -int_radius; x <= int_radius; x += step_size) {
        for (var y = -int_radius; y <= int_radius; y += step_size) {
            let fx = f32(x);
            let fy = f32(y);
            let dist_sq = fx * fx + fy * fy;

            let weight = exp(-dist_sq / (2.0 * sigma * sigma));

            let offset = vec2<f32>(fx, fy) * texel;
            let sample_color = textureSample(screen_texture, screen_sampler, uv + offset);

            total_color += sample_color * weight;
            total_weight += weight;
        }
    }

    return total_color / total_weight;
}
