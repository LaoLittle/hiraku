import bevy_ui_render::ui::{VertexOutput, sprite_texture, sprite_sampler, enabled, TEXTURED, BORDER_ANY, draw_uinode_border, draw_uinode_background};
import hiraku_sprite3d::sampling::decode_color;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = decode_color(textureSample(sprite_texture, sprite_sampler, in.uv), u32(constants::COLOR_SAMPLING));
    let color = select(in.color, in.color * texel, enabled(in.flags, TEXTURED));
    if enabled(in.flags, BORDER_ANY) {
        return draw_uinode_border(color, in.point, in.size, in.radius_x, in.radius_y, in.border, in.flags);
    }
    return draw_uinode_background(color, in.point, in.size, in.radius_x, in.radius_y, in.border, in.flags);
}
