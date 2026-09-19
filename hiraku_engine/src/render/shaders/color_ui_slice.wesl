import bevy_ui_render::ui_texture_slice::{UiVertexOutput, map_uvs_to_slice, sprite_texture, sprite_sampler};
import hiraku_sprite3d::sampling::decode_color;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let uv = map_uvs_to_slice(in.uv, in.target_slices, in.texture_slices, in.repeat);
    let atlas_uv = in.atlas_rect.xy + uv * (in.atlas_rect.zw - in.atlas_rect.xy);
    return in.color * decode_color(textureSample(sprite_texture, sprite_sampler, atlas_uv), u32(constants::COLOR_SAMPLING));
}
