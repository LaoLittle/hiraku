import bevy_ui_render::ui_vertex_output::UiVertexOutput;

@if(FORMAT_I420) {

@group(1) @binding(0) var y_texture: texture_2d<f32>;
@group(1) @binding(1) var u_texture: texture_2d<f32>;
@group(1) @binding(2) var v_texture: texture_2d<f32>;

}

@elif(FORMAT_NV12) {

@group(1) @binding(0) var y_texture: texture_2d<f32>;
@group(1) @binding(1) var uv_texture: texture_2d<f32>;
@group(1) @binding(2) var _dummy: texture_2d<f32>;

}

@group(1) @binding(3) var main_sampler: sampler;

struct YuvColorTransform {
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
};

@group(1) @binding(4) var<uniform> color_transform: YuvColorTransform;

fn to_linear_vec3(val: vec3<f32>) -> vec3<f32> {
    let x = clamp(val, vec3<f32>(0.0), vec3<f32>(1.0));
    @if(TRANSFER_LINEAR)
    return x;

    @elif(TRANSFER_SRGB) {
        let lo = x / 12.92;
        let hi = pow((x + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
        return select(hi, lo, x <= vec3<f32>(0.04045));
    }
    
    @elif(TRANSFER_GAMMA_22)
    return pow(x, vec3<f32>(2.2));

    @elif(TRANSFER_GAMMA_28)
    return pow(x, vec3<f32>(2.8));

    // Conventional BT.709/BT.601/BT.2020 SDR is displayed using BT.1886.
    // For an ideal black level its EOTF is a pure 2.4 power function. The
    // piecewise 4.5 curve belongs to the BT.709 camera OETF and is not the
    // display transfer function.
    @else
    return pow(x, vec3<f32>(2.4));
}

fn sample_yuv(uv: vec2<f32>) -> vec3<f32> {
    @if(FORMAT_I420)
    return vec3<f32>(
        textureSample(y_texture, main_sampler, uv).r,
        textureSample(u_texture, main_sampler, uv).r,
        textureSample(v_texture, main_sampler, uv).r,
    );
    @elif(FORMAT_NV12)
    let y = textureSample(y_texture, main_sampler, uv).r;
    let chroma = textureSample(uv_texture, main_sampler, uv).rg;

    return vec3<f32>(y, chroma.r, chroma.g);
}

@group(1) @binding(5) var<uniform> opacity: f32;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let yuv = vec4<f32>(
        sample_yuv(in.uv),
        1.0,
    );

    let encoded_rgb = vec3<f32>(
        dot(color_transform.row_r, yuv),
        dot(color_transform.row_g, yuv),
        dot(color_transform.row_b, yuv),
    );

    let linear_rgb = to_linear_vec3(encoded_rgb);

    return vec4<f32>(linear_rgb, opacity);
}
