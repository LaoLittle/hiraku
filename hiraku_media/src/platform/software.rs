use std::sync::Arc;
use hiraku_opus::OpusDecoder;
use hiraku_rav1d::{Decoder as Av1Decoder, Picture as Av1Picture, PixelLayout, PlanarImageComponent, Rav1dError, Settings};
use crate::{AudioData, AudioDecoderConfig, CodecError, DecodeSettings, EncodedChunk, TransferFunction, VideoDecoderConfig, VideoFrame, VideoPixels, YuvColorTransform};

pub(super) fn supports_video(config: &VideoDecoderConfig) -> bool {
    // The current software frame bridge is 8-bit 4:2:0. Keep dispatch here,
    // rather than restricting codec strings in the public API.
    let fields: Vec<_> = config.codec.0.split('.').collect();
    matches!(fields.len(), 4 | 10) && fields[0] == "av01" && fields[1] == "0" && fields[3] == "08"
        && fields[2].len() == 3
        && fields[2].as_bytes()[..2].iter().all(u8::is_ascii_digit)
        && fields[2][..2].parse::<u8>().is_ok_and(|level| level <= 23)
        && matches!(fields[2].as_bytes()[2], b'M' | b'H')
        && (fields.len() == 4 || (fields[4] == "0" && matches!(fields[5], "110" | "111" | "112")
            && fields[6..9].iter().all(|field| field.len() == 2 && field.bytes().all(|c| c.is_ascii_digit()))
            && !matches!(fields[7], "16" | "18")
            && matches!(fields[9], "0" | "1")))
}
pub(super) fn supports_audio(config: &AudioDecoderConfig) -> bool {
    config.codec.0 == "opus" && matches!(config.number_of_channels, 1 | 2)
        && matches!(config.sample_rate, 8000 | 12000 | 16000 | 24000 | 48000)
        && config.description.as_ref().is_none_or(|d| d.is_empty())
}

pub(super) struct Video {
    decoder: Av1Decoder,
}
impl Video {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        if !supports_video(config) { return Err(CodecError::Unsupported(config.codec.0.clone())); }
        let available = std::thread::available_parallelism().map(usize::from).unwrap_or(1);
        let (threads, delay) = resolve_decode_settings(&config.software, available);
        let mut settings = Settings::new();
        settings.set_n_threads(threads);
        settings.set_max_frame_delay(if config.optimize_for_latency { 1 } else { delay });
        Ok(Self { decoder: Av1Decoder::with_settings(&settings).map_err(operation)? })
    }
    pub fn decode(&mut self, chunk: EncodedChunk) -> Result<Vec<VideoFrame>, CodecError> {
        let mut frames = Vec::new();
        let mut result = self.decoder.send_data(chunk.data.to_vec().into_boxed_slice(), None,
            Some(chunk.timestamp), chunk.duration.map(|d| d.min(i64::MAX as u64) as i64));
        loop {
            match self.decoder.get_picture() {
                Ok(picture) => frames.push(picture_to_yuv420(picture.clone(), picture.timestamp().unwrap_or(0))
                    .map_err(CodecError::Operation)?),
                Err(Rav1dError::TryAgain) => {},
                Err(e) => return Err(operation(e)),
            }
            match result {
                Ok(()) => break,
                Err(Rav1dError::TryAgain) => result = self.decoder.send_pending_data(),
                Err(e) => return Err(operation(e)),
            }
        }
        Ok(frames)
    }
    pub fn flush(&mut self) -> Result<Vec<VideoFrame>, CodecError> {
        let mut frames = Vec::new();
        // get_picture drains delayed pictures. rav1d_flush is a state reset,
        // so it must never precede draining at end-of-stream.
        loop {
            match self.decoder.get_picture() {
                Ok(picture) => {
                    let timestamp = picture.timestamp().unwrap_or(0);
                    frames.push(picture_to_yuv420(picture, timestamp).map_err(CodecError::Operation)?);
                }
                Err(Rav1dError::TryAgain) => break,
                Err(e) => return Err(operation(e)),
            }
        }
        Ok(frames)
    }
}
pub(super) struct Audio { decoder: OpusDecoder, config: AudioDecoderConfig }
impl Audio {
    pub fn new(config: AudioDecoderConfig) -> Result<Self, CodecError> {
        if !supports_audio(&config) { return Err(CodecError::Unsupported(config.codec.0.clone())); }
        let decoder = OpusDecoder::new(config.sample_rate as i32, config.number_of_channels.into()).map_err(operation)?;
        Ok(Self { decoder, config })
    }
    pub fn decode(&mut self, chunk: EncodedChunk) -> Result<Vec<AudioData>, CodecError> {
        let frame_size = (self.config.sample_rate / 1000 * 120) as usize;
        let mut samples = vec![0.0; frame_size * self.config.number_of_channels as usize];
        let count = self.decoder.decode(&chunk.data, frame_size, &mut samples).map_err(operation)?;
        samples.truncate(count * self.config.number_of_channels as usize);
        Ok(vec![AudioData { timestamp: chunk.timestamp, sample_rate: self.config.sample_rate,
            number_of_channels: self.config.number_of_channels, samples: samples.into() }])
    }
    pub fn flush(&mut self) -> Result<Vec<AudioData>, CodecError> { Ok(Vec::new()) }
}
fn operation(error: impl std::fmt::Display) -> CodecError { CodecError::Operation(error.to_string()) }

