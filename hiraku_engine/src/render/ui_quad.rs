//! Affine projected images in the normal Bevy UI pass (including back-face UVs).
use bevy::{
    prelude::*,
    render::render_resource::{AsBindGroup, RenderPipelineDescriptor},
    shader::ShaderRef,
    ui_render::{
        UiMaterialPlugin,
        ui_material::{UiMaterial, UiMaterialKey},
    },
};

pub(crate) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/ui_quad.wgsl");
    app.add_plugins(UiMaterialPlugin::<UiQuadMaterial>::default());
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(UiQuadKey)]
pub(crate) struct UiQuadMaterial {
    #[uniform(0)]
    pub color: LinearRgba,
    /// Rows of the mapping from bounding-box UV to image UV.
    #[uniform(1)]
    pub uv_u: Vec4,
    #[uniform(2)]
    pub uv_v: Vec4,
    #[texture(3)]
    #[sampler(4)]
    pub image: Handle<Image>,
    #[uniform(5)]
    pub parameters: Vec4,
    #[uniform(6)]
    pub clock: Vec4,
    #[texture(7)]
    pub texture1: Handle<Image>,
    #[texture(8)]
    pub texture2: Handle<Image>,
    #[texture(9)]
    pub texture3: Handle<Image>,
    pub shader: Option<Handle<Shader>>,
    pub blend: crate::ui::UiShaderBlend,
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct UiQuadKey(Option<Handle<Shader>>, crate::ui::UiShaderBlend);
impl From<&UiQuadMaterial> for UiQuadKey {
    fn from(material: &UiQuadMaterial) -> Self {
        Self(material.shader.clone(), material.blend)
    }
}
impl UiMaterial for UiQuadMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/render/shaders/ui_quad.wgsl".into()
    }
    fn specialize(descriptor: &mut RenderPipelineDescriptor, key: UiMaterialKey<Self>) {
        if key.bind_group_data.1 == crate::ui::UiShaderBlend::Additive {
            use bevy::render::render_resource::{BlendComponent, BlendFactor, BlendOperation, BlendState};
            let component=BlendComponent { src_factor:BlendFactor::SrcAlpha,dst_factor:BlendFactor::One,operation:BlendOperation::Add };
            if let Some(fragment)=&mut descriptor.fragment {
                for target in fragment.targets.iter_mut().flatten() { target.blend=Some(BlendState { color:component,alpha:component }); }
            }
        }
        if key.bind_group_data.1 == crate::ui::UiShaderBlend::Multiply {
            use bevy::render::render_resource::{
                BlendComponent, BlendFactor, BlendOperation, BlendState,
            };
            if let Some(fragment) = &mut descriptor.fragment {
                for target in fragment.targets.iter_mut().flatten() {
                    target.blend = Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::Dst,
                            dst_factor: BlendFactor::Zero,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    });
                }
            }
        }
        if let Some(shader) = key.bind_group_data.0 {
            if let Some(fragment) = &mut descriptor.fragment {
                fragment.shader = shader;
                fragment.entry_point = Some("main".into());
            }
        }
    }
}

#[derive(Component)]
pub(crate) struct UiShaderAssets(pub Vec<Handle<Shader>>);

#[derive(Component)]
pub(crate) struct UiImageAssets(pub Vec<Handle<Image>>);

