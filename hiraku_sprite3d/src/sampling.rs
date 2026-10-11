//! Image storage and color meaning are independent.
use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::TextureFormat},
};

#[derive(Resource, Default, Clone, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct ImageSampling(pub std::collections::HashMap<AssetId<Image>, u32>);

impl ImageSampling {
    pub fn mode(&self, image: &Handle<Image>, images: &Assets<Image>) -> u32 {
        self.0.get(&image.id()).copied().unwrap_or_else(|| {
            images
                .get(image)
                .map_or(0, |image| color_mode(image.texture_descriptor.format, true))
        })
    }
}

pub fn color_mode(format: TextureFormat, srgb: bool) -> u32 {
    let channels = match format {
        TextureFormat::R8Unorm | TextureFormat::R16Unorm => 1,
        TextureFormat::Rg8Unorm | TextureFormat::Rg16Unorm => 2,
        _ => return if !srgb && format.is_srgb() { 5 } else { 0 },
    };
    channels + if srgb { 0 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered_mix(a: Vec4, b: Vec4, amount: f32) -> Vec4 {
        let cover = |color: Vec4| (color.truncate() * color.w).extend(color.w);
        cover(a).lerp(cover(b), amount)
    }

    #[test]
    fn transparent_rgb_cannot_brighten_filtered_edges() {
        let opaque = Vec4::new(0.1, 0.2, 0.3, 1.0);
        let black = covered_mix(Vec4::ZERO, opaque, 0.5);
        let white = covered_mix(Vec4::new(1.0, 1.0, 1.0, 0.0), opaque, 0.5);
        assert_eq!(black, white);
        assert!(white.abs_diff_eq(Vec4::new(0.05, 0.1, 0.15, 0.5), 1e-6));
    }

    #[test]
    fn filtering_preserves_translucent_coverage_without_multiplying_alpha_twice() {
        let filtered = covered_mix(
            Vec4::new(0.8, 0.4, 0.2, 0.25),
            Vec4::new(0.2, 0.6, 0.8, 0.75),
            0.5,
        );
        assert!(filtered.abs_diff_eq(Vec4::new(0.175, 0.275, 0.325, 0.5), 1e-6));
        let mask = 0.4;
        assert!((filtered * mask).abs_diff_eq(Vec4::new(0.07, 0.11, 0.13, 0.2), 1e-6));
    }

    #[test]
    fn color_layout_and_transfer_are_independent() {
        assert_eq!(color_mode(TextureFormat::R8Unorm, true), 1);
        assert_eq!(color_mode(TextureFormat::Rg8Unorm, true), 2);
        assert_eq!(color_mode(TextureFormat::Rg8Unorm, false), 4);
        assert_eq!(color_mode(TextureFormat::Rgba8UnormSrgb, true), 0);
        assert_eq!(color_mode(TextureFormat::Rgba8UnormSrgb, false), 5);
    }
}
