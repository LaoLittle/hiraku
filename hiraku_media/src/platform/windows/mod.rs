mod media_foundation;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder(media_foundation::MediaFoundationDecoder);

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        media_foundation::MediaFoundationDecoder::new(config.coded_width, config.coded_height)
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
