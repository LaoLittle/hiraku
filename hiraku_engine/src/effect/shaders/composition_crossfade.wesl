import bevy_pbr::render::forward_io::VertexOutput;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var from_image: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var image_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var to_image: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<uniform> progress: vec4<f32>;

// Inputs are complete linear premultiplied compositions. Mix RGBA together;
// fading two source-over quads would leak the backing layer at intermediate t.
@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let previous_color = textureSample(from_image, image_sampler, input.uv);
    let next_color = textureSample(to_image, image_sampler, input.uv);
    return mix(previous_color, next_color, clamp(progress.x, 0.0, 1.0));
}
