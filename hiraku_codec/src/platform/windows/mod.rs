mod media_foundation;
use crate::{CodecError, EncodedChunk, VideoDecoderConfig, VideoFrame};
use std::sync::atomic::AtomicBool;

pub(super) struct VideoDecoder(
    media_foundation::MediaFoundationDecoder,
    Option<super::nal::NalInput>,
);

impl VideoDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let nal = super::nal::NalInput::new(config)?;
        if nal
            .as_ref()
            .and_then(|n| n.bit_depth)
            .or(config.codec.bit_depth())
            .is_some_and(|depth| depth > 8)
        {
            return Err(CodecError::Unsupported(
                "Media Foundation bridge currently negotiates 8-bit NV12 only".into(),
            ));
        }
        let subtype = match Some(config.codec.family()) {
            Some("avc1" | "avc3") => windows::Win32::Media::MediaFoundation::MFVideoFormat_H264,
            Some("hvc1" | "hev1") => windows::Win32::Media::MediaFoundation::MFVideoFormat_HEVC,
            Some("av01") => windows::Win32::Media::MediaFoundation::MFVideoFormat_AV1,
            Some("vp09") => {
                super::vp9::configuration(config)?;
                windows::Win32::Media::MediaFoundation::MFVideoFormat_VP90
            }
            Some("vp8") if config.codec == crate::Codec::Vp8 && config.description.is_none() => {
                windows::Win32::Media::MediaFoundation::MFVideoFormat_VP80
            }
            _ => return Err(CodecError::Unsupported(config.codec.to_string())),
        };
        media_foundation::MediaFoundationDecoder::new(
            config.coded_width,
            config.coded_height,
            subtype,
        )
        .map(|decoder| Self(decoder, nal))
        .map_err(CodecError::Operation)
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        cancelled: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        let bytes = match &self.1 {
            Some(nal) => nal.annex_b(&chunk.data, chunk.kind == crate::ChunkType::Key)?,
            None => std::borrow::Cow::Borrowed(chunk.data.as_ref()),
        };
        self.0
            .decode(
                &bytes,
                chunk.timestamp,
                chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64,
                1,
                1_000_000,
                cancelled,
            )
            .map_err(CodecError::Operation)
    }
    pub fn flush(&mut self, cancelled: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        self.0.finish(cancelled).map_err(CodecError::Operation)
    }
}
