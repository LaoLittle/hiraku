//! NDK MediaCodec byte-buffer decoding. Driver buffers are released on the
//! worker thread after copying; neither JNI nor a rendering surface is needed.
use super::frame::{Plane, planar_frame};
mod color;
use crate::*;
pub(super) use MediaCodecDecoder as VideoDecoder;
use ndk::media::{
    media_codec::{
        DequeuedInputBufferResult as Input, DequeuedOutputBufferInfoResult as Output, MediaCodec,
        MediaCodecDirection,
    },
    media_format::MediaFormat,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) struct MediaCodecDecoder {
    nal: Option<super::nal::NalInput>,
    codec: MediaCodec,
    origin: Option<i64>,
    codec_name: String,
}
fn error(value: impl std::fmt::Display) -> CodecError {
    CodecError::Operation(format!("MediaCodec: {value}"))
}
fn continuing(cancel: &AtomicBool, start: Instant) -> Result<(), CodecError> {
    if cancel.load(Ordering::Acquire) {
        return Err(error("decode cancelled"));
    }
    if start.elapsed() > Duration::from_secs(5) {
        return Err(error("decoder made no progress within 5 seconds"));
    }
    Ok(())
}
impl Drop for MediaCodecDecoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}
impl MediaCodecDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let nal = super::nal::NalInput::new(config)?;
        if nal
            .as_ref()
            .and_then(|n| n.bit_depth)
            .or(config.codec.bit_depth())
            .is_some_and(|depth| depth > 8)
        {
            return Err(CodecError::Unsupported(
                "MediaCodec bridge currently negotiates 8-bit output only".into(),
            ));
        }
        let mime = match Some(config.codec.family()) {
            Some("avc1" | "avc3") => "video/avc",
            Some("hvc1" | "hev1") => "video/hevc",
            Some("av01") => "video/av01",
            Some("vp09") => {
                super::vp9::configuration(config)?;
                "video/x-vnd.on2.vp9"
            }
            Some("vp8") if config.codec == crate::Codec::Vp8 && config.description.is_none() => {
                "video/x-vnd.on2.vp8"
            }
            _ => return Err(CodecError::Unsupported(config.codec.to_string())),
        };
        let codec = MediaCodec::from_decoder_type(mime)
            .ok_or_else(|| error(format!("no MediaCodec decoder for {mime}")))?;
        // Do not silently introduce Android's VP8/VP9 software decoders into
        // the hardware adapter. Hiraku's software adapter remains AV1-only.
        let name = codec.name().map_err(error)?;
        if name.starts_with("c2.android.") || name.starts_with("OMX.google.") {
            return Err(CodecError::Unsupported(format!(
                "{name} is a software decoder"
            )));
        }
        let mut format = MediaFormat::new();
        format.set_str("mime", mime);
        format.set_i32("width", i32::try_from(config.coded_width).map_err(error)?);
        format.set_i32("height", i32::try_from(config.coded_height).map_err(error)?);
        // Explicit linear I420. Never reinterpret flexible/tiled vendor formats.
        format.set_i32("color-format", 19);
        if config.optimize_for_latency {
            format.set_i32("low-latency", 1);
        }
        // NAL parameter sets are prepended to key access units below. Passing
        // avcC/hvcC as csd-0 would incorrectly treat a record as Annex B.
        if let Some(description) = config.description.as_ref().filter(|_| nal.is_none()) {
            format.set_buffer("csd-0", description);
        }
        codec
            .configure(&format, None, MediaCodecDirection::Decoder)
            .map_err(error)?;
        codec.start().map_err(error)?;
        Ok(Self {
            nal,
            codec,
            origin: None,
            codec_name: config.codec.to_string(),
        })
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        cancelled: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        let origin = *self.origin.get_or_insert(chunk.timestamp.min(0));
        let timestamp = chunk
            .timestamp
            .checked_sub(origin)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| error("timestamp is outside the configured MediaCodec timeline"))?;
        let mut frames = Vec::new();
        let bytes = match &self.nal {
            Some(nal) => nal.annex_b(&chunk.data, chunk.kind == ChunkType::Key)?,
            None => std::borrow::Cow::Borrowed(chunk.data.as_ref()),
        };
        self.submit(&bytes, timestamp, 0, cancelled, &mut frames)?;
        self.drain(false, cancelled, &mut frames)?;
        Ok(frames)
    }
    pub fn flush(&mut self, cancelled: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        let mut frames = Vec::new();
        self.submit(&[], 0, 4, cancelled, &mut frames)?;
        self.drain(true, cancelled, &mut frames)?;
        // WebCodecs flush drains before resetting EOS so future keyframes work.
        self.codec.flush().map_err(error)?;
        Ok(frames)
    }
    fn submit(
        &self,
        bytes: &[u8],
        timestamp: u64,
        flags: u32,
        cancel: &AtomicBool,
        frames: &mut Vec<VideoFrame>,
    ) -> Result<(), CodecError> {
        let start = Instant::now();
        loop {
            continuing(cancel, start)?;
            match self
                .codec
                .dequeue_input_buffer(Duration::from_millis(5))
                .map_err(error)?
            {
                Input::Buffer(mut input) => {
                    let dst = input.buffer_mut();
                    if bytes.len() > dst.len() {
                        return Err(error("access unit exceeds MediaCodec input capacity"));
                    }
                    // Copy initialized bytes once into the codec-owned buffer.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            dst.as_mut_ptr().cast(),
                            bytes.len(),
                        );
                    }
                    self.codec
                        .queue_input_buffer(input, 0, bytes.len(), timestamp, flags)
                        .map_err(error)?;
                    return Ok(());
                }
                Input::TryAgainLater => {
                    self.drain(false, cancel, frames)?;
                }
            }
        }
    }
    fn drain(
        &self,
        until_eos: bool,
        cancel: &AtomicBool,
        frames: &mut Vec<VideoFrame>,
    ) -> Result<(), CodecError> {
        let mut progress = Instant::now();
        loop {
            continuing(cancel, progress)?;
            match self
                .codec
                .dequeue_output_buffer(if until_eos {
                    Duration::from_millis(5)
                } else {
                    Duration::ZERO
                })
                .map_err(error)?
            {
                Output::TryAgainLater if !until_eos => return Ok(()),
                Output::TryAgainLater => {}
                Output::OutputFormatChanged | Output::OutputBuffersChanged => {}
                Output::Buffer(buffer) => {
                    let info = *buffer.info();
                    let eos = info.flags() & 4 != 0;
                    let result = if info.size() > 0 && info.flags() & 2 == 0 {
                        (|| {
                            let start = usize::try_from(info.offset()).map_err(error)?;
                            let end = start
                                .checked_add(usize::try_from(info.size()).map_err(error)?)
                                .ok_or_else(|| error("output range overflow"))?;
                            let bytes = buffer
                                .buffer()
                                .get(start..end)
                                .ok_or_else(|| error("output range exceeds buffer"))?;
                            let timestamp = info
                                .presentation_time_us()
                                .checked_add(self.origin.unwrap_or(0))
                                .ok_or_else(|| error("timestamp overflow"))?;
                            copy_output(bytes, &buffer.format(), timestamp, &self.codec_name)
                                .map(Some)
                        })()
                    } else {
                        Ok(None)
                    };
                    // Return even unsupported or malformed output buffers.
                    let released = self
                        .codec
                        .release_output_buffer(buffer, false)
                        .map_err(error);
                    let frame = result?;
                    released?;
                    if let Some(frame) = frame {
                        frames.push(frame);
                    }
                    progress = Instant::now();
                    if eos {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn copy_output(
    bytes: &[u8],
    format: &MediaFormat,
    timestamp: i64,
    codec: &str,
) -> Result<VideoFrame, CodecError> {
    let positive = |key: &str| -> Result<usize, CodecError> {
        format
            .i32(key)
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| error(format!("missing/invalid output {key}")))
    };
    let width = positive("width")?;
    let height = positive("height")?;
    let stride = positive("stride").unwrap_or(width);
    let slice_height = positive("slice-height").unwrap_or(height);
    let crop = |key: &str, default| {
        format
            .i32(key)
            .map_or(Ok(default), |v| usize::try_from(v).map_err(error))
    };
    let left = crop("crop-left", 0)?;
    let top = crop("crop-top", 0)?;
    let right = crop("crop-right", width - 1)?;
    let bottom = crop("crop-bottom", height - 1)?;
    if left > right
        || top > bottom
        || right >= width
        || bottom >= height
        || left % 2 != 0
        || top % 2 != 0
        || stride < width
        || slice_height < height
    {
        return Err(error("unsupported crop/stride geometry"));
    }
    let w = right - left + 1;
    let h = bottom - top + 1;
    let nv12 = match format.i32("color-format") {
        Some(19) => false,
        Some(21) => true,
        other => {
            return Err(error(format!(
                "unsupported output color format {other:?}; expected linear I420/NV12"
            )));
        }
    };
    let y_size = stride
        .checked_mul(slice_height)
        .ok_or_else(|| error("plane overflow"))?;
    let cs = if nv12 { stride } else { stride.div_ceil(2) };
    let u_start = y_size
        .checked_add(
            (top / 2)
                .checked_mul(cs)
                .ok_or_else(|| error("plane overflow"))?,
        )
        .and_then(|n| n.checked_add(left / 2 * if nv12 { 2 } else { 1 }))
        .ok_or_else(|| error("plane overflow"))?;
    let v_start = if nv12 {
        u_start.checked_add(1)
    } else {
        cs.checked_mul(slice_height.div_ceil(2))
            .and_then(|n| u_start.checked_add(n))
    }
    .ok_or_else(|| error("plane overflow"))?;
    let y_start = top
        .checked_mul(stride)
        .and_then(|n| n.checked_add(left))
        .ok_or_else(|| error("plane overflow"))?;
    let tail = |offset| {
        bytes
            .get(offset..)
            .ok_or_else(|| error("plane outside output buffer"))
    };

    let (transform, transfer) = color::resolve(
        format.i32("color-standard"),
        format.i32("color-transfer"),
        format.i32("color-range"),
        codec,
    )?;
    planar_frame(
        timestamp,
        w as u32,
        h as u32,
        [
            Plane {
                bytes: tail(y_start)?,
                stride,
                pixel_stride: 1,
            },
            Plane {
                bytes: tail(u_start)?,
                stride: cs,
                pixel_stride: if nv12 { 2 } else { 1 },
            },
            Plane {
                bytes: tail(v_start)?,
                stride: cs,
                pixel_stride: if nv12 { 2 } else { 1 },
            },
        ],
        transform,
        transfer,
    )
}