#[derive(Component)]
pub(crate) struct UiShaderSource {
    pub shader: Handle<Shader>,
    pub textures: Vec<Handle<Image>>,
    pub keys: Vec<crate::ui::UiShaderKeyframe>,
    pub blend: crate::ui::UiShaderBlend,
}
pub(crate) fn valid_shader_keys(keys: &[crate::ui::UiShaderKeyframe]) -> bool {
    use crate::ui::UiShaderKeyframe::At;
    !keys.is_empty()
        && keys
            .iter()
            .all(|&At(t, x, y, z, w)| t >= 0. && [t, x, y, z, w].iter().all(|v| v.is_finite()))
        && keys.windows(2).all(|pair| {
            let At(a, ..) = pair[0];
            let At(b, ..) = pair[1];
            a < b
        })
}
pub(crate) fn shader_parameters(keys: &[crate::ui::UiShaderKeyframe], time: f64) -> Vec4 {
    use crate::ui::UiShaderKeyframe::At;
    let index = keys
        .partition_point(|&At(t, ..)| t <= time)
        .saturating_sub(1);
    let At(t, x, y, z, w) = keys[index];
    let a = Vec4::new(x as f32, y as f32, z as f32, w as f32);
    if let Some(&At(t1, x, y, z, w)) = keys.get(index + 1) {
        a.lerp(
            Vec4::new(x as f32, y as f32, z as f32, w as f32),
            ((time - t) / (t1 - t)).clamp(0., 1.) as f32,
        )
    } else {
        a
    }
}

/// Degenerate edge-on quads are invisible; do not divide by a near-zero area.
pub(crate) fn projection(origin: Vec2, u: Vec2, v: Vec2) -> (Rect, Vec4, Vec4) {
    let points = [origin, origin + u, origin + v, origin + u + v];
    let min = points
        .into_iter()
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = points
        .into_iter()
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    let bounds = Rect::from_corners(min, max);
    let det = u.perp_dot(v);
    if det.abs() < 1e-5 {
        return (bounds, Vec4::new(0., 0., -1., 0.), Vec4::ZERO);
    }
    let inverse = Mat2::from_cols(u, v).inverse();
    let offset = inverse * (min - origin);
    let matrix = inverse * Mat2::from_diagonal(bounds.size());
    (
        bounds,
        Vec4::new(matrix.x_axis.x, matrix.y_axis.x, offset.x, 0.),
        Vec4::new(matrix.x_axis.y, matrix.y_axis.y, offset.y, 0.),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shader_parameter_tracks_validate_and_interpolate() {
        use crate::ui::UiShaderKeyframe::At;
        let keys = [At(0., 0., 2., 4., 6.), At(2., 2., 4., 6., 8.)];
        assert!(valid_shader_keys(&keys));
        assert_eq!(shader_parameters(&keys, 1.), Vec4::new(1., 3., 5., 7.));
        assert_eq!(shader_parameters(&keys, 9.), Vec4::new(2., 4., 6., 8.));
        assert!(!valid_shader_keys(&[]));
        assert!(!valid_shader_keys(&[keys[1], keys[0]]));
        assert!(!valid_shader_keys(&[At(0., f64::NAN, 0., 0., 0.)]));
    }
    #[test]
    fn projected_shader_is_valid_wgsl() {
        let source = include_str!("shaders/ui_quad.wgsl");
        let source=source.replace("#import bevy_ui::ui_vertex_output::UiVertexOutput", "struct UiVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, }");
        let module = naga::front::wgsl::parse_str(&source).expect("projected UI shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("projected UI shader validates");
    }
    #[test]
    fn projected_uv_preserves_shear_and_reflection() {
        for (u, v) in [
            (Vec2::new(30., 5.), Vec2::new(8., 20.)),
            (Vec2::new(-30., 5.), Vec2::new(8., 20.)),
        ] {
            let origin = Vec2::new(80., 90.);
            let (bounds, a, b) = projection(origin, u, v);
            for uv in [Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE] {
                let q = (origin + u * uv.x + v * uv.y - bounds.min) / bounds.size();
                let actual = Vec2::new(a.x * q.x + a.y * q.y + a.z, b.x * q.x + b.y * q.y + b.z);
                assert!(actual.abs_diff_eq(uv, 1e-5));
            }
        }
        let (_, a, _) = projection(Vec2::ZERO, Vec2::ZERO, Vec2::Y);
        assert_eq!(a.z, -1.);
    }
}
