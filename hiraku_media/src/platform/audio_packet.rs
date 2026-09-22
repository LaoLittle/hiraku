use crate::{AudioDecoderConfig, CodecError};

pub(crate) struct AudioPacketDecoder {
    decoder: hiraku_opus::OpusDecoder,
    frames: usize,
    channels: usize,
}

impl AudioPacketDecoder {
    pub fn new(config: &AudioDecoderConfig) -> Result<Self, CodecError> {
        if config.codec.0 != "opus"
            || !matches!(config.number_of_channels, 1 | 2)
            || !matches!(config.sample_rate, 8000 | 12000 | 16000 | 24000 | 48000)
            || config
                .description
                .as_ref()
                .is_some_and(|data| !data.is_empty())
        {
            return Err(CodecError::Unsupported(format!(
                "synchronous audio configuration: {config:?}"
            )));
        }
        let channels = usize::from(config.number_of_channels);
        let decoder = hiraku_opus::OpusDecoder::new(config.sample_rate as i32, channels)
            .map_err(|error| CodecError::Operation(error.to_string()))?;
        Ok(Self {
            decoder,
            frames: (config.sample_rate / 1000 * 120) as usize,
            channels,
        })
    }
    pub fn output_capacity(&self) -> usize {
        self.frames * self.channels
    }
    pub fn decode_into(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, CodecError> {
        if packet.is_empty() || output.len() < self.output_capacity() {
            return Err(CodecError::Configuration("audio packet must be nonempty and output must hold the decoder's maximum packet size".into()));
        }
        let count = self
            .decoder
            .decode(packet, self.frames, output)
            .map_err(|error| CodecError::Operation(error.to_string()))?;
        if count > self.frames
            || output[..count * self.channels]
                .iter()
                .any(|sample| !sample.is_finite())
        {
            return Err(CodecError::Operation(
                "audio decoder returned invalid PCM".into(),
            ));
        }
        Ok(count)
    }
}
