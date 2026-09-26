//! WebCodecs image decoding contracts. No image adapter is enabled yet.
//! MIME types select image decoders (not video registry codec strings).
use crate::{CodecError, VideoFrame};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorSpaceConversion {
    #[default]
    Default,
    None,
}
#[derive(Clone, Debug)]
pub struct ImageDecoderInit {
    pub mime_type: String,
    pub data: Arc<[u8]>,
    pub color_space_conversion: ColorSpaceConversion,
    pub desired_width: Option<u32>,
    pub desired_height: Option<u32>,
    pub prefer_animation: Option<bool>,
}
impl ImageDecoderInit {
    pub fn new(mime_type: impl Into<String>, data: impl Into<Arc<[u8]>>) -> Self {
        Self {
            mime_type: mime_type.into(),
            data: data.into(),
            color_space_conversion: Default::default(),
            desired_width: None,
            desired_height: None,
            prefer_animation: None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ImageDecodeOptions {
    pub frame_index: u32,
    pub complete_frames_only: bool,
}
impl Default for ImageDecodeOptions {
    fn default() -> Self {
        Self {
            frame_index: 0,
            complete_frames_only: true,
        }
    }
}
#[derive(Debug)]
pub struct ImageDecodeResult {
    pub image: VideoFrame,
    pub complete: bool,
}
#[derive(Clone, Debug)]
pub struct ImageTrack {
    animated: bool,
    frame_count: u32,
    /// Positive infinity represents unbounded repetition, as in WebCodecs.
    repetition_count: f32,
    selected: bool,
}
impl ImageTrack {
    pub fn animated(&self) -> bool {
        self.animated
    }
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }
    pub fn repetition_count(&self) -> f32 {
        self.repetition_count
    }
    pub fn selected(&self) -> bool {
        self.selected
    }
}
#[derive(Debug, Default)]
pub struct ImageTrackList {
    tracks: Vec<ImageTrack>,
}
impl ImageTrackList {
    pub fn len(&self) -> usize {
        self.tracks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
    pub fn get(&self, index: usize) -> Option<&ImageTrack> {
        self.tracks.get(index)
    }
    pub fn selected_index(&self) -> Option<usize> {
        self.tracks.iter().position(|t| t.selected)
    }
    pub fn selected_track(&self) -> Option<&ImageTrack> {
        self.selected_index().and_then(|i| self.get(i))
    }
    pub async fn ready(&self) -> Result<(), CodecError> {
        Err(unavailable())
    }
}
/// Constructor fails until an image adapter is supplied. No placeholder images
/// or fictitious completed/ready events are ever returned.
pub struct ImageDecoder {
    mime_type: String,
    tracks: ImageTrackList,
    closed: bool,
}
fn unavailable() -> CodecError {
    CodecError::Unsupported("image decoder adapters are not implemented".into())
}
impl ImageDecoder {
    pub fn new(init: ImageDecoderInit) -> Result<Self, CodecError> {
        if init.mime_type.is_empty()
            || init.desired_width == Some(0)
            || init.desired_height == Some(0)
        {
            return Err(CodecError::Configuration(
                "invalid image decoder MIME type or dimensions".into(),
            ));
        }
        Err(unavailable())
    }
    pub async fn is_type_supported(_mime_type: &str) -> Result<bool, CodecError> {
        Ok(false)
    }
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }
    pub fn tracks(&self) -> &ImageTrackList {
        &self.tracks
    }
    pub fn complete(&self) -> bool {
        false
    }
    pub async fn completed(&self) -> Result<(), CodecError> {
        self.require_open()?;
        Err(unavailable())
    }
    pub async fn decode(
        &mut self,
        _options: ImageDecodeOptions,
    ) -> Result<ImageDecodeResult, CodecError> {
        self.require_open()?;
        Err(unavailable())
    }
    /// Selection goes through the decoder so a future adapter can cancel pending
    /// work when the selected track changes.
    pub fn select_track(&mut self, _index: Option<usize>) -> Result<(), CodecError> {
        self.require_open()?;
        Err(unavailable())
    }
    pub fn reset(&mut self) -> Result<(), CodecError> {
        self.require_open()?;
        Err(unavailable())
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    fn require_open(&self) -> Result<(), CodecError> {
        if self.closed {
            Err(CodecError::InvalidState("image decoder is closed"))
        } else {
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interface_does_not_claim_image_support() {
        assert!(
            !futures_lite::future::block_on(ImageDecoder::is_type_supported("image/png"))
                .expect("query")
        );
        assert!(matches!(
            ImageDecoder::new(ImageDecoderInit::new("image/png", &b"png"[..])),
            Err(CodecError::Unsupported(_))
        ));
        assert!(ImageDecodeOptions::default().complete_frames_only);
    }
}
