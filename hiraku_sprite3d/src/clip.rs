use bevy::prelude::*;

/// An oriented rectangle in world XY coordinates, independent of sprite UVs.
/// Several sprites can share this value to form one clipping region without
/// extra cameras or render targets. It does not change their internal masks.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct ClipRect {
    center: Vec2,
    half_size: Vec2,
    radians: f32,
}

impl ClipRect {
    pub fn new(center: Vec2, size: Vec2, degrees: f32) -> Result<Self, &'static str> {
        if !center.is_finite()
            || !size.is_finite()
            || size.min_element() <= 0.0
            || !degrees.is_finite()
        {
            return Err(
                "clip rectangle requires finite coordinates, positive size and finite rotation",
            );
        }
        Ok(Self {
            center,
            half_size: size * 0.5,
            radians: degrees.to_radians(),
        })
    }

    /// Two aligned vectors used by materials implementing the same clipping contract.
    pub fn shader_parameters(self) -> [Vec4; 2] {
        [
            self.center
                .extend(self.half_size.x)
                .extend(self.half_size.y),
            Vec4::new(self.radians.cos(), self.radians.sin(), 1.0, 0.0),
        ]
    }

    pub fn contains(self, point: Vec2) -> bool {
        let p = point - self.center;
        let (s, c) = self.radians.sin_cos();
        let local = Vec2::new(c * p.x + s * p.y, -s * p.x + c * p.y);
        local.is_finite() && local.abs().cmple(self.half_size).all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotated_clip_uses_world_space_and_includes_boundary() {
        let clip = ClipRect::new(Vec2::new(100.0, 40.0), Vec2::new(20.0, 4.0), 90.0).expect("clip");
        assert!(clip.contains(Vec2::new(100.0, 49.0)));
        assert!(!clip.contains(Vec2::new(104.0, 40.0)));
        assert!(
            ClipRect::new(Vec2::ZERO, Vec2::splat(10.0), 0.0)
                .expect("clip")
                .contains(Vec2::splat(5.0))
        );
        assert!(ClipRect::new(Vec2::ZERO, Vec2::ZERO, 0.0).is_err());
        assert!(ClipRect::new(Vec2::ZERO, Vec2::ONE, f32::NAN).is_err());
    }
}
