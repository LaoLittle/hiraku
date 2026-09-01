use bevy::{
    math::{UVec3, Vec4},
    render::render_resource::ShaderType,
};

pub(crate) const TRANSFER_LINEAR: u32 = 0;
pub(crate) const TRANSFER_BT709: u32 = 1;
pub(crate) const TRANSFER_SRGB: u32 = 2;
pub(crate) const TRANSFER_GAMMA_22: u32 = 3;
pub(crate) const TRANSFER_GAMMA_28: u32 = 4;

/// A precombined YUV-range and YUV-to-RGB affine transform.
///
/// Each row includes its offset in `w`, allowing the shader to evaluate a
/// complete output channel with one four-component dot product.
#[derive(Clone, Copy, Debug, ShaderType)]
pub(crate) struct YuvColorTransform {
    pub row_r: Vec4,
    pub row_g: Vec4,
    pub row_b: Vec4,
    pub transfer: u32,
    pub _padding: UVec3,
}

impl YuvColorTransform {
    pub fn from_luma_coefficients(kr: f32, kb: f32, limited_range: bool, transfer: u32) -> Self {
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
            transfer,
            _padding: UVec3::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_and_white_map_to_display_endpoints() {
        let transform =
            YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true, TRANSFER_BT709);
        let black = Vec4::new(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        let white = Vec4::new(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        for row in [transform.row_r, transform.row_g, transform.row_b] {
            assert!(row.dot(black).abs() < 1.0e-5);
            assert!((row.dot(white) - 1.0).abs() < 1.0e-5);
        }
    }
}
