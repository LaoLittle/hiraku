use bevy::{math::Vec4, render::render_resource::ShaderType};

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

// GPU layout adapter only; color conversion math belongs to hiraku-media.
impl From<hiraku_media::YuvColorTransform> for YuvColorTransform {
    fn from(value: hiraku_media::YuvColorTransform) -> Self {
        Self {
            row_r: Vec4::from_array(value.row_r),
            row_g: Vec4::from_array(value.row_g),
            row_b: Vec4::from_array(value.row_b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_and_white_map_to_display_endpoints() {
        let transform = YuvColorTransform::from(
            hiraku_media::YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true),
        );
        let black = Vec4::new(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        let white = Vec4::new(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        for row in [transform.row_r, transform.row_g, transform.row_b] {
            assert!(row.dot(black).abs() < 1.0e-5);
            assert!((row.dot(white) - 1.0).abs() < 1.0e-5);
        }
    }
}
