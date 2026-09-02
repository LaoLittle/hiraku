use bevy::{math::Vec4, render::render_resource::ShaderType};

/// BT.1886 display EOTF for conventional SDR television content.
///
/// With an ideal black level this is a pure 2.4 power function. It must not
/// be replaced with the inverse BT.709 camera OETF: the two curves describe
/// different sides of the imaging chain.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        dead_code,
        reason = "the WebCodecs decoder backend is not implemented yet"
    )
)]
pub(crate) enum TransferFunction {
    Linear,
    Bt1886,
    Srgb,
    Gamma22,
    Gamma28,
}

/// A precombined YUV-range and YUV-to-RGB affine transform.
///
/// Each row includes its offset in `w`, allowing the shader to evaluate a
/// complete output channel with one four-component dot product.
#[derive(Clone, Copy, Debug, ShaderType)]
pub(crate) struct YuvColorTransform {
    pub row_r: Vec4,
    pub row_g: Vec4,
    pub row_b: Vec4,
}

impl YuvColorTransform {
    pub fn from_luma_coefficients(kr: f32, kb: f32, limited_range: bool) -> Self {
        let kg = 1.0 - kr - kb;
        let red_v = 2.0 * (1.0 - kr);
        let blue_u = 2.0 * (1.0 - kb);
        let green_u = -2.0 * kb * (1.0 - kb) / kg;
        let green_v = -2.0 * kr * (1.0 - kr) / kg;
        let (y_offset, y_scale, chroma_offset, chroma_scale) = if limited_range {
            (16.0 / 255.0, 255.0 / 219.0, 128.0 / 255.0, 255.0 / 224.0)
        } else {
            (0.0, 1.0, 0.5, 1.0)
        };
        let offset = |u: f32, v: f32| -y_scale * y_offset - chroma_scale * chroma_offset * (u + v);
        Self {
            row_r: Vec4::new(y_scale, 0.0, red_v * chroma_scale, offset(0.0, red_v)),
            row_g: Vec4::new(
                y_scale,
                green_u * chroma_scale,
                green_v * chroma_scale,
                offset(green_u, green_v),
            ),
            row_b: Vec4::new(y_scale, blue_u * chroma_scale, 0.0, offset(blue_u, 0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_and_white_map_to_display_endpoints() {
        let transform = YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true);
        let black = Vec4::new(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        let white = Vec4::new(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        for row in [transform.row_r, transform.row_g, transform.row_b] {
            assert!(row.dot(black).abs() < 1.0e-5);
            assert!((row.dot(white) - 1.0).abs() < 1.0e-5);
        }
    }
}
