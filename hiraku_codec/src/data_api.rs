//! Buffer-facing WebCodecs contracts. Rust ownership replaces detached objects:
//! close consumes a value, while cloning AudioData retains its shared buffer.
use crate::{AudioData, CodecError, VideoFrame};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSampleFormat {
    U8,
    S16,
    S32,
    F32,
    U8Planar,
    S16Planar,
    S32Planar,
    F32Planar,
}
#[derive(Clone, Debug)]
pub struct AudioDataInit {
    pub format: AudioSampleFormat,
    pub sample_rate: u32,
    pub number_of_frames: u32,
    pub number_of_channels: u16,
    pub timestamp: i64,
    pub data: Arc<[u8]>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioDataCopyToOptions {
    pub plane_index: u32,
    pub frame_offset: u32,
    pub frame_count: Option<u32>,
    pub format: Option<AudioSampleFormat>,
}
impl AudioData {
    pub fn new(init: AudioDataInit) -> Result<Self, CodecError> {
        if init.sample_rate == 0 || init.number_of_channels == 0 || init.number_of_frames == 0 {
            return Err(CodecError::Configuration(
                "audio dimensions must be nonzero".into(),
            ));
        }
        if init.format != AudioSampleFormat::F32 {
            return Err(CodecError::Unsupported(
                "AudioData construction currently accepts interleaved f32 only".into(),
            ));
        }
        let count = (init.number_of_frames as usize)
            .checked_mul(init.number_of_channels as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| CodecError::Configuration("audio buffer size overflow".into()))?;
        let bytes = init
            .data
            .get(..count)
            .ok_or_else(|| CodecError::Configuration("audio buffer is too short".into()))?;
        let samples = bytes
            .chunks_exact(4)
            .map(|v| f32::from_ne_bytes([v[0], v[1], v[2], v[3]]))
            .collect::<Vec<_>>()
            .into();
        Ok(Self {
            timestamp: init.timestamp,
            sample_rate: init.sample_rate,
            number_of_channels: init.number_of_channels,
            samples,
        })
    }
    pub fn format(&self) -> AudioSampleFormat {
        AudioSampleFormat::F32
    }
    pub fn duration(&self) -> f64 {
        self.number_of_frames() as f64 * 1_000_000.0 / f64::from(self.sample_rate)
    }
    fn copy_range(
        &self,
        options: AudioDataCopyToOptions,
    ) -> Result<(usize, usize, bool), CodecError> {
        let planar = match options.format.unwrap_or(AudioSampleFormat::F32) {
            AudioSampleFormat::F32 => false,
            AudioSampleFormat::F32Planar => true,
            _ => {
                return Err(CodecError::Unsupported(
                    "audio copy conversion supports f32 and f32-planar".into(),
                ));
            }
        };
        if self.number_of_channels == 0
            || options.plane_index
                >= if planar {
                    u32::from(self.number_of_channels)
                } else {
                    1
                }
        {
            return Err(CodecError::Configuration(
                "invalid audio plane index".into(),
            ));
        }
        let offset = options.frame_offset as usize;
        let remaining = self
            .number_of_frames()
            .checked_sub(offset)
            .ok_or_else(|| CodecError::Configuration("audio frame offset out of bounds".into()))?;
        let count = options.frame_count.map_or(remaining, |v| v as usize);
        if count > remaining {
            return Err(CodecError::Configuration(
                "audio frame count out of bounds".into(),
            ));
        }
        Ok((offset, count, planar))
    }
    pub fn allocation_size(&self, options: AudioDataCopyToOptions) -> Result<usize, CodecError> {
        let (_, count, planar) = self.copy_range(options)?;
        count
            .checked_mul(if planar {
                1
            } else {
                self.number_of_channels as usize
            })
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| CodecError::Configuration("audio buffer size overflow".into()))
    }
    pub fn copy_to(
        &self,
        destination: &mut [u8],
        options: AudioDataCopyToOptions,
    ) -> Result<(), CodecError> {
        let (offset, count, planar) = self.copy_range(options)?;
        let output = destination
            .get_mut(..self.allocation_size(options)?)
            .ok_or_else(|| CodecError::Configuration("audio destination is too small".into()))?;
        let channels = self.number_of_channels as usize;
        let samples = &self.samples[offset * channels..(offset + count) * channels];
        for (i, chunk) in output.chunks_exact_mut(4).enumerate() {
            let sample = if planar {
                samples[i * channels + options.plane_index as usize]
            } else {
                samples[i]
            };
            chunk.copy_from_slice(&sample.to_ne_bytes());
        }
        Ok(())
    }
    pub fn close(self) {
        drop(self);
    }
}
impl VideoFrame {
    pub fn coded_width(&self) -> u32 {
        self.width
    }
    pub fn coded_height(&self) -> u32 {
        self.height
    }
    pub fn close(self) {
        drop(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_plane_copy_checks_ranges_and_preserves_channels() {
        let audio = AudioData {
            timestamp: 0,
            sample_rate: 48000,
            number_of_channels: 2,
            samples: Arc::from([1.0, 2.0, 3.0, 4.0]),
        };
        let options = AudioDataCopyToOptions {
            plane_index: 1,
            format: Some(AudioSampleFormat::F32Planar),
            ..Default::default()
        };
        let mut dst = [0; 8];
        audio.copy_to(&mut dst, options).expect("copy");
        assert_eq!(
            f32::from_ne_bytes(dst[..4].try_into().expect("four bytes")),
            2.0
        );
        assert_eq!(
            f32::from_ne_bytes(dst[4..].try_into().expect("four bytes")),
            4.0
        );
        assert!(audio.copy_to(&mut dst[..4], options).is_err());
        assert!(
            audio
                .allocation_size(AudioDataCopyToOptions {
                    frame_offset: 3,
                    ..Default::default()
                })
                .is_err()
        );
    }
}
