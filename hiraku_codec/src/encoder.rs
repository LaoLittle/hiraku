//! WebCodecs-shaped encoder contracts. Output uses the same nonblocking poll
//! model as decoders; container muxing is deliberately outside this crate.
use crate::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvcBitstreamFormat {
    #[default]
    Avc,
    AnnexB,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HevcBitstreamFormat {
    #[default]
    Hevc,
    AnnexB,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct AvcEncoderConfig {
    pub format: AvcBitstreamFormat,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct HevcEncoderConfig {
    pub format: HevcBitstreamFormat,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoEncoderEncodeOptionsForAvc {
    pub quantizer: Option<u16>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoEncoderEncodeOptionsForHevc {
    pub quantizer: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlphaOption {
    #[default]
    Discard,
    Keep,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LatencyMode {
    #[default]
    Quality,
    Realtime,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoEncoderBitrateMode {
    #[default]
    Variable,
    Constant,
    Quantizer,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioEncoderBitrateMode {
    #[default]
    Variable,
    Constant,
}

#[derive(Clone, Debug)]
pub struct VideoEncoderConfig {
    pub avc: Option<AvcEncoderConfig>,
    pub hevc: Option<HevcEncoderConfig>,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
    pub bitrate: Option<u64>,
    pub framerate: Option<f64>,
    pub hardware_acceleration: HardwareAcceleration,
    pub alpha: AlphaOption,
    pub scalability_mode: Option<String>,
    pub bitrate_mode: VideoEncoderBitrateMode,
    pub latency_mode: LatencyMode,
}
impl VideoEncoderConfig {
    pub fn new(
        codec: impl TryInto<Codec, Error: Into<CodecError>>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        Ok(Self {
            avc: None,
            hevc: None,
            codec: codec.try_into().map_err(Into::into)?,
            width,
            height,
            display_width: None,
            display_height: None,
            bitrate: None,
            framerate: None,
            hardware_acceleration: HardwareAcceleration::NoPreference,
            alpha: AlphaOption::Discard,
            scalability_mode: None,
            bitrate_mode: VideoEncoderBitrateMode::Variable,
            latency_mode: LatencyMode::Quality,
        })
    }
    fn validate(&self) -> Result<(), CodecError> {
        crate::codec::validate_codec(&self.codec)?;
        if self.width == 0
            || self.height == 0
            || self.display_width == Some(0)
            || self.display_height == Some(0)
            || self.display_width.is_some() != self.display_height.is_some()
            || self.bitrate == Some(0)
            || self
                .framerate
                .is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
        {
            return Err(CodecError::Configuration(
                "invalid video encoder dimensions, bitrate or framerate".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AudioEncoderConfig {
    pub codec: Codec,
    pub sample_rate: u32,
    pub number_of_channels: u16,
    pub bitrate: Option<u64>,
    pub bitrate_mode: AudioEncoderBitrateMode,
}
impl AudioEncoderConfig {
    pub fn new(
        codec: impl TryInto<Codec, Error: Into<CodecError>>,
        sample_rate: u32,
        number_of_channels: u16,
    ) -> Result<Self, CodecError> {
        Ok(Self {
            codec: codec.try_into().map_err(Into::into)?,
            sample_rate,
            number_of_channels,
            bitrate: None,
            bitrate_mode: AudioEncoderBitrateMode::Variable,
        })
    }
    fn validate(&self) -> Result<(), CodecError> {
        AudioDecoderConfig::new(
            self.codec.clone(),
            self.sample_rate,
            self.number_of_channels,
        )?
        .validate()?;
        if self.bitrate == Some(0) {
            return Err(CodecError::Configuration("bitrate must be positive".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VideoEncoderEncodeOptions {
    pub key_frame: bool,
    pub avc: Option<VideoEncoderEncodeOptionsForAvc>,
    pub hevc: Option<VideoEncoderEncodeOptionsForHevc>,
}

#[derive(Clone, Debug, Default)]
pub struct EncodedVideoChunkMetadata {
    pub decoder_config: Option<VideoDecoderConfig>,
    pub temporal_layer_id: Option<u32>,
    pub alpha_side_data: Option<std::sync::Arc<[u8]>>,
}
#[derive(Clone, Debug, Default)]
pub struct EncodedAudioChunkMetadata {
    pub decoder_config: Option<AudioDecoderConfig>,
}

#[derive(Debug)]
pub enum EncoderEvent<C, M> {
    Output { chunk: C, metadata: M },
    Flushed(FlushId),
    Error(CodecError),
}

macro_rules! encoder {
    ($name:ident, $config:ty, $chunk:ty, $metadata:ty) => {
        /// Interface-only until a platform encoder adapter is supplied.
        pub struct $name {
            state: CodecState,
        }
        impl $name {
            pub fn new() -> Result<Self, CodecError> {
                Ok(Self {
                    state: CodecState::Unconfigured,
                })
            }
            pub async fn is_config_supported(
                config: &$config,
            ) -> Result<ConfigSupport<$config>, CodecError> {
                config.validate()?;
                Ok(ConfigSupport {
                    supported: false,
                    config: config.clone(),
                })
            }
            pub fn configure(&mut self, config: $config) -> Result<(), CodecError> {
                self.require_open()?;
                config.validate()?;
                Err(crate::platform::encoder::unavailable())
            }
            pub fn state(&self) -> CodecState {
                self.state
            }
            pub fn encode_queue_size(&self) -> usize {
                0
            }
            pub fn poll(&mut self) -> Option<EncoderEvent<$chunk, $metadata>> {
                None
            }
            pub fn flush(&mut self) -> Result<FlushId, CodecError> {
                self.require_configured()?;
                Err(crate::platform::encoder::unavailable())
            }
            pub fn reset(&mut self) -> Result<(), CodecError> {
                self.require_open()?;
                self.state = CodecState::Unconfigured;
                Ok(())
            }
            pub fn close(&mut self) {
                self.state = CodecState::Closed;
            }
            fn require_open(&self) -> Result<(), CodecError> {
                if self.state == CodecState::Closed {
                    Err(CodecError::InvalidState("encoder is closed"))
                } else {
                    Ok(())
                }
            }
            fn require_configured(&self) -> Result<(), CodecError> {
                if self.state != CodecState::Configured {
                    Err(CodecError::InvalidState("encoder is not configured"))
                } else {
                    Ok(())
                }
            }
        }
    };
}
encoder!(
    VideoEncoder,
    VideoEncoderConfig,
    EncodedVideoChunk,
    EncodedVideoChunkMetadata
);
encoder!(
    AudioEncoder,
    AudioEncoderConfig,
    EncodedAudioChunk,
    EncodedAudioChunkMetadata
);
impl VideoEncoder {
    pub fn encode(
        &mut self,
        _frame: &VideoFrame,
        _options: VideoEncoderEncodeOptions,
    ) -> Result<(), CodecError> {
        self.require_configured()?;
        Err(crate::platform::encoder::unavailable())
    }
}
impl AudioEncoder {
    pub fn encode(&mut self, _data: &AudioData) -> Result<(), CodecError> {
        self.require_configured()?;
        Err(crate::platform::encoder::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;
    #[test]
    fn encoder_never_claims_an_unimplemented_backend() {
        let config =
            VideoEncoderConfig::new("vp09.00.10.08", 64, 64).expect("valid codec identifier");
        assert!(
            !block_on(VideoEncoder::is_config_supported(&config))
                .expect("support")
                .supported
        );
        let mut encoder = VideoEncoder::new().expect("create");
        assert!(matches!(
            encoder.configure(config.clone()),
            Err(CodecError::Unsupported(_))
        ));
        assert_eq!(encoder.state(), CodecState::Unconfigured);
        assert!(encoder.flush().is_err());
        assert!(encoder.poll().is_none());
        encoder.close();
        assert!(encoder.reset().is_err());
        assert!(matches!(
            encoder.configure(config),
            Err(CodecError::InvalidState(_))
        ));
    }
    #[test]
    fn invalid_encoder_configuration_is_not_reported_as_unsupported() {
        let mut config =
            VideoEncoderConfig::new("av01.0.04M.08", 64, 64).expect("valid codec identifier");
        config.framerate = Some(f64::NAN);
        assert!(matches!(
            block_on(VideoEncoder::is_config_supported(&config)),
            Err(CodecError::Configuration(_))
        ));
        let config = AudioEncoderConfig::new("opus", 0, 2).expect("valid codec identifier");
        assert!(block_on(AudioEncoder::is_config_supported(&config)).is_err());
    }
}
