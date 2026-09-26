@if(WORLD_VIDEO) import bevy_pbr::render::forward_io::VertexOutput;
@if(!WORLD_VIDEO) import bevy_ui_render::ui_vertex_output::UiVertexOutput;

@if(WORLD_VIDEO) const VIDEO_GROUP: u32 = constants::MATERIAL_BIND_GROUP;
@else const VIDEO_GROUP: u32 = 1u;

@if(FORMAT_I420) {

@group(VIDEO_GROUP) @binding(0) var y_texture: texture_2d<f32>;
@group(VIDEO_GROUP) @binding(1) var u_texture: texture_2d<f32>;
@group(VIDEO_GROUP) @binding(2) var v_texture: texture_2d<f32>;

}

@elif(FORMAT_NV12) {

@group(VIDEO_GROUP) @binding(0) var y_texture: texture_2d<f32>;
@group(VIDEO_GROUP) @binding(1) var uv_texture: texture_2d<f32>;
@group(VIDEO_GROUP) @binding(2) var _dummy: texture_2d<f32>;

}

@group(VIDEO_GROUP) @binding(3) var main_sampler: sampler;

struct YuvColorTransform {
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
    luma: vec2<f32>,
    gamut_r: vec4<f32>,
    gamut_g: vec4<f32>,
    gamut_b: vec4<f32>,
};

@group(VIDEO_GROUP) @binding(4) var<uniform> color_transform: YuvColorTransform;

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

    // ST 2084 absolute luminance, in units of 203-nit diffuse white.
    @elif(TRANSFER_PQ) {
        let p = pow(x, vec3<f32>(1.0 / 78.84375));
        return pow(max(p - vec3<f32>(0.8359375), vec3<f32>(0.0)) /
            max(vec3<f32>(18.8515625) - 18.6875 * p, vec3<f32>(0.000001)),
            vec3<f32>(1.0 / 0.1593017578125)) * (10000.0 / 203.0);
    }
    // BT.2100 HLG inverse OETF followed by the reference 1000-nit OOTF.
    @elif(TRANSFER_HLG) {
        let scene = select((exp((x - vec3<f32>(0.55991073)) / 0.17883277) + 0.28466892) / 12.0,
            x * x / 3.0, x <= vec3<f32>(0.5));
        let y = dot(scene, vec3<f32>(0.2627, 0.6780, 0.0593));
        return scene * pow(max(y, 0.0), 0.2) * (1000.0 / 203.0);
    }

    // Conventional BT.709/BT.601/BT.2020 SDR is displayed using BT.1886.
    // For an ideal black level its EOTF is a pure 2.4 power function. The
    // piecewise 4.5 curve belongs to the BT.709 camera OETF and is not the
    // display transfer function.
    @else
    return pow(x, vec3<f32>(2.4));
}

fn sample_yuv(uv: vec2<f32>) -> vec3<f32> {
    @if(FORMAT_I420) {
        return vec3<f32>(
            textureSample(y_texture, main_sampler, uv).r,
            textureSample(u_texture, main_sampler, uv).r,
            textureSample(v_texture, main_sampler, uv).r,
        );
    }
    @elif(FORMAT_NV12) {
        let y = textureSample(y_texture, main_sampler, uv).r;
        let chroma = textureSample(uv_texture, main_sampler, uv).rg;
    
        return vec3<f32>(y, chroma.r, chroma.g);
    }
}

@group(VIDEO_GROUP) @binding(5) var<uniform> opacity: f32;

fn video_color(uv: vec2<f32>) -> vec4<f32> {
    var color_uv = uv;
    var alpha = 1.0;
    // Reserve a chroma texel at the split to prevent bilinear sampling from
    // blending the color half with the grayscale matte half.
    let margin = vec2<f32>(1.0) / vec2<f32>(textureDimensions(y_texture));
    @if(ALPHA_VERTICAL) {
        color_uv = clamp(uv * vec2<f32>(1.0, 0.5), margin, vec2<f32>(1.0, 0.5) - margin);
        let alpha_uv = clamp(uv * vec2<f32>(1.0, 0.5) + vec2<f32>(0.0, 0.5), vec2<f32>(0.0, 0.5) + margin, vec2<f32>(1.0) - margin);
        alpha = textureSample(y_texture, main_sampler, alpha_uv).r;
    }
    @elif(ALPHA_HORIZONTAL) {
        color_uv = clamp(uv * vec2<f32>(0.5, 1.0), margin, vec2<f32>(0.5, 1.0) - margin);
        let alpha_uv = clamp(uv * vec2<f32>(0.5, 1.0) + vec2<f32>(0.5, 0.0), vec2<f32>(0.5, 0.0) + margin, vec2<f32>(1.0) - margin);
        alpha = textureSample(y_texture, main_sampler, alpha_uv).r;
    }
    // Hardware may convert full-range encoded Y to limited-range output.
    // Undo that range conversion, but never apply a color EOTF to coverage.
    @if(!FORMAT_RGBA) {
        @if(ALPHA_VERTICAL || ALPHA_HORIZONTAL) {
            alpha = alpha * color_transform.luma.x + color_transform.luma.y;
        }
    }
    @if(FORMAT_RGBA) {
        let rgb = textureSample(y_texture, main_sampler, color_uv).rgb;
        return vec4<f32>(to_linear_vec3(rgb), clamp(alpha, 0.0, 1.0) * opacity);
    }
    @else {
    let yuv = vec4<f32>(
        sample_yuv(color_uv),
        1.0,
    );

    let encoded_rgb = vec3<f32>(
        dot(color_transform.row_r, yuv),
        dot(color_transform.row_g, yuv),
        dot(color_transform.row_b, yuv),
    );

    let source = vec4<f32>(to_linear_vec3(encoded_rgb), 1.0);
    var linear_rgb = max(vec3<f32>(dot(color_transform.gamut_r, source),
        dot(color_transform.gamut_g, source), dot(color_transform.gamut_b, source)), vec3<f32>(0.0));
    // Current presentation targets are SDR. Preserve HDR in codec frames and
    // perform luminance-preserving tone mapping only at this presentation edge.
    @if(TRANSFER_PQ || TRANSFER_HLG) {
        let luminance = dot(linear_rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        linear_rgb = linear_rgb / (1.0 + luminance);
    }

    return vec4<f32>(linear_rgb, clamp(alpha, 0.0, 1.0) * opacity);
    }
}

@if(WORLD_VIDEO)
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return video_color(in.uv);
}
@else
@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    return video_color(in.uv);
}
