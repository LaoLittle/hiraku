mod video_toolbox;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder(video_toolbox::VideoToolboxDecoder);

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        if !video_toolbox::av1_hardware_decode_supported() {
            return Err(CodecError::Unsupported(
                "hardware AV1 decoding is unavailable".into(),
            ));
        }
        let description = config.description.as_ref().ok_or_else(|| {
            CodecError::Unsupported("VideoToolbox requires a codec description".into())
        })?;
        video_toolbox::VideoToolboxDecoder::new(
            config.coded_width,
            config.coded_height,
            description,
        )
        .map(Self)
        .map_err(CodecError::Operation)
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        _: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        self.0
            .decode(
                &chunk.data,
                chunk.timestamp,
                chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64,
                1,
                1_000_000,
            )
            .map_err(CodecError::Operation)
    }
    pub fn flush(&mut self, _: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        self.0.finish().map_err(CodecError::Operation)
    }
}
