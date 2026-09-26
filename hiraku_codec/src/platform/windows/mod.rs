mod media_foundation;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder(media_foundation::MediaFoundationDecoder);

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let subtype = match config.codec.0.split('.').next() {
            Some("av01") => windows::Win32::Media::MediaFoundation::MFVideoFormat_AV1,
            Some("vp09") => {
                super::vp9::configuration(config)?;
                windows::Win32::Media::MediaFoundation::MFVideoFormat_VP90
            }
            Some("vp8") if config.codec.0 == "vp8" && config.description.is_none() => {
                windows::Win32::Media::MediaFoundation::MFVideoFormat_VP80
            }
            _ => return Err(CodecError::Unsupported(config.codec.0.clone())),
        };
        media_foundation::MediaFoundationDecoder::new(
            config.coded_width,
            config.coded_height,
            subtype,
        )
        .map(Self)
        .map_err(CodecError::Operation)
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        cancelled: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        self.0
            .decode(
                &chunk.data,
                chunk.timestamp,
                chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64,
                1,
                1_000_000,
                cancelled,
            )
            .map_err(CodecError::Operation)
    }
    pub fn flush(&mut self, cancelled: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        self.0.finish(cancelled).map_err(CodecError::Operation)
    }
}
