use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
    reflect::TypePath,
};
use thiserror::Error;

#[derive(Asset, TypePath, Debug, Clone)]
#[expect(dead_code, reason = "typed asset is exposed for game-side systems and scripts")]
pub struct RhaiScriptAsset {
    pub source: String,
}

#[derive(Default, TypePath)]
pub struct RhaiScriptAssetLoader;

#[derive(Debug, Error)]
pub enum RhaiScriptAssetLoaderError {
    #[error("failed to read script bytes: {0}")]
    Io(#[from] std::io::Error),
    #[error("script is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

impl AssetLoader for RhaiScriptAssetLoader {
    type Asset = RhaiScriptAsset;
    type Settings = ();
    type Error = RhaiScriptAssetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        Ok(RhaiScriptAsset {
            source: String::from_utf8(bytes)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["rhai"]
    }
}

#[derive(Asset, TypePath, Debug, Clone)]
#[expect(dead_code, reason = "typed asset is exposed for generic binary loading")]
pub struct BytesAsset {
    pub bytes: Vec<u8>,
}

#[derive(Default, TypePath)]
pub struct BytesAssetLoader;

#[derive(Debug, Error)]
pub enum BytesAssetLoaderError {
    #[error("failed to read file bytes: {0}")]
    Io(#[from] std::io::Error),
}

impl AssetLoader for BytesAssetLoader {
    type Asset = BytesAsset;
    type Settings = ();
    type Error = BytesAssetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(BytesAsset { bytes })
    }
}
