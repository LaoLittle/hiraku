mod video_toolbox;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder(video_toolbox::VideoToolboxDecoder);

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let (codec_type, atom, description) = match config.codec.0.split('.').next() {
            Some("av01") => (u32::from_be_bytes(*b"av01"), "av1C", config.description.as_deref()
                .ok_or_else(|| CodecError::Unsupported("VideoToolbox requires an AV1 codec description".into()))?.to_vec()),
            Some("vp09") => (u32::from_be_bytes(*b"vp09"), "vpcC", super::vp9::configuration(config)?.to_vec()),
            _ => return Err(CodecError::Unsupported(config.codec.0.clone())),
        };
        if !video_toolbox::hardware_decode_supported(codec_type) {
            return Err(CodecError::Unsupported(
                format!("hardware decoding is unavailable for {}", config.codec.0),
            ));
        }
        video_toolbox::VideoToolboxDecoder::new(
            config.coded_width,
            config.coded_height,
            codec_type,
            atom,
            &description,
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
