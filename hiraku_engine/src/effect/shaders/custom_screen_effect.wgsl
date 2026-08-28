#import bevy_pbr::forward_io::VertexOutput

struct CustomScreenEffectMaterial {
    progress: f32,
    duration: f32,
    time: f32,
    mode: f32,
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: CustomScreenEffectMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var source_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var source_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var target_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var target_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var rule_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var rule_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var aux0_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var aux0_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(9) var aux1_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(10) var aux1_sampler: sampler;

fn mode_crossfade(uv: vec2<f32>) -> vec4<f32> {
    let source = textureSample(source_texture, source_sampler, uv);
    let target = textureSample(target_texture, target_sampler, uv);
    return mix(source, target, clamp(material.progress, 0.0, 1.0));
}

fn mode_rule(uv: vec2<f32>) -> vec4<f32> {
    let source = textureSample(source_texture, source_sampler, uv);
    let target = textureSample(target_texture, target_sampler, uv);
    let rule = textureSample(rule_texture, rule_sampler, uv).r;
    let vague = max(material.p0.x, 0.0001);
    let edge = clamp((material.progress - rule) / vague + 0.5, 0.0, 1.0);
    return mix(source, target, edge);
}

fn mode_wave(uv: vec2<f32>) -> vec4<f32> {
    let amplitude = material.p0.x;
    let frequency = max(material.p0.y, 0.0001);
    let speed = material.p0.z;
    let mix_target = clamp(material.p0.w, 0.0, 1.0);
    let wave = sin((uv.y + material.time * speed) * frequency) * amplitude;
    let warped_uv = vec2(uv.x + wave, uv.y);
    let source = textureSample(source_texture, source_sampler, warped_uv);
    let target = textureSample(target_texture, target_sampler, uv);
    return mix(source, target, mix_target * clamp(material.progress, 0.0, 1.0));
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv;
    let mode = i32(material.mode + 0.5);

    if mode == 1 {
        return mode_rule(uv);
    }
    if mode == 2 {
        return mode_wave(uv);
    }

    return mode_crossfade(uv);
}
