import hiraku::render::{Source, texelSize, sampleColor, samplePremultiplied, straightAlpha};

// One Kawase pass; additional passes must sample the previous pass's target.
// Transparent samples are averaged in premultiplied space before unpremultiplying.
fn blur(input: Source, radius: f32) -> vec4<f32> {
    if radius <= 0.001 { return sampleColor(input, input.uv); }
    let step = (radius + 0.5) * texelSize();
    let sum = samplePremultiplied(input, input.uv + step)
        + samplePremultiplied(input, input.uv - step)
        + samplePremultiplied(input, input.uv + vec2<f32>(step.x, -step.y))
        + samplePremultiplied(input, input.uv + vec2<f32>(-step.x, step.y));
    if radius >= 1.0 { return straightAlpha(sum * 0.25); }
    // Continuous fade from the unfiltered image avoids a half-texel pop at zero.
    let center = samplePremultiplied(input, input.uv);
    return straightAlpha(mix(center, sum * 0.25, radius));
}
