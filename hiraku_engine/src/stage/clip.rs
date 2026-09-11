//! Viewport half-plane masks preserve the view's projection and image UVs.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ViewClip {
    #[default]
    Full,
    Left(f64, f64),
    Right(f64, f64),
}
impl ViewClip {
    #[getter]
    fn full() -> ViewClip { Self::Full }
    fn left(x: f64, angle: f64) -> ViewClip { Self::Left(x, angle) }
    fn right(x: f64, angle: f64) -> ViewClip { Self::Right(x, angle) }
}
}

impl ViewClip {
    pub(super) fn valid(self) -> bool {
        match self {
            Self::Full => true,
            Self::Left(x, angle) | Self::Right(x, angle) => {
                x.is_finite()
                    && (0.0..=100.0).contains(&x)
                    && angle.is_finite()
                    && angle.abs() < 90.0
            }
        }
    }
    pub(super) fn plane(self, canvas: Vec2) -> Option<Vec3> {
        let (x, angle, sign) = match self {
            Self::Full => return None,
            Self::Left(x, angle) => (x, angle, 1.0),
            Self::Right(x, angle) => (x, angle, -1.0),
        };
        let (sin, cos) = (angle as f32).to_radians().sin_cos();
        Some(Vec3::new(cos, -sin, cos * (x as f32 * 0.01 - 0.5) * canvas.x) * sign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opposing_masks_share_the_same_diagonal_without_stretching_uvs() {
        let canvas = Vec2::new(1600.0, 900.0);
        let left = ViewClip::Left(25.0, 10.0)
            .plane(canvas)
            .expect("left plane");
        let right = ViewClip::Right(25.0, 10.0)
            .plane(canvas)
            .expect("right plane");
        assert_eq!(left, -right);
        for y in [-450.0, 0.0, 450.0] {
            let x = -400.0 + y * 10.0_f32.to_radians().tan();
            assert!((left.truncate().dot(Vec2::new(x, y)) - left.z).abs() < 0.001);
        }
        assert!(ViewClip::Full.plane(canvas).is_none());
        assert!(!ViewClip::Left(50.0, 90.0).valid());
        assert!(!ViewClip::Right(f64::NAN, 0.0).valid());
    }
}
