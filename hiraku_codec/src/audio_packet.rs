//! Synchronous, caller-buffer decoding for pull-based audio mixers.
//! Unlike AudioDecoder this API never waits on a worker or a browser callback.
use crate::{AudioDecoderConfig, CodecError, platform};

pub struct AudioPacketDecoder(platform::AudioPacketDecoder);

impl AudioPacketDecoder {
    pub fn new(config: &AudioDecoderConfig) -> Result<Self, CodecError> {
        config.validate()?;
        platform::AudioPacketDecoder::new(config).map(Self)
    }

    /// Maximum interleaved sample capacity required by this decoder.
    pub fn output_capacity(&self) -> usize {
        self.0.output_capacity()
    }

    /// Decode one packet. Returns frames per channel, not interleaved samples.
    /// Container pre-skip, gain and end trimming are owned by the caller.
    pub fn decode_into(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, CodecError> {
        self.0.decode_into(packet, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;
    #[test]
    fn packet_decoder_validates_config_and_output_before_decoding() {
        for config in [
            AudioDecoderConfig::new(Codec::Vp8, 48000, 2).expect("valid codec identifier"),
            AudioDecoderConfig::new("opus", 48000, 3).expect("valid codec identifier"),
            AudioDecoderConfig::new("opus", 0, 1).expect("valid codec identifier"),
        ] {
            assert!(AudioPacketDecoder::new(&config).is_err());
        }
        let mut decoder = AudioPacketDecoder::new(
            &AudioDecoderConfig::new("opus", 48000, 1).expect("valid codec identifier"),
        )
        .expect("Opus");
        assert_eq!(decoder.output_capacity(), 5760);
        assert!(
            decoder
                .decode_into(&[0xf8, 0xff, 0xfe], &mut [0.0; 1])
                .is_err()
        );
        let mut pcm = vec![0.0; decoder.output_capacity()];
        assert!(decoder.decode_into(&[], &mut pcm).is_err());
        assert_eq!(
            decoder
                .decode_into(&[0xf8, 0xff, 0xfe], &mut pcm)
                .expect("silence packet"),
            960
        );
        assert!(pcm[..960].iter().all(|sample| sample.is_finite()));
    }
}
