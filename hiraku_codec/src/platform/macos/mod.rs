mod video_toolbox;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder {
    decoder: Option<video_toolbox::VideoToolboxDecoder>,
    annex_b: Option<bool>,
    parameter_sets: Vec<Vec<u8>>,
    depth: u8,
}

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let nal = super::nal::NalInput::new(config)?;
        let depth = nal
            .as_ref()
            .and_then(|n| n.bit_depth)
            .or(config.codec.bit_depth())
            .unwrap_or(8);
        if depth > 10 {
            return Err(CodecError::Unsupported(
                "VideoToolbox bridge supports up to 10-bit output".into(),
            ));
        }
        if nal.as_ref().is_some_and(|n| n.length_size.is_none()) {
            let hevc = matches!(config.codec, crate::Codec::Hevc(_));
            let codec = u32::from_be_bytes(if hevc { *b"hvc1" } else { *b"avc1" });
            if !video_toolbox::hardware_decode_supported(codec) {
                return Err(CodecError::Unsupported(config.codec.to_string()));
            }
            // In-band parameter sets only arrive with the first key access unit.
            // HEVC uses 10-bit output to avoid truncating Main10 input.
            return Ok(Self {
                decoder: None,
                annex_b: Some(hevc),
                parameter_sets: Vec::new(),
                depth: if hevc { 10 } else { 8 },
            });
        }
        let (codec_type, atom, description) = match Some(config.codec.family()) {
            Some("avc1" | "avc3" | "hvc1" | "hev1") => {
                let hevc = matches!(config.codec, crate::Codec::Hevc(_));
                (
                    u32::from_be_bytes(if hevc { *b"hvc1" } else { *b"avc1" }),
                    if hevc { "hvcC" } else { "avcC" },
                    config
                        .description
                        .as_deref()
                        .ok_or_else(|| {
                            CodecError::Unsupported(
                        "VideoToolbox NAL input currently requires an avcC/hvcC description".into())
                        })?
                        .to_vec(),
                )
            }
            Some("av01") => (
                u32::from_be_bytes(*b"av01"),
                "av1C",
                config
                    .description
                    .as_deref()
                    .ok_or_else(|| {
                        CodecError::Unsupported(
                            "VideoToolbox requires an AV1 codec description".into(),
                        )
                    })?
                    .to_vec(),
            ),
            Some("vp09") => (
                u32::from_be_bytes(*b"vp09"),
                "vpcC",
                super::vp9::configuration(config)?.to_vec(),
            ),
            _ => return Err(CodecError::Unsupported(config.codec.to_string())),
        };
        if !video_toolbox::hardware_decode_supported(codec_type) {
            return Err(CodecError::Unsupported(format!(
                "hardware decoding is unavailable for {}",
                config.codec
            )));
        }
        video_toolbox::VideoToolboxDecoder::new(
            config.coded_width,
            config.coded_height,
            codec_type,
            atom,
            &description,
            depth,
        )
        .map(|decoder| Self {
            decoder: Some(decoder),
            annex_b: None,
            parameter_sets: Vec::new(),
            depth,
        })
        .map_err(CodecError::Operation)
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        _: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        let mut frames = Vec::new();
        let mut converted = Vec::new();
        let bytes = if let Some(hevc) = self.annex_b {
            let units = super::nal::annex_b_units(&chunk.data)?;
            let sets: Vec<_> = units
                .iter()
                .filter(|unit| {
                    if hevc {
                        matches!((unit[0] >> 1) & 63, 32..=34)
                    } else {
                        matches!(unit[0] & 31, 7 | 8)
                    }
                })
                .map(|v| v.to_vec())
                .collect();
            if !sets.is_empty() && sets != self.parameter_sets {
                // Drain delayed pictures before replacing an in-band format.
                if let Some(old) = self.decoder.as_mut() {
                    frames.extend(old.finish().map_err(CodecError::Operation)?);
                }
                self.decoder = Some(
                    video_toolbox::VideoToolboxDecoder::from_parameter_sets(
                        hevc, &sets, self.depth,
                    )
                    .map_err(CodecError::Operation)?,
                );
                self.parameter_sets = sets;
            }
            for unit in units {
                let length = u32::try_from(unit.len())
                    .map_err(|_| CodecError::Configuration("NAL is too large".into()))?;
                converted.extend_from_slice(&length.to_be_bytes());
                converted.extend_from_slice(unit);
            }
            converted.as_slice()
        } else {
            chunk.data.as_ref()
        };
        let decoded = self
            .decoder
            .as_mut()
            .ok_or_else(|| {
                CodecError::Configuration("Annex B key chunk must contain parameter sets".into())
            })?
            .decode(
                bytes,
                chunk.timestamp,
                chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64,
                1,
                1_000_000,
            )
            .map_err(CodecError::Operation)?;
        frames.extend(decoded);
        Ok(frames)
    }
    pub fn flush(&mut self, _: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        match &mut self.decoder {
            Some(d) => d.finish().map_err(CodecError::Operation),
            None => Ok(Vec::new()),
        }
    }
}
