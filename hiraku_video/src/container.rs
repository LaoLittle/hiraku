use std::{io::Cursor, sync::Arc};

use symphonia::core::{
    codecs::{audio::well_known as audio_codecs, video::well_known as video_codecs},
    formats::{FormatOptions, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
use thiserror::Error;

use crate::asset::VideoMetadata as MediaMetadata;
use hiraku_media::{
    AudioDecoderConfig, ChunkType, EncodedAudioChunk, EncodedChunk, EncodedVideoChunk,
    VideoDecoderConfig,
};

/// Container parsing is separate from codec processing. More codec mappings
/// can be added here without changing the public decoder interfaces.
pub struct MatroskaDemuxer {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    video_track: u32,
    audio_track: u32,
    video_base: (u32, u32),
    audio_base: (u32, u32),
    first_video: bool,
    pub video_config: VideoDecoderConfig,
    pub audio_config: AudioDecoderConfig,
}

pub enum DemuxedChunk {
    Video(EncodedVideoChunk),
    Audio(EncodedAudioChunk),
}

impl MatroskaDemuxer {
    pub fn new(bytes: Arc<[u8]>, extension: &str) -> Result<Self, MediaError> {
        let format = open_container(bytes, extension)?;
        let video = format
            .tracks()
            .iter()
            .find(|track| {
                track
                    .codec_params
                    .as_ref()
                    .and_then(|p| p.video())
                    .is_some_and(|p| p.codec == video_codecs::CODEC_ID_AV1)
            })
            .ok_or(MediaError::MissingAv1)?;
        let audio = format
            .tracks()
            .iter()
            .find(|track| {
                track
                    .codec_params
                    .as_ref()
                    .and_then(|p| p.audio())
                    .is_some_and(|p| p.codec == audio_codecs::CODEC_ID_OPUS)
            })
            .ok_or(MediaError::MissingOpus)?;
        let vp = video
            .codec_params
            .as_ref()
            .and_then(|p| p.video())
            .ok_or(MediaError::MissingAv1)?;
        let ap = audio
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(MediaError::MissingOpus)?;
        let width = vp
            .width
            .filter(|v| *v != 0)
            .ok_or(MediaError::MissingDimensions)?;
        let height = vp
            .height
            .filter(|v| *v != 0)
            .ok_or(MediaError::MissingDimensions)?;
        let description: Option<Arc<[u8]>> =
            vp.extra_data.first().map(|d| Arc::from(d.data.as_ref()));
        let codec = description
            .as_deref()
            .filter(|d| d.len() >= 4 && d[0] == 0x81)
            .map(|d| {
                format!(
                    "av01.{}.{:02}{}.{}",
                    d[1] >> 5,
                    d[1] & 31,
                    if d[2] & 128 != 0 { 'H' } else { 'M' },
                    if d[2] & 64 == 0 {
                        "08"
                    } else if d[2] & 32 == 0 {
                        "10"
                    } else {
                        "12"
                    }
                )
            })
            .unwrap_or_else(|| "av01.0.04M.08".into());
        let mut video_config = VideoDecoderConfig::new(codec.as_str(), width.into(), height.into());
        video_config.description = description;
        let channels = ap.channels.as_ref().map_or(2, |c| c.count());
        let channels =
            u16::try_from(channels).map_err(|_| MediaError::UnsupportedChannels(channels))?;
        let rate = ap.sample_rate.unwrap_or(48000);
        if channels == 0 {
            return Err(MediaError::UnsupportedChannels(0));
        }
        if rate == 0 {
            return Err(MediaError::InvalidSampleRate);
        }
        let audio_config = AudioDecoderConfig::new("opus", rate, channels);
        let video_track = video.id;
        let audio_track = audio.id;
        let video_base = video
            .time_base
            .map(|b| (b.numer.get(), b.denom.get()))
            .unwrap_or((1, 1));
        let audio_base = audio
            .time_base
            .map(|b| (b.numer.get(), b.denom.get()))
            .unwrap_or((1, 1));
        Ok(Self {
            format,
            video_track,
            audio_track,
            video_base,
            audio_base,
            first_video: true,
            video_config,
            audio_config,
        })
    }

    pub fn next_chunk(&mut self) -> Result<Option<DemuxedChunk>, MediaError> {
        loop {
            let Some(packet) = self
                .format
                .next_packet()
                .map_err(|e| MediaError::Container(e.to_string()))?
            else {
                return Ok(None);
            };
            let is_video = packet.track_id == self.video_track;
            if !is_video && packet.track_id != self.audio_track {
                continue;
            }
            let base = if is_video {
                self.video_base
            } else {
                self.audio_base
            };
            let timestamp = i64::try_from(
                i128::from(packet.pts.get()) * i128::from(base.0) * 1_000_000 / i128::from(base.1),
            )
            .map_err(|_| MediaError::Container("timestamp overflow".into()))?;
            let duration = u64::try_from(
                u128::from(packet.block_dur().get()) * u128::from(base.0) * 1_000_000
                    / u128::from(base.1),
            )
            .map_err(|_| MediaError::Container("duration overflow".into()))?;
            // Sequential demux starts at the first independently decodable
            // video chunk. This adapter does not expose arbitrary seeking.
            let kind = if !is_video || self.first_video {
                ChunkType::Key
            } else {
                ChunkType::Delta
            };
            let chunk = EncodedChunk {
                kind,
                timestamp,
                duration: Some(duration),
                data: Arc::from(packet.data.as_ref()),
            };
            return Ok(Some(if is_video {
                self.first_video = false;
                DemuxedChunk::Video(EncodedVideoChunk(chunk))
            } else {
                DemuxedChunk::Audio(EncodedAudioChunk(chunk))
            }));
        }
    }
}

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
    let demuxer = MatroskaDemuxer::new(Arc::from(bytes), extension)?;
    Ok(MediaMetadata {
        width: demuxer.video_config.coded_width,
        height: demuxer.video_config.coded_height,
        sample_rate: demuxer.audio_config.sample_rate,
        channels: demuxer.audio_config.number_of_channels,
    })
}
