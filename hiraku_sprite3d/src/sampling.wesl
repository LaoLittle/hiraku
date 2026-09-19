fn decode_color(sampled: vec4<f32>, mode: u32) -> vec4<f32> {
    if mode == 0u { return sampled; }
    if mode == 5u {
        let rgb = select(1.055 * pow(max(sampled.rgb, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055, sampled.rgb * 12.92, sampled.rgb <= vec3(0.0031308));
        return vec4(rgb, sampled.a);
    }
    var gray = sampled.r;
    if mode <= 2u {
        gray = select(pow((gray + 0.055) / 1.055, 2.4), gray / 12.92, gray <= 0.04045);
    }
    return vec4(vec3(gray), select(1.0, sampled.g, mode == 2u || mode == 4u));
}
