use crate::{AudioDecoderConfig, CodecError};
pub(crate) struct AudioPacketDecoder;
impl AudioPacketDecoder {
    pub fn new(_: &AudioDecoderConfig) -> Result<Self, CodecError> {
        Err(CodecError::Unsupported(
            "synchronous audio requires the software feature".into(),
        ))
    }
    pub fn output_capacity(&self) -> usize {
        0
    }
    pub fn decode_into(&mut self, _: &[u8], _: &mut [f32]) -> Result<usize, CodecError> {
        Err(CodecError::Unsupported(
            "synchronous audio requires the software feature".into(),
        ))
    }
}
