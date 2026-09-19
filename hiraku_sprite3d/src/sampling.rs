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
    #[test]
    fn color_layout_and_transfer_are_independent() {
        assert_eq!(color_mode(TextureFormat::R8Unorm, true), 1);
        assert_eq!(color_mode(TextureFormat::Rg8Unorm, true), 2);
        assert_eq!(color_mode(TextureFormat::Rg8Unorm, false), 4);
        assert_eq!(color_mode(TextureFormat::Rgba8UnormSrgb, true), 0);
        assert_eq!(color_mode(TextureFormat::Rgba8UnormSrgb, false), 5);
    }
}
