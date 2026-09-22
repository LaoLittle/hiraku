use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::Asset,
    reflect::TypePath,
};
use thiserror::Error;

/// Packed straight-alpha video: color first, grayscale alpha second.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum AlphaLayout {
    #[default]
    Vertical,
    Horizontal,
}

impl AlphaLayout {
    pub fn display_size(self, width: u32, height: u32) -> Option<(u32, u32)> {
        // Each half must align to the 4:2:0 chroma grid.
        match self {
            Self::Vertical if width > 0 && width % 2 == 0 && height > 0 && height % 4 == 0 => {
                Some((width, height / 2))
            }
            Self::Horizontal if height > 0 && height % 2 == 0 && width > 0 && width % 4 == 0 => {
                Some((width / 2, height))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct VideoLoaderSettings {
    pub layout: AlphaLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct EncodedMedia {
    pub bytes: std::sync::Arc<[u8]>,
}

#[derive(Asset, Clone, Debug, TypePath)]
pub struct VideoAsset {
    pub(crate) media: EncodedMedia,
    pub metadata: VideoMetadata,
    pub alpha_layout: Option<AlphaLayout>,
}

#[derive(Default, TypePath)]
pub struct VideoAssetLoader;

#[derive(Debug, Error)]
pub enum VideoAssetLoaderError {
    #[error("failed to read video asset: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Media(#[from] crate::container::MediaError),
    #[error("packed alpha video dimensions must align each half to the 4:2:0 chroma grid")]
    InvalidAlphaDimensions,
}

impl AssetLoader for VideoAssetLoader {
    type Asset = VideoAsset;
    type Settings = VideoLoaderSettings;
    type Error = VideoAssetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let extension = load_context
            .path()
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("mkv");
        let alpha_layout = matches!(extension, "mkva" | "webma").then_some(settings.layout);
        let container = match extension {
            "mkva" => "mkv",
            "webma" => "webm",
            other => other,
        };
        let metadata = crate::container::inspect_media(&bytes, container)?;
        if let Some(layout) = alpha_layout {
            layout
                .display_size(metadata.width, metadata.height)
                .ok_or(VideoAssetLoaderError::InvalidAlphaDimensions)?;
        }
        let media = EncodedMedia {
            bytes: bytes.into(),
        };
        Ok(VideoAsset {
            media,
            metadata,
            alpha_layout,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["mkv", "webm", "mkva", "webma"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_halves_preserve_display_dimensions_and_chroma_alignment() {
        assert_eq!(AlphaLayout::default(), AlphaLayout::Vertical);
        assert_eq!(
            AlphaLayout::Vertical.display_size(1280, 1440),
            Some((1280, 720))
        );
        assert_eq!(
            AlphaLayout::Horizontal.display_size(2560, 720),
            Some((1280, 720))
        );
        assert_eq!(AlphaLayout::Vertical.display_size(1280, 722), None);
        assert_eq!(AlphaLayout::Horizontal.display_size(1282, 720), None);
        assert_eq!(AlphaLayout::Vertical.display_size(0, 0), None);
    }

    #[test]
    fn loader_only_claims_the_supported_container_extensions() {
        let loader = VideoAssetLoader;
        assert_eq!(loader.extensions(), &["mkv", "webm", "mkva", "webma"]);
    }
}
