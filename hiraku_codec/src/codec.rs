//! Rust control surface modelled on WebCodecs. Calls enqueue work; output and
//! flush completion are polled, so an ECS system never waits for a decoder.
use crate::{AudioData, Codec, DecodeSettings, VideoFrame, platform};
use std::sync::Arc;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("invalid codec configuration: {0}")]
    Configuration(String),
    #[error("unsupported codec configuration: {0}")]
    Unsupported(String),
    #[error("invalid decoder state: {0}")]
    InvalidState(&'static str),
    #[error("a key chunk is required after configure or flush")]
    KeyRequired,
    #[error("codec operation failed: {0}")]
    Operation(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecState {
    Unconfigured,
    Configured,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HardwareAcceleration {
    #[default]
    NoPreference,
    PreferHardware,
    PreferSoftware,
}

#[derive(Clone, Debug)]
pub struct VideoDecoderConfig {
    pub codec: Codec,
    pub coded_width: u32,
    pub coded_height: u32,
    pub description: Option<Arc<[u8]>>,
    pub hardware_acceleration: HardwareAcceleration,
    pub optimize_for_latency: bool,
    /// Native implementation tuning; browsers may ignore these hints.
    pub software: DecodeSettings,
}

impl VideoDecoderConfig {
    pub fn new(
        codec: impl TryInto<Codec, Error: Into<CodecError>>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        Ok(Self {
            codec: codec.try_into().map_err(Into::into)?,
            coded_width: width,
            coded_height: height,
            description: None,
            hardware_acceleration: HardwareAcceleration::NoPreference,
            optimize_for_latency: false,
            software: DecodeSettings::default(),
        })
    }
    pub(crate) fn validate(&self) -> Result<(), CodecError> {
        validate_codec(&self.codec)?;
        if self.coded_width == 0 || self.coded_height == 0 {
            return Err(CodecError::Configuration(
                "coded dimensions must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AudioDecoderConfig {
    pub codec: Codec,
    pub sample_rate: u32,
    pub number_of_channels: u16,
    pub description: Option<Arc<[u8]>>,
}

impl AudioDecoderConfig {
    pub fn new(
        codec: impl TryInto<Codec, Error: Into<CodecError>>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, CodecError> {
        Ok(Self {
            codec: codec.try_into().map_err(Into::into)?,
            sample_rate,
            number_of_channels: channels,
            description: None,
        })
    }
    pub(crate) fn validate(&self) -> Result<(), CodecError> {
        validate_codec(&self.codec)?;
        if self.sample_rate == 0 || self.number_of_channels == 0 {
            return Err(CodecError::Configuration(
                "sample rate and channel count must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_codec(codec: &Codec) -> Result<(), CodecError> {
    codec.validate()
}

#[derive(Clone, Debug)]
pub struct ConfigSupport<C> {
    pub supported: bool,
    pub config: C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkType {
    Key,
    Delta,
}

/// Timestamps are signed microseconds, preserving negative preroll.
#[derive(Clone, Debug)]
pub struct EncodedChunk {
    pub kind: ChunkType,
    pub timestamp: i64,
    pub duration: Option<u64>,
    pub data: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct EncodedVideoChunk(pub EncodedChunk);
#[derive(Clone, Debug)]
pub struct EncodedAudioChunk(pub EncodedChunk);

macro_rules! chunk_accessors {
    ($name:ident) => {
        impl $name {
            pub fn new(init: EncodedChunk) -> Self {
                Self(init)
            }
            pub fn kind(&self) -> ChunkType {
                self.0.kind
            }
            pub fn timestamp(&self) -> i64 {
                self.0.timestamp
            }
            pub fn duration(&self) -> Option<u64> {
                self.0.duration
            }
            pub fn byte_length(&self) -> usize {
                self.0.data.len()
            }
            pub fn copy_to(&self, destination: &mut [u8]) -> Result<(), CodecError> {
                let output = destination.get_mut(..self.byte_length()).ok_or_else(|| {
                    CodecError::Configuration("chunk destination is too small".into())
                })?;
                output.copy_from_slice(&self.0.data);
                Ok(())
            }
        }
    };
}
chunk_accessors!(EncodedAudioChunk);
chunk_accessors!(EncodedVideoChunk);
pub type EncodedAudioChunkInit = EncodedChunk;
pub type EncodedVideoChunkInit = EncodedChunk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlushId(pub u64);

#[derive(Debug)]
pub enum DecoderEvent<F> {
    Output(F),
    /// All output for work preceding this barrier has been delivered.
    Flushed(FlushId),
    Error(CodecError),
}

macro_rules! decoder {
    ($name:ident, $config:ident, $chunk:ident, $frame:ty, $support:ident) => {
        pub struct $name {
            backend: platform::$name,
            state: CodecState,
            key_required: bool,
            next_flush: u64,
        }

        impl $name {
            pub fn new() -> Result<Self, CodecError> {
                Ok(Self {
                    backend: platform::$name::new()?,
                    state: CodecState::Unconfigured,
                    key_required: true,
                    next_flush: 0,
                })
            }

            pub async fn is_config_supported(
                config: &$config,
            ) -> Result<ConfigSupport<$config>, CodecError> {
                config.validate()?;
                Ok(ConfigSupport {
                    supported: platform::$support(config).await?,
                    config: config.clone(),
                })
            }

            pub fn configure(&mut self, config: $config) -> Result<(), CodecError> {
                if self.state == CodecState::Closed {
                    return Err(CodecError::InvalidState("decoder is closed"));
                }
                config.validate()?;
                self.backend.configure(config)?;
                self.state = CodecState::Configured;
                self.key_required = true;
                Ok(())
            }

            pub fn state(&self) -> CodecState {
                self.state
            }

            pub fn decode_queue_size(&self) -> usize {
                self.backend.decode_queue_size()
            }

            pub fn pending_output(&self) -> usize {
                self.backend.pending_output()
            }

            pub fn decode(&mut self, chunk: $chunk) -> Result<(), CodecError> {
                self.require_configured()?;
                if self.key_required && chunk.0.kind != ChunkType::Key {
                    return Err(CodecError::KeyRequired);
                }
                if chunk.0.data.is_empty() {
                    return Err(CodecError::Operation("encoded chunk is empty".into()));
                }
                self.backend.decode(chunk.0)?;
                self.key_required = false;
                Ok(())
            }

            /// Enqueue a drain barrier. Observe DecoderEvent::Flushed without
            /// blocking a thread. Decode calls after this require a key chunk.
            pub fn flush(&mut self) -> Result<FlushId, CodecError> {
                self.require_configured()?;
                self.next_flush = self
                    .next_flush
                    .checked_add(1)
                    .ok_or_else(|| CodecError::Operation("flush identifier exhausted".into()))?;
                let id = FlushId(self.next_flush);
                self.backend.flush(id)?;
                self.key_required = true;
                Ok(id)
            }

            pub fn poll(&mut self) -> Option<DecoderEvent<$frame>> {
                let event = self.backend.poll()?;
                if matches!(event, DecoderEvent::Error(_)) {
                    self.backend.close();
                    self.state = CodecState::Closed;
                }
                Some(event)
            }

            /// Discard queued work, output and pending flush barriers.
            /// The next operation must configure the decoder again.
            pub fn reset(&mut self) -> Result<(), CodecError> {
                if self.state == CodecState::Closed {
                    return Err(CodecError::InvalidState("decoder is closed"));
                }
                self.backend.close();
                self.state = CodecState::Unconfigured;
                self.key_required = true;
                Ok(())
            }

            pub fn close(&mut self) {
                self.backend.close();
                self.state = CodecState::Closed;
            }

            fn require_configured(&self) -> Result<(), CodecError> {
                if self.state != CodecState::Configured {
                    Err(CodecError::InvalidState("decoder is not configured"))
                } else {
                    Ok(())
                }
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                self.backend.close();
            }
        }
    };
}

decoder!(
    VideoDecoder,
    VideoDecoderConfig,
    EncodedVideoChunk,
    VideoFrame,
    video_config_supported
);
decoder!(
    AudioDecoder,
    AudioDecoderConfig,
    EncodedAudioChunk,
    AudioData,
    audio_config_supported
);
