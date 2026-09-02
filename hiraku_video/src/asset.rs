use std::{io::Cursor, sync::Arc};

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::Asset,
    reflect::TypePath,
};
use symphonia::core::{
    codecs::{audio::well_known as audio_codecs, video::well_known as video_codecs},
    formats::{FormatOptions, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Asset, Clone, Debug, TypePath)]
pub struct VideoAsset {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) bytes: Arc<[u8]>,
    pub metadata: VideoMetadata,
}

#[derive(Default, TypePath)]
pub struct VideoAssetLoader;

#[derive(Debug, Error)]
pub enum VideoAssetLoaderError {
    #[error("failed to read video asset: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Matroska/WebM container: {0}")]
    Container(String),
    #[error("video must contain the supported AV1 video track")]
    MissingAv1,
    #[error("AV1 track must declare non-zero coded dimensions")]
    MissingDimensions,
    #[error("video must contain the supported Opus audio track")]
    MissingOpus,
    #[error("Opus channel count {0} is unsupported")]
    UnsupportedChannels(usize),
    #[error("Opus sample rate must be non-zero")]
    InvalidSampleRate,
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
        let metadata = inspect_media_profile(&bytes, extension)?;
        Ok(VideoAsset {
            bytes: Arc::from(bytes),
            metadata,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["mkv", "webm"]
    }
}

pub(crate) fn open_container(
    bytes: Arc<[u8]>,
    extension: &str,
) -> Result<Box<dyn symphonia::core::formats::FormatReader>, VideoAssetLoaderError> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(extension);
    symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| VideoAssetLoaderError::Container(error.to_string()))
}

fn inspect_media_profile(
    bytes: &[u8],
    extension: &str,
) -> Result<VideoMetadata, VideoAssetLoaderError> {
    let format = open_container(Arc::from(bytes), extension)?;
    let mut video_dimensions = None;
    let mut audio_metadata = None;
    for track in format.tracks() {
        let Some(parameters) = &track.codec_params else {
            continue;
        };
        if let Some(video) = parameters.video()
            && video.codec == video_codecs::CODEC_ID_AV1
        {
            video_dimensions = video.width.zip(video.height).and_then(|(width, height)| {
                (width != 0 && height != 0).then_some((u32::from(width), u32::from(height)))
            });
        }
        if let Some(audio) = parameters.audio()
            && audio.codec == audio_codecs::CODEC_ID_OPUS
        {
            let channels = audio
                .channels
                .as_ref()
                .map_or(2, |channels| channels.count());
            let channels = u16::try_from(channels)
                .map_err(|_| VideoAssetLoaderError::UnsupportedChannels(channels))?;
            if channels == 0 || channels > 2 {
                return Err(VideoAssetLoaderError::UnsupportedChannels(channels.into()));
            }
            let sample_rate = audio.sample_rate.unwrap_or(48_000);
            if sample_rate == 0 {
                return Err(VideoAssetLoaderError::InvalidSampleRate);
            }
            audio_metadata = Some((sample_rate, channels));
        }
    }
    let Some((width, height)) = video_dimensions else {
        if format.tracks().iter().any(|track| {
            track
                .codec_params
                .as_ref()
                .and_then(|parameters| parameters.video())
                .is_some_and(|video| video.codec == video_codecs::CODEC_ID_AV1)
        }) {
            return Err(VideoAssetLoaderError::MissingDimensions);
        }
        return Err(VideoAssetLoaderError::MissingAv1);
    };
    let (sample_rate, channels) = audio_metadata.ok_or(VideoAssetLoaderError::MissingOpus)?;
    Ok(VideoMetadata {
        width,
        height,
        sample_rate,
        channels,
    })
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
