#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var y_texture: texture_2d<f32>;
@group(1) @binding(1) var u_texture: texture_2d<f32>;
@group(1) @binding(2) var v_texture: texture_2d<f32>;
@group(1) @binding(3) var main_sampler: sampler;
struct YuvColorTransform {
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
    transfer: u32,
    _padding: vec3<u32>,
};

@group(1) @binding(4) var<uniform> color_transform: YuvColorTransform;

fn to_linear_channel(value: f32, transfer: u32) -> f32 {
    let x = clamp(value, 0.0, 1.0);
    if transfer == 0u {
        return x;
    }
    if transfer == 2u {
        return select(
            pow((x + 0.055) / 1.055, 2.4),
            x / 12.92,
            x <= 0.04045,
        );
    }
    if transfer == 3u {
        return pow(x, 2.2);
    }
    if transfer == 4u {
        return pow(x, 2.8);
    }
    return select(
        pow((x + 0.099) / 1.099, 1.0 / 0.45),
        x / 4.5,
        x < 0.081,
    );
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let yuv = vec4<f32>(
        textureSample(y_texture, main_sampler, in.uv).r,
        textureSample(u_texture, main_sampler, in.uv).r,
        textureSample(v_texture, main_sampler, in.uv).r,
        1.0,
    );
    let encoded_rgb = vec3<f32>(
        dot(color_transform.row_r, yuv),
        dot(color_transform.row_g, yuv),
        dot(color_transform.row_b, yuv),
    );
    let linear_rgb = vec3<f32>(
        to_linear_channel(encoded_rgb.r, color_transform.transfer),
        to_linear_channel(encoded_rgb.g, color_transform.transfer),
        to_linear_channel(encoded_rgb.b, color_transform.transfer),
    );
    return vec4<f32>(linear_rgb, 1.0);
}
