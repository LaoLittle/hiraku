use std::{io::Cursor, sync::Arc};

use symphonia::core::{
    codecs::{audio::well_known as audio_codecs, video::well_known as video_codecs},
    formats::{FormatOptions, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
use thiserror::Error;

use crate::MediaMetadata;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("invalid Matroska/WebM container: {0}")]
    Container(String),
    #[error("media must contain an AV1 video track")]
    MissingAv1,
    #[error("AV1 track must declare non-zero coded dimensions")]
    MissingDimensions,
    #[error("media must contain an Opus audio track")]
    MissingOpus,
    #[error("Opus channel count {0} is unsupported")]
    UnsupportedChannels(usize),
    #[error("Opus sample rate must be non-zero")]
    InvalidSampleRate,
}

pub(crate) fn open_container(
    bytes: Arc<[u8]>,
    extension: &str,
) -> Result<Box<dyn symphonia::core::formats::FormatReader>, MediaError> {
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
        .map_err(|error| MediaError::Container(error.to_string()))
}

pub fn inspect_media(bytes: &[u8], extension: &str) -> Result<MediaMetadata, MediaError> {
    let format = open_container(Arc::from(bytes), extension)?;
    let mut dimensions = None;
    let mut audio_metadata = None;
    for track in format.tracks() {
        let Some(parameters) = &track.codec_params else {
            continue;
        };
        if let Some(video) = parameters.video()
            && video.codec == video_codecs::CODEC_ID_AV1
        {
            dimensions = video.width.zip(video.height).and_then(|(width, height)| {
                (width != 0 && height != 0).then_some((u32::from(width), u32::from(height)))
            });
        }
        if let Some(audio) = parameters.audio()
            && audio.codec == audio_codecs::CODEC_ID_OPUS
        {
            let count = audio
                .channels
                .as_ref()
                .map_or(2, |channels| channels.count());
            let channels =
                u16::try_from(count).map_err(|_| MediaError::UnsupportedChannels(count))?;
            if channels == 0 || channels > 2 {
                return Err(MediaError::UnsupportedChannels(count));
            }
            let sample_rate = audio.sample_rate.unwrap_or(48_000);
            if sample_rate == 0 {
                return Err(MediaError::InvalidSampleRate);
            }
            audio_metadata = Some((sample_rate, channels));
        }
    }
    let (width, height) = dimensions.ok_or_else(|| {
        if format.tracks().iter().any(|track| {
            track
                .codec_params
                .as_ref()
                .and_then(|parameters| parameters.video())
                .is_some_and(|video| video.codec == video_codecs::CODEC_ID_AV1)
        }) {
            MediaError::MissingDimensions
        } else {
            MediaError::MissingAv1
        }
    })?;
    let (sample_rate, channels) = audio_metadata.ok_or(MediaError::MissingOpus)?;
    Ok(MediaMetadata {
        width,
        height,
        sample_rate,
        channels,
    })
}