fn resolve_decode_settings(settings: &DecodeSettings, available: usize) -> (u32, u32) {
    let threads = settings.decoder_threads.unwrap_or(match available {
        0 | 1 => 1, 2..=4 => 2, 5..=8 => 4, 9..=12 => 5, 13..=16 => 6, _ => 8,
    }).clamp(1, 256);
    (threads, settings.max_frame_delay.unwrap_or(if available <= 4 { 2 } else { 3 }).clamp(1, threads))
}
fn picture_to_yuv420(picture: Av1Picture, timestamp: i64) -> Result<VideoFrame, String> {
    if picture.bit_depth() != 8 || picture.pixel_layout() != PixelLayout::I420 {
        return Err(format!(
            "unsupported AV1 pixel format: expected 8-bit 4:2:0, got {}-bit {:?}",
            picture.bit_depth(),
            picture.pixel_layout()
        ));
    }
    let width = picture.width();
    let height = picture.height();
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let (color_transform, transfer) = picture_color_transform(&picture)?;
    let y_stride = picture.stride(PlanarImageComponent::Y);
    let u_stride = picture.stride(PlanarImageComponent::U);
    let v_stride = picture.stride(PlanarImageComponent::V);
    if u_stride != v_stride {
        return Err(format!(
            "unsupported AV1 plane layout: U stride {u_stride} differs from V stride {v_stride}"
        ));
    }

    // rav1d pictures are intentionally !Send/!Sync. Copy each padded plane once
    // at the decoder boundary, retaining its row stride so no row-by-row pack is
    // needed here or in the render world.
    let plane_len = |stride: u32, plane_height: u32| {
        usize::try_from(stride)
            .expect("u32 stride must fit usize")
            .checked_mul(usize::try_from(plane_height).expect("u32 height must fit usize"))
            .expect("decoded video plane size must fit usize")
    };
    let y_len = plane_len(y_stride, height);
    let u_len = plane_len(u_stride, chroma_height);
    let v_len = plane_len(v_stride, chroma_height);
    let u_offset = y_len;
    let v_offset = y_len
        .checked_add(u_len)
        .expect("decoded video plane offsets must fit usize");
    let total_len = v_offset
        .checked_add(v_len)
        .expect("decoded video frame size must fit usize");
    let mut planes = Arc::<[u8]>::new_uninit_slice(total_len);
    let destination = Arc::get_mut(&mut planes).expect("new Arc storage must be uniquely owned");
    let mut copy_plane = |offset: usize, source: &[u8]| {
        let destination = &mut destination[offset..offset + source.len()];
        destination.write_copy_of_slice(source);
    };
    copy_plane(0, &picture.plane(PlanarImageComponent::Y)[..y_len]);
    copy_plane(u_offset, &picture.plane(PlanarImageComponent::U)[..u_len]);
    copy_plane(v_offset, &picture.plane(PlanarImageComponent::V)[..v_len]);
    // Every byte in the allocation was initialized by the three exhaustive
    // plane copies above.
    let planes = unsafe { planes.assume_init() };
    Ok(VideoFrame {
        timestamp,
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform,
        transfer,
        pixels: VideoPixels::I420Strided {
            planes,
            u_offset,
            v_offset,
            y_stride,
            chroma_stride: u_stride,
        },
    })
}

fn picture_color_transform(
    picture: &Av1Picture,
) -> Result<(YuvColorTransform, TransferFunction), String> {
    use hiraku_rav1d::pixel::{MatrixCoefficients, TransferCharacteristic, YUVRange};

    let (kr, kb) = match picture.matrix_coefficients() {
        MatrixCoefficients::BT470M => (0.30, 0.11),
        MatrixCoefficients::BT470BG | MatrixCoefficients::ST170M => (0.299, 0.114),
        MatrixCoefficients::ST240M => (0.2122, 0.0865),
        MatrixCoefficients::BT2020NonConstantLuminance
        | MatrixCoefficients::BT2020ConstantLuminance => (0.2627, 0.0593),
        MatrixCoefficients::BT709
        | MatrixCoefficients::Identity
        | MatrixCoefficients::Unspecified
        | MatrixCoefficients::Reserved
        | MatrixCoefficients::YCgCo
        | MatrixCoefficients::ST2085
        | MatrixCoefficients::ChromaticityDerivedNonConstantLuminance
        | MatrixCoefficients::ChromaticityDerivedConstantLuminance
        | MatrixCoefficients::ICtCp => (0.2126, 0.0722),
    };
    let transfer_characteristic = picture.transfer_characteristic();
    let transfer = match transfer_characteristic {
        TransferCharacteristic::Linear => TransferFunction::Linear,
        TransferCharacteristic::SRGB => TransferFunction::Srgb,
        TransferCharacteristic::BT470M => TransferFunction::Gamma22,
        TransferCharacteristic::BT470BG => TransferFunction::Gamma28,
        TransferCharacteristic::BT1886
        | TransferCharacteristic::Unspecified
        | TransferCharacteristic::Reserved0
        | TransferCharacteristic::Reserved
        | TransferCharacteristic::ST170M
        | TransferCharacteristic::ST240M
        | TransferCharacteristic::XVYCC
        | TransferCharacteristic::BT1361E
        | TransferCharacteristic::BT2020Ten
        | TransferCharacteristic::BT2020Twelve => TransferFunction::Bt1886,
        TransferCharacteristic::Logarithmic100
        | TransferCharacteristic::Logarithmic316
        | TransferCharacteristic::PerceptualQuantizer
        | TransferCharacteristic::ST428
        | TransferCharacteristic::HybridLogGamma => {
            return Err(format!(
                "unsupported AV1 transfer characteristic {transfer_characteristic:?}: HDR and log tone mapping are not implemented"
            ));
        }
    };
    Ok((
        YuvColorTransform::from_luma_coefficients(
            kr,
            kb,
            picture.color_range() == YUVRange::Limited,
        ),
        transfer,
    ))
}
