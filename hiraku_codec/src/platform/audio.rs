use crate::{AudioData, AudioDecoderConfig, AudioPacketDecoder, CodecError, EncodedChunk};

pub(super) fn supports_audio(config: &AudioDecoderConfig) -> bool {
    config.codec == crate::Codec::Opus
        && matches!(config.number_of_channels, 1 | 2)
        && matches!(config.sample_rate, 8000 | 12000 | 16000 | 24000 | 48000)
        && config.description.as_ref().is_none_or(|d| d.is_empty())
}

pub(super) struct Audio {
    decoder: AudioPacketDecoder,
    config: AudioDecoderConfig,
}
impl Audio {
    pub fn new(config: AudioDecoderConfig) -> Result<Self, CodecError> {
        if !supports_audio(&config) {
            return Err(CodecError::Unsupported(config.codec.to_string()));
        }
        let decoder = AudioPacketDecoder::new(&config)?;
        Ok(Self { decoder, config })
    }
    pub fn decode(&mut self, chunk: EncodedChunk) -> Result<Vec<AudioData>, CodecError> {
        let mut samples = vec![0.0; self.decoder.output_capacity()];
        let count = self.decoder.decode_into(&chunk.data, &mut samples)?;
        samples.truncate(count * self.config.number_of_channels as usize);
        Ok(vec![AudioData {
            timestamp: chunk.timestamp,
            sample_rate: self.config.sample_rate,
            number_of_channels: self.config.number_of_channels,
            samples: samples.into(),
        }])
    }
    pub fn flush(&mut self) -> Result<Vec<AudioData>, CodecError> {
        Ok(Vec::new())
    }
}
