#import bevy_sprite::mesh2d_vertex_output::VertexOutput

struct AlphaMaskMaterial {
    tint: vec4<f32>,
    main_rect: vec4<f32>,
    mask_rect: vec4<f32>,
    offsets: vec4<f32>,
    opacity: f32,
    mask_enabled: f32,
    _padding: vec2<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: AlphaMaskMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var mask_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var mask_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let color_size = vec2<f32>(textureDimensions(color_texture));
    let color_uv = (material.main_rect.xy + mesh.uv * material.main_rect.zw) / color_size;
    var color = textureSample(color_texture, color_sampler, color_uv) * material.tint;

    let main_local = vec2(
        (mesh.uv.x - 0.5) * material.main_rect.z,
        (0.5 - mesh.uv.y) * material.main_rect.w,
    );
    let actor_local = main_local + material.offsets.xy;
    let mask_delta = actor_local - material.offsets.zw;
    let mask_local = vec2(
        mask_delta.x / material.mask_rect.z + 0.5,
        0.5 - mask_delta.y / material.mask_rect.w,
    );

    let clamped_local = clamp(mask_local, vec2(0.0), vec2(1.0));
    let is_in_bounds = select(0.0, 1.0, all(mask_local == clamped_local));

    let mask_size = vec2<f32>(textureDimensions(mask_texture));
    let mask_uv = (material.mask_rect.xy + clamped_local * material.mask_rect.zw) / mask_size;
    let raw_mask_alpha = textureSample(mask_texture, mask_sampler, mask_uv).a;

    let mask_pass = select(0.0, 1.0, (raw_mask_alpha * is_in_bounds) > (1.0 / 255.0));

    let effective_mask_factor = mix(1.0, mask_pass, material.mask_enabled);
    color.a *= effective_mask_factor * material.opacity;

    if color.a <= 0.0001 {
        discard;
    }

    return color;
}
