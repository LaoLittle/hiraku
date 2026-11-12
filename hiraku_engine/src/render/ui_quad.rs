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
    bevy::asset::embedded_asset!(app, "shaders/ui_quad.wesl");
    app.add_plugins(UiMaterialPlugin::<UiQuadMaterial>::default());
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(UiQuadKey)]
pub(crate) struct UiQuadMaterial {
    #[uniform(14)]
    pub image_uv_rect: Vec4,
    pub image_pixel_rect: Option<Rect>,
    #[uniform(15)]
    pub sampling: UVec4,
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
    pub alpha_blend: crate::ui::UiShaderAlphaBlend,
}

pub(crate) fn image_uv_rect(rect: Option<Rect>, image_size: Vec2) -> Vec4 {
    if image_size.x <= 0. || image_size.y <= 0. {
        return Vec4::ZERO;
    }
    rect.map_or(Vec4::new(0., 0., 1., 1.), |rect| {
        let min = rect.min / image_size;
        let max = rect.max / image_size;
        Vec4::new(min.x, min.y, max.x, max.y)
    })
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct UiQuadKey(
    Option<Handle<Shader>>,
    crate::ui::UiShaderBlend,
    crate::ui::UiShaderAlphaBlend,
);
impl From<&UiQuadMaterial> for UiQuadKey {
    fn from(material: &UiQuadMaterial) -> Self {
        Self(
            material.shader.clone(),
            material.blend,
            material.alpha_blend,
        )
    }
}
impl UiMaterial for UiQuadMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/render/shaders/ui_quad.wesl".into()
    }
    fn specialize(descriptor: &mut RenderPipelineDescriptor, key: UiMaterialKey<Self>) {
        if key.bind_group_data.1 == crate::ui::UiShaderBlend::Additive {
            use bevy::render::render_resource::{
                BlendComponent, BlendFactor, BlendOperation, BlendState,
            };
            let component = BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            };
            if let Some(fragment) = &mut descriptor.fragment {
                for target in fragment.targets.iter_mut().flatten() {
                    target.blend = Some(BlendState {
                        color: component,
                        alpha: component,
                    });
                }
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
        if key.bind_group_data.2 != crate::ui::UiShaderAlphaBlend::Inherit {
            if let Some(fragment) = &mut descriptor.fragment {
                for target in fragment.targets.iter_mut().flatten() {
                    let blend = target
                        .blend
                        .get_or_insert(bevy::render::render_resource::BlendState::ALPHA_BLENDING);
                    blend.alpha = alpha_blend_component(key.bind_group_data.2, blend.alpha);
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

fn alpha_blend_component(
    mode: crate::ui::UiShaderAlphaBlend,
    inherited: bevy::render::render_resource::BlendComponent,
) -> bevy::render::render_resource::BlendComponent {
    use crate::ui::UiShaderAlphaBlend;
    use bevy::render::render_resource::{BlendComponent, BlendFactor, BlendOperation};
    let (src_factor, dst_factor) = match mode {
        UiShaderAlphaBlend::Inherit => return inherited,
        UiShaderAlphaBlend::Over => (BlendFactor::One, BlendFactor::OneMinusSrcAlpha),
        UiShaderAlphaBlend::Replace => (BlendFactor::One, BlendFactor::Zero),
        UiShaderAlphaBlend::Preserve => (BlendFactor::Zero, BlendFactor::One),
        UiShaderAlphaBlend::Additive => (BlendFactor::One, BlendFactor::One),
    };
    BlendComponent {
        src_factor,
        dst_factor,
        operation: BlendOperation::Add,
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
    pub alpha_blend: crate::ui::UiShaderAlphaBlend,
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
    fn projected_image_preserves_atlas_region_without_changing_local_uvs() {
        assert_eq!(
            image_uv_rect(
                Some(Rect::from_corners(
                    Vec2::new(128., 0.),
                    Vec2::new(256., 128.)
                )),
                Vec2::splat(256.)
            ),
            Vec4::new(0.5, 0., 1., 0.5)
        );
        assert_eq!(
            image_uv_rect(None, Vec2::splat(256.)),
            Vec4::new(0., 0., 1., 1.)
        );
        assert_eq!(image_uv_rect(Some(Rect::default()), Vec2::ZERO), Vec4::ZERO);
    }

    #[test]
    fn alpha_policy_is_independent_from_color_blending() {
        use crate::ui::UiShaderAlphaBlend;
        use bevy::render::render_resource::{BlendComponent, BlendFactor, BlendOperation};
        let inherited = BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        assert_eq!(
            alpha_blend_component(UiShaderAlphaBlend::Inherit, inherited),
            inherited
        );
        let replace = alpha_blend_component(UiShaderAlphaBlend::Replace, inherited);
        assert_eq!(replace.src_factor, BlendFactor::One);
        assert_eq!(replace.dst_factor, BlendFactor::Zero);
        let preserve = alpha_blend_component(UiShaderAlphaBlend::Preserve, inherited);
        assert_eq!(preserve.src_factor, BlendFactor::Zero);
        assert_eq!(preserve.dst_factor, BlendFactor::One);
        let over = alpha_blend_component(UiShaderAlphaBlend::Over, inherited);
        assert_eq!(over.src_factor, BlendFactor::One);
        assert_eq!(over.dst_factor, BlendFactor::OneMinusSrcAlpha);
    }

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
