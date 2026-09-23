fn grade(color: vec4<f32>, uv: vec2<f32>, exposure: f32, saturation: f32, tint: vec4<f32>, v: vec4<f32>) -> vec4<f32> {
    let rgb = color.rgb * exp2(exposure);
    let gray = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let vignette = 1.0 - v.x * smoothstep(v.y, max(v.y + 0.0001, v.z), distance(uv, vec2<f32>(0.5)));
    return vec4<f32>(mix(vec3<f32>(gray), rgb, saturation) * vignette, color.a) * tint;
}
