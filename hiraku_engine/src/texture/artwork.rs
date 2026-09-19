//! Authoring semantics, separate from decoded channel layout.
use bevy::{prelude::*, render::extract_resource::ExtractResourcePlugin};
use hiraku_sprite3d::sampling::{ImageSampling, color_mode};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TextureType {
    #[default]
    Color,
    Mask,
    Data,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ColorSpace {
    Srgb,
    Linear,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextureMetadata {
    #[serde(default)]
    pub texture_type: TextureType,
    pub color_space: Option<ColorSpace>,
}
impl TextureMetadata {
    pub fn canonical(self) -> Self {
        let color_space = if self.texture_type == TextureType::Data {
            ColorSpace::Linear
        } else {
            self.color_space
                .unwrap_or(if self.texture_type == TextureType::Color {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                })
        };
        Self {
            color_space: Some(color_space),
            ..self
        }
    }
    pub fn mode(self, format: bevy::render::render_resource::TextureFormat) -> u32 {
        let srgb = self
            .color_space
            .unwrap_or(if self.texture_type == TextureType::Color {
                ColorSpace::Srgb
            } else {
                ColorSpace::Linear
            })
            == ColorSpace::Srgb;
        if self.texture_type == TextureType::Data {
            return if format.is_srgb() { 5 } else { 0 };
        }
        color_mode(format, srgb)
    }
}
pub(crate) struct ArtworkPlugin;
impl Plugin for ArtworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImageSampling>()
            .add_plugins(ExtractResourcePlugin::<ImageSampling>::default())
            .add_systems(
                PostUpdate,
                (update_metadata, sync_materials)
                    .chain()
                    .after(crate::render::world_sprite::sync_world_sprites)
                    .after(hiraku_sprite3d::Sprite3dSync)
                    .before(hiraku_sprite3d::ImageSamplingSync),
            );
        crate::render::color_ui::install(app);
    }
}
fn update_metadata(
    images: Res<Assets<Image>>,
    server: Res<AssetServer>,
    catalog: Option<Res<super::TextureCatalog>>,
    mut sampling: ResMut<ImageSampling>,
) {
    if !images.is_changed() && !catalog.as_ref().is_some_and(|c| c.is_changed()) {
        return;
    }
    let mut next = std::collections::HashMap::new();
    for (id, image) in images.iter() {
        let Some(path) = server.get_path(id) else {
            continue;
        };
        let path = path.to_string();
        let metadata = catalog
            .as_ref()
            .and_then(|c| c.metadata.get(&path))
            .copied()
            .unwrap_or_default();
        next.insert(id, metadata.mode(image.texture_descriptor.format));
    }
    if sampling.0 != next {
        sampling.0 = next;
    }
}
fn sync_materials(
    images: Res<Assets<Image>>,
    sampling: Res<ImageSampling>,
    mut world: ResMut<Assets<crate::render::world_sprite::WorldSpriteMaterial>>,
    mut alpha: ResMut<Assets<crate::render::character_part::AlphaMaskMaterial>>,
    mut multiply: ResMut<Assets<crate::render::character_part::MultiplyMaterial>>,
    mut rule: ResMut<Assets<crate::effect::transition::RuleTransitionMaterial>>,
    mut custom: ResMut<Assets<crate::effect::custom::CustomScreenEffectMaterial>>,
    mut ui: ResMut<Assets<crate::render::ui_quad::UiQuadMaterial>>,
) {
    let mode = |image: &Handle<Image>| sampling.mode(image, &images);
    macro_rules! sync {
        ($assets:ident, $m:ident, $value:expr) => {{
            let updates: Vec<_> = $assets
                .iter()
                .filter_map(|(id, $m)| {
                    let value = $value;
                    ($m.sampling != value).then_some((id, value))
                })
                .collect();
            for (id, value) in updates {
                if let Some(mut m) = $assets.get_mut(id) {
                    m.sampling = value;
                }
            }
        }};
    }
    sync!(
        world,
        m,
        UVec4::new(m.image.as_ref().map_or(0, mode), 0, 0, 0)
    );
    sync!(
        alpha,
        m,
        UVec4::new(mode(&m.texture), mode(&m.mask_texture), 0, 0)
    );
    sync!(multiply, m, UVec4::new(mode(&m.texture), 0, 0, 0));
    sync!(
        rule,
        m,
        UVec4::new(mode(&m.from_texture), mode(&m.to_texture), 0, 0)
    );
    sync!(
        custom,
        m,
        UVec4::new(mode(&m.source_texture), mode(&m.target_texture), 0, 0)
    );
    sync!(ui, m, UVec4::new(mode(&m.image), 0, 0, 0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::TextureFormat;
    #[test]
    fn metadata_defaults_and_data_semantics_are_explicit() {
        assert_eq!(
            TextureMetadata::default().canonical(),
            TextureMetadata {
                texture_type: TextureType::Color,
                color_space: Some(ColorSpace::Srgb)
            }
            .canonical()
        );
        for (text, mode) in [
            (".{}", 2),
            (".{ textureType: \"color\", colorSpace: \"linear\" }", 4),
            (".{ textureType: \"mask\" }", 4),
            (".{ textureType: \"data\" }", 0),
        ] {
            let metadata: TextureMetadata = hiraku_script::hson::from_str(text).expect("metadata");
            assert_eq!(metadata.mode(TextureFormat::Rg8Unorm), mode);
        }
        assert!(
            hiraku_script::hson::from_str::<TextureMetadata>(".{ textureType: \"unknown\" }")
                .is_err()
        );
    }
    #[test]
    fn sampling_metadata_never_expands_source_pixels() {
        let image = Image::from_dynamic(
            image::DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(
                2,
                1,
                image::LumaA([128, 96]),
            )),
            true,
            default(),
        );
        assert_eq!(
            TextureMetadata::default().mode(image.texture_descriptor.format),
            2
        );
        assert_eq!(image.data.as_deref(), Some([128, 96, 128, 96].as_slice()));
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rg8Unorm);
    }
}
