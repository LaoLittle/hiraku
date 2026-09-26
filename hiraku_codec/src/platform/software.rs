pub use imp::*;

#[cfg(feature = "software")]
mod imp {
    use crate::{
        CodecError, DecodeSettings, EncodedChunk, TransferFunction, VideoDecoderConfig, VideoFrame,
        VideoPixels, YuvColorTransform,
    };
    use hiraku_rav1d::{
        Decoder as Av1Decoder, Picture as Av1Picture, PixelLayout, PlanarImageComponent, Rav1dError,
        Settings,
    };
    use std::sync::Arc;

    pub fn supports_video(config: &VideoDecoderConfig) -> bool {
        matches!(config.codec, crate::Codec::Av1(_)) && config.codec.validate().is_ok()
    }

    pub struct Video {
        decoder: Av1Decoder,
    }
    
    impl Video {
        pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
            if !supports_video(config) {
                return Err(CodecError::Unsupported(config.codec.to_string()));
            }
            let available = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);
            let (threads, delay) = resolve_decode_settings(&config.software, available);
            let mut settings = Settings::new();
            settings.set_n_threads(threads);
            settings.set_max_frame_delay(if config.optimize_for_latency {
                1
            } else {
                delay
            });
            Ok(Self {
                decoder: Av1Decoder::with_settings(&settings).map_err(operation)?,
            })
        }
        pub fn decode(&mut self, chunk: EncodedChunk) -> Result<Vec<VideoFrame>, CodecError> {
            let mut frames = Vec::new();
            let mut result = self.decoder.send_data(
                chunk.data.to_vec().into_boxed_slice(),
                None,
                Some(chunk.timestamp),
                chunk.duration.map(|d| d.min(i64::MAX as u64) as i64),
            );
            loop {
                match self.decoder.get_picture() {
                    Ok(picture) => frames.push(
                        picture_to_yuv420(picture.clone(), picture.timestamp().unwrap_or(0))
                            .map_err(CodecError::Operation)?,
                    ),
                    Err(Rav1dError::TryAgain) => {}
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
                        frames.push(
                            picture_to_yuv420(picture, timestamp).map_err(CodecError::Operation)?,
                        );
                    }
                    Err(Rav1dError::TryAgain) => break,
                    Err(e) => return Err(operation(e)),
                }
            }
            Ok(frames)
        }
    }
    fn operation(error: impl std::fmt::Display) -> CodecError {
        CodecError::Operation(error.to_string())
    }

    fn resolve_decode_settings(settings: &DecodeSettings, available: usize) -> (u32, u32) {
        let threads = settings
            .decoder_threads
            .unwrap_or(match available {
                0 | 1 => 1,
                2..=4 => 2,
                5..=8 => 4,
                9..=12 => 5,
                13..=16 => 6,
                _ => 8,
            })
            .clamp(1, 256);
        (
            threads,
            settings
                .max_frame_delay
                .unwrap_or(if available <= 4 { 2 } else { 3 })
                .clamp(1, threads),
        )
    }
    fn picture_to_yuv420(picture: Av1Picture, timestamp: i64) -> Result<VideoFrame, String> {
        if !matches!(picture.bit_depth(), 8 | 10 | 12) {
            return Err(format!(
                "unsupported AV1 pixel format: expected 8-bit 4:2:0, got {}-bit {:?}",
                picture.bit_depth(),
                picture.pixel_layout()
            ));
        }
        let width = picture.width();
        let height = picture.height();
        let monochrome = picture.pixel_layout() == PixelLayout::I400;
        let (chroma_width, chroma_height) = match picture.pixel_layout() {
            PixelLayout::I400 => (1, 1),
            PixelLayout::I420 => (width.div_ceil(2), height.div_ceil(2)),
            PixelLayout::I422 => (width.div_ceil(2), height),
            PixelLayout::I444 => (width, height),
        };
        let (color_transform, transfer) = picture_color_transform(&picture)?;
        let y_stride = picture.stride(PlanarImageComponent::Y);
        let u_stride = if monochrome {
            if picture.bit_depth() == 8 { 1 } else { 2 }
        } else {
            picture.stride(PlanarImageComponent::U)
        };
        let v_stride = if monochrome {
            u_stride
        } else {
            picture.stride(PlanarImageComponent::V)
        };
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
        if monochrome {
            let neutral = (1u16 << (picture.bit_depth() - 1)).to_le_bytes();
            copy_plane(u_offset, &neutral[..u_len]);
            copy_plane(v_offset, &neutral[..v_len]);
        } else {
            copy_plane(u_offset, &picture.plane(PlanarImageComponent::U)[..u_len]);
            copy_plane(v_offset, &picture.plane(PlanarImageComponent::V)[..v_len]);
        }
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
            pixels: if picture.bit_depth() > 8 {
                VideoPixels::Planar16 {
                    planes,
                    u_offset,
                    v_offset,
                    y_stride,
                    chroma_stride: u_stride,
                    bit_depth: picture.bit_depth() as u8,
                }
            } else {
                VideoPixels::I420Strided {
                    planes,
                    u_offset,
                    v_offset,
                    y_stride,
                    chroma_stride: u_stride,
                }
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
            MatrixCoefficients::BT2020NonConstantLuminance => (0.2627, 0.0593),
            MatrixCoefficients::BT709
            | MatrixCoefficients::Identity
            | MatrixCoefficients::Unspecified => (0.2126, 0.0722),
            other => return Err(format!("unsupported AV1 matrix coefficients: {other:?}")),
        };
        let transfer_characteristic = picture.transfer_characteristic();
        let transfer = match transfer_characteristic {
            TransferCharacteristic::PerceptualQuantizer => TransferFunction::Pq,
            TransferCharacteristic::HybridLogGamma => TransferFunction::Hlg,
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
            | TransferCharacteristic::ST428 => {
                return Err(format!(
                    "unsupported AV1 transfer characteristic {transfer_characteristic:?}: HDR and log tone mapping are not implemented"
                ));
            }
        };
        let mut transform = YuvColorTransform::from_luma_coefficients_depth(
            kr,
            kb,
            picture.color_range() == YUVRange::Limited,
            picture.bit_depth() as u8,
        )
        .with_bt2020_primaries(picture.color_primaries() as u8 == 9);
        if picture.matrix_coefficients() == MatrixCoefficients::Identity {
            if picture.color_range() != YUVRange::Full {
                return Err("limited-range identity matrix is unsupported".into());
            }
            // AV1 identity planes are G, B, R, not Y, Cb, Cr.
            transform.row_r = [0.0, 0.0, 1.0, 0.0];
            transform.row_g = [1.0, 0.0, 0.0, 0.0];
            transform.row_b = [0.0, 1.0, 0.0, 0.0];
        }
        Ok((transform, transfer))
    }
}

#[cfg(not(feature = "software"))]
mod imp {
    use crate::*;
    pub struct Video;
    
    fn disabled() -> CodecError {
        CodecError::Unsupported("software adapter is disabled".into())
    }
    
    impl Video {
        pub fn new(_: &VideoDecoderConfig) -> Result<Self, CodecError> {
            Err(disabled())
        }
        pub fn decode(&mut self, _: EncodedChunk) -> Result<Vec<VideoFrame>, CodecError> {
            Err(disabled())
        }
        pub fn flush(&mut self) -> Result<Vec<VideoFrame>, CodecError> {
            Err(disabled())
        }
    }
}