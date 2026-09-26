use bevy::{
    math::{Vec2, Vec4},
    render::render_resource::ShaderType,
};

/// A precombined YUV-range and YUV-to-RGB affine transform.
///
/// Each row includes its offset in `w`, allowing the shader to evaluate a
/// complete output channel with one four-component dot product.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct YuvColorTransform {
    pub row_r: Vec4,
    pub row_g: Vec4,
    pub row_b: Vec4,
    pub luma: Vec2,
    pub gamut_r: Vec4,
    pub gamut_g: Vec4,
    pub gamut_b: Vec4,
}

// GPU layout adapter only; color conversion math belongs to hiraku-codec.
impl From<hiraku_codec::YuvColorTransform> for YuvColorTransform {
    fn from(value: hiraku_codec::YuvColorTransform) -> Self {
        Self {
            row_r: Vec4::from_array(value.row_r),
            row_g: Vec4::from_array(value.row_g),
            row_b: Vec4::from_array(value.row_b),
            luma: Vec2::from_array(value.luma),
            gamut_r: Vec4::from_array(value.gamut[0]),
            gamut_g: Vec4::from_array(value.gamut[1]),
            gamut_b: Vec4::from_array(value.gamut[2]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_alpha_survives_decoder_range_conversion_without_gamma() {
        for limited in [false, true] {
            for (kr, kb) in [(0.2126, 0.0722), (0.299, 0.114)] {
                let transform = YuvColorTransform::from(
                    hiraku_codec::YuvColorTransform::from_luma_coefficients(kr, kb, limited),
                );
                for coverage in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let decoded_y = if limited {
                        (16.0 + 219.0 * coverage) / 255.0
                    } else {
                        coverage
                    };
                    let alpha = decoded_y * transform.luma.x + transform.luma.y;
                    assert!((alpha - coverage).abs() < 1.0e-6);
                }
            }
        }
    }

    #[test]
    fn full_range_neutral_chroma_preserves_gray_including_black() {
        for (kr, kb) in [(0.2126, 0.0722), (0.299, 0.114), (0.2627, 0.0593)] {
            let transform = YuvColorTransform::from(
                hiraku_codec::YuvColorTransform::from_luma_coefficients(kr, kb, false),
            );
            for code in [0.0, 1.0, 16.0, 64.0, 128.0, 235.0, 255.0] {
                let sample = Vec4::new(code / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
                for row in [transform.row_r, transform.row_g, transform.row_b] {
                    assert!((row.dot(sample) - code / 255.0).abs() < 1.0e-6);
                }
            }
        }
    }

    #[test]
    fn full_range_color_roundtrip_matches_packed_alpha_encoder_matrix() {
        let (kr, kb) = (0.2126, 0.0722);
        let transform = YuvColorTransform::from(
            hiraku_codec::YuvColorTransform::from_luma_coefficients(kr, kb, false),
        );
        for rgb in [[0.8, 0.1, 0.2], [0.1, 0.8, 0.2], [0.1, 0.2, 0.8]] {
            let [r, g, b] = rgb;
            let y = kr * r + (1.0 - kr - kb) * g + kb * b;
            let sample = Vec4::new(
                y,
                128.0 / 255.0 + (b - y) / (2.0 * (1.0 - kb)),
                128.0 / 255.0 + (r - y) / (2.0 * (1.0 - kr)),
                1.0,
            );
            for (row, expected) in [transform.row_r, transform.row_g, transform.row_b]
                .into_iter()
                .zip(rgb)
            {
                assert!((row.dot(sample) - expected).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn limited_range_black_and_white_map_to_display_endpoints() {
        let transform = YuvColorTransform::from(
            hiraku_codec::YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true),
        );
        let black = Vec4::new(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        let white = Vec4::new(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0);
        for row in [transform.row_r, transform.row_g, transform.row_b] {
            assert!(row.dot(black).abs() < 1.0e-5);
            assert!((row.dot(white) - 1.0).abs() < 1.0e-5);
        }
    }
}
