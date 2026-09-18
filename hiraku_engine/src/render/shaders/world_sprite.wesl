import bevy_pbr::render::forward_io::VertexOutput;

struct WorldSpriteMaterial {
    clip_plane: vec4<f32>,
    slice_borders: vec4<f32>,
    slice_size: vec4<f32>,
    clip_bounds: vec4<f32>,
    clip_axes: vec4<f32>,
    effects: vec4<f32>,
    dissolve: vec4<f32>,
    tint: vec4<f32>,
    // `[left, top, width, height]`; a zero size selects the full image.
    rect: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: WorldSpriteMaterial;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var color_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var color_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var dissolve_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var dissolve_sampler: sampler;

// Keep corners at source-pixel size, shrinking proportionally when the target
// is smaller than both borders combined. Only the center strip stretches.
fn slice_axis(uv: f32, source: f32, destination: f32, before: f32, after: f32) -> f32 {
    let borders = vec2<f32>(before, after) * min(1.0, source / max(before + after, 0.0001));
    let screen = borders * min(1.0, destination / max(borders.x + borders.y, 0.0001));
    let p = uv * destination;
    if p < screen.x { return p / max(screen.x, 0.0001) * borders.x / source; }
    if p > destination - screen.y { return 1.0 - (destination - p) / max(screen.y, 0.0001) * borders.y / source; }
    return (borders.x + (p - screen.x) / max(destination - screen.x - screen.y, 0.0001) * (source - borders.x - borders.y)) / source;
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    if material.clip_plane.w > 0.0 && dot(material.clip_plane.xy, mesh.world_position.xy) > material.clip_plane.z {
        discard;
    }
    let texture_size = vec2<f32>(textureDimensions(color_texture));
    let full_image = material.rect.z <= 0.0 || material.rect.w <= 0.0;
    let source_size = select(material.rect.zw, texture_size, full_image);
    var local_uv = mesh.uv;
    if material.slice_size.z > 0.0 {
        local_uv = vec2<f32>(
            slice_axis(mesh.uv.x, source_size.x, material.slice_size.x, material.slice_borders.x, material.slice_borders.z),
            slice_axis(mesh.uv.y, source_size.y, material.slice_size.y, material.slice_borders.y, material.slice_borders.w),
        );
    }
    let uv = select(
        (material.rect.xy + clamp(local_uv * source_size, vec2<f32>(0.5), source_size - vec2<f32>(0.5))) / texture_size,
        local_uv,
        full_image,
    );
    var sampled = textureSample(color_texture, color_sampler, uv);
    if material.effects.y > 0.0 {
        let cell = vec2<u32>(floor(clamp(local_uv, vec2<f32>(0.0), vec2<f32>(0.999999)) * material.effects.zw));
        var hash = (cell.x * 1597334677u) ^ (cell.y * 3812015801u) ^ (u32(material.effects.y) * 2798796415u);
        hash = (hash ^ (hash >> 16u)) * 2246822519u;
        hash = (hash ^ (hash >> 13u)) * 3266489917u;
        hash = hash ^ (hash >> 16u);
        let gray = f32(hash & 255u) / 255.0;
        // The original noise raster is display-referred, like an sRGB texture.
        let linear = select(pow((gray + 0.055) / 1.055, 2.4), gray / 12.92, gray <= 0.04045);
        sampled = vec4<f32>(vec3<f32>(linear), 1.0);
    }
    if material.effects.x > 0.0 {
        let origin = select(material.rect.xy, vec2<f32>(0.0), full_image);
        let size = select(material.rect.zw, texture_size, full_image);
        let low = (origin + vec2<f32>(0.5)) / texture_size;
        let high = (origin + size - vec2<f32>(0.5)) / texture_size;
        let weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
        var sum = vec4<f32>(0.0);
        for (var y = 0u; y < 5u; y += 1u) {
            for (var x = 0u; x < 5u; x += 1u) {
                let offset = (vec2<f32>(f32(x), f32(y)) - vec2<f32>(2.0)) * material.effects.x * 0.5 / texture_size;
                let tap = textureSampleLevel(color_texture, color_sampler, clamp(uv + offset, low, high), 0.0);
                // Filter premultiplied color to avoid dark fringes at transparent edges.
                sum += vec4<f32>(tap.rgb * tap.a, tap.a) * weights[x] * weights[y];
            }
        }
        let alpha = sum.a / 256.0;
        sampled = vec4<f32>(sum.rgb / max(sum.a, 0.000001), alpha);
    }
    var color = sampled * material.tint;
    if material.dissolve.x != 0.0 {
        // Screen coordinates keep a full-canvas mask independent of the large
        // scene-covering quad and of camera zoom/projection.
        let threshold = textureSample(dissolve_texture, dissolve_sampler, mesh.position.xy / material.dissolve.zw).r;
        // Both entering and leaving traverse the mask from low to high.
        let reversed = material.dissolve.x < 0.0;
        let progress = select(material.tint.a, 1.0 - material.tint.a, reversed);
        let softness = material.dissolve.y;
        var coverage = step(threshold, progress);
        if softness > 0.0 {
            coverage = smoothstep(threshold, threshold + softness, progress * (1.0 + softness));
        }
        // Ensure exact endpoints even when the mask contains black or white.
        coverage = select(coverage, 0.0, progress <= 0.0);
        coverage = select(coverage, 1.0, progress >= 1.0);
        color.a = select(coverage, 1.0 - coverage, reversed);
    }
    let delta = mesh.world_position.xy - material.clip_bounds.xy;
    let local = vec2<f32>(dot(delta, material.clip_axes.xy), dot(delta, vec2<f32>(-material.clip_axes.y, material.clip_axes.x)));
    let outside = material.clip_axes.z > 0.0 && any(abs(local) > material.clip_bounds.zw);
    if outside || color.a <= 0.0001 {
        discard;
    }
    return color;
}
