use std::{io::Cursor, sync::Arc};
mod codec;
#[cfg(test)]
mod mp4_tests;

use symphonia::core::{
    codecs::audio::well_known as audio_codecs,
    formats::{FormatOptions, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};
use thiserror::Error;

use crate::asset::VideoMetadata as MediaMetadata;
use hiraku_codec::{
    AudioDecoderConfig, ChunkType, EncodedAudioChunk, EncodedChunk, EncodedVideoChunk,
    VideoDecoderConfig,
};

/// Container parsing is separate from codec processing. More codec mappings
/// can be added here without changing the public decoder interfaces.
pub struct MediaDemuxer {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    video_track: u32,
    audio_track: Option<u32>,
    video_base: (u32, u32),
    audio_base: (u32, u32),
    first_video: bool,
    pub video_config: VideoDecoderConfig,
    pub audio_config: Option<AudioDecoderConfig>,
}

pub enum DemuxedChunk {
    Video(EncodedVideoChunk),
    Audio(EncodedAudioChunk),
}

impl MediaDemuxer {
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
                    .is_some()
            })
            .ok_or(MediaError::MissingVideo)?;
        let audio = format.tracks().iter().find(|track| {
            track
                .codec_params
                .as_ref()
                .and_then(|p| p.audio())
                .is_some_and(|p| p.codec == audio_codecs::CODEC_ID_OPUS)
        });
        let vp = video
            .codec_params
            .as_ref()
            .and_then(|p| p.video())
            .ok_or(MediaError::MissingVideo)?;
        if audio.is_none()
            && format
                .tracks()
                .iter()
                .any(|t| t.codec_params.as_ref().and_then(|p| p.audio()).is_some())
        {
            return Err(MediaError::MissingOpus);
        }
        let width = vp
            .width
            .filter(|v| *v != 0)
            .ok_or(MediaError::MissingDimensions)?;
        let height = vp
            .height
            .filter(|v| *v != 0)
            .ok_or(MediaError::MissingDimensions)?;
        let video_config = codec::video_config(vp, width.into(), height.into())?;
        let audio_config = if let Some(ap) = audio
            .and_then(|t| t.codec_params.as_ref())
            .and_then(|p| p.audio())
        {
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
            Some(
                AudioDecoderConfig::new("opus", rate, channels)
                    .map_err(|e| MediaError::Container(e.to_string()))?,
            )
        } else {
            None
        };
        let video_track = video.id;
        let audio_track = audio.map(|t| t.id);
        let video_base = video
            .time_base
            .map(|b| (b.numer.get(), b.denom.get()))
            .unwrap_or((1, 1));
        let audio_base = audio
            .and_then(|t| t.time_base)
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
            if !is_video && Some(packet.track_id) != self.audio_track {
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
    #[error("invalid media container: {0}")]
    Container(String),
    #[error("media must contain a video track")]
    MissingVideo,
    #[error("unsupported video codec: {0}")]
    UnsupportedVideo(String),
    #[error("video track must declare non-zero coded dimensions")]
    MissingDimensions,
    #[error("unsupported audio track: only Opus is currently supported")]
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
    let demuxer = MediaDemuxer::new(Arc::from(bytes), extension)?;
    Ok(MediaMetadata {
        width: demuxer.video_config.coded_width,
        height: demuxer.video_config.coded_height,
        sample_rate: demuxer.audio_config.as_ref().map_or(0, |a| a.sample_rate),
        channels: demuxer
            .audio_config
            .as_ref()
            .map_or(0, |a| a.number_of_channels),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        let mut result = id.to_vec();
        assert!(data.len() < 16383, "small synthetic EBML fixture");
        result.extend_from_slice(&(0x4000u16 | data.len() as u16).to_be_bytes());
        result.extend_from_slice(data);
        result
    }
    #[test]
    fn avc_mkv_loads_and_demuxes_without_an_av1_track() {
        let header = element(
            &[0x1a, 0x45, 0xdf, 0xa3],
            &element(&[0x42, 0x82], b"matroska"),
        );
        let mut entry = element(&[0xd7], &[1]);
        entry.extend(element(&[0x73, 0xc5], &[1]));
        entry.extend(element(&[0x83], &[1]));
        entry.extend(element(&[0x86], b"V_MPEG4/ISO/AVC"));
        entry.extend(element(
            &[0x63, 0xa2],
            &[1, 66, 0, 30, 255, 225, 0, 2, 103, 1, 1, 0, 2, 104, 1],
        ));
        let mut dimensions = element(&[0xb0], &[64]);
        dimensions.extend(element(&[0xba], &[32]));
        entry.extend(element(&[0xe0], &dimensions));
        let mut info = element(&[0x2a, 0xd7, 0xb1], &[0x0f, 0x42, 0x40]);
        info.extend(element(&[0x4d, 0x80], b"fixture"));
        info.extend(element(&[0x57, 0x41], b"fixture"));
        let mut segment = element(&[0x15, 0x49, 0xa9, 0x66], &info);
        segment.extend(element(
            &[0x16, 0x54, 0xae, 0x6b],
            &element(&[0xae], &entry),
        ));
        let mut cluster = element(&[0xe7], &[0]);
        cluster.extend(element(
            &[0xa3],
            &[0x81, 0, 0, 0x80, 0, 0, 0, 2, 0x65, 0x80],
        ));
        segment.extend(element(&[0x1f, 0x43, 0xb6, 0x75], &cluster));
        let mut bytes = header;
        bytes.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
        let metadata = inspect_media(&bytes, "mkv").expect("AVC asset inspection");
        assert_eq!((metadata.width, metadata.height), (64, 32));
        let mut demux = MediaDemuxer::new(bytes.into(), "mkv").expect("AVC demux");
        assert_eq!(demux.video_config.codec.to_string(), "avc1.42001E");
        let Some(DemuxedChunk::Video(chunk)) = demux.next_chunk().expect("chunk") else {
            panic!("expected video");
        };
        assert_eq!(chunk.0.data.as_ref(), &[0, 0, 0, 2, 0x65, 0x80]);
    }
}
