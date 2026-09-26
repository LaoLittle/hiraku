use crate::*;
pub(super) fn supports_audio(_: &AudioDecoderConfig) -> bool { false }
pub(super) struct Video;
pub(super) struct Audio;
fn disabled() -> CodecError { CodecError::Unsupported("software adapter is disabled".into()) }
impl Video {
    pub fn new(_: &VideoDecoderConfig) -> Result<Self, CodecError> { Err(disabled()) }
    pub fn decode(&mut self, _: EncodedChunk) -> Result<Vec<VideoFrame>, CodecError> { Err(disabled()) }
    pub fn flush(&mut self) -> Result<Vec<VideoFrame>, CodecError> { Err(disabled()) }
}
impl Audio {
    pub fn new(_: AudioDecoderConfig) -> Result<Self, CodecError> { Err(disabled()) }
    pub fn decode(&mut self, _: EncodedChunk) -> Result<Vec<AudioData>, CodecError> { Err(disabled()) }
    pub fn flush(&mut self) -> Result<Vec<AudioData>, CodecError> { Err(disabled()) }
}
