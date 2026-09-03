use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::Asset,
    reflect::TypePath,
};
use thiserror::Error;

pub type VideoMetadata = hiraku_media::MediaMetadata;

#[derive(Asset, Clone, Debug, TypePath)]
pub struct VideoAsset {
    pub(crate) media: hiraku_media::EncodedMedia,
    pub metadata: VideoMetadata,
}

#[derive(Default, TypePath)]
pub struct VideoAssetLoader;

#[derive(Debug, Error)]
pub enum VideoAssetLoaderError {
    #[error("failed to read video asset: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Media(#[from] hiraku_media::MediaError),
}

impl AssetLoader for VideoAssetLoader {
    type Asset = VideoAsset;
    type Settings = ();
    type Error = VideoAssetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
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
        let media = hiraku_media::EncodedMedia::inspect(bytes, extension)?;
        let metadata = media.metadata;
        Ok(VideoAsset { media, metadata })
    }

    fn extensions(&self) -> &[&str] {
        &["mkv", "webm"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_only_claims_the_supported_container_extensions() {
        let loader = VideoAssetLoader;
        assert_eq!(loader.extensions(), &["mkv", "webm"]);
    }
}
