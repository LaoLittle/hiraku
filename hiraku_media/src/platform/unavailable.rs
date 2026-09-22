//! An uninhabited hardware backend when no platform decoder is enabled.
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) enum VideoDecoder {}

impl VideoDecoder {
    pub fn new(_: &VideoDecoderConfig) -> Result<Self, CodecError> {
        Err(CodecError::Unsupported(
            "no platform video decoder enabled".into(),
        ))
    }
    pub fn decode(
        &mut self,
        _: EncodedChunk,
        _: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        match *self {}
    }
    pub fn flush(&mut self, _: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        match *self {}
    }
}
