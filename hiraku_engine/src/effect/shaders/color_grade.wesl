fn grade(color: vec4<f32>, uv: vec2<f32>, exposure: f32, saturation: f32, tint: vec4<f32>, gamma: vec4<f32>, v: vec4<f32>) -> vec4<f32> {
    var rgb = color.rgb;
    if gamma.w > 0.5 {
        // Source TVPDoGrayScale/TVPInitGammaAdjustTempData work in 8-bit
        // display space. Convert the sampled linear color to that domain,
        // preserve the original alpha, then return to linear for blending.
        let encoded = select(12.92 * rgb, 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055,
            rgb > vec3<f32>(0.0031308));
        let gray = dot(encoded, vec3<f32>(54.0, 183.0, 19.0) / 256.0);
        let adjusted = pow(vec3<f32>(clamp(gray, 0.0, 1.0)), vec3<f32>(1.0) / gamma.rgb);
        rgb = select(adjusted / 12.92, pow((adjusted + 0.055) / 1.055, vec3<f32>(2.4)),
            adjusted > vec3<f32>(0.04045));
    }
    rgb *= exp2(exposure);
    let gray = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let vignette = 1.0 - v.x * smoothstep(v.y, max(v.y + 0.0001, v.z), distance(uv, vec2<f32>(0.5)));
    return vec4<f32>(mix(vec3<f32>(gray), rgb, saturation) * vignette, color.a) * tint;
}
