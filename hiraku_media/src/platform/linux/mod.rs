//! Direct VA-API AV1 decode, confined to the existing codec worker.
mod stream;
pub(super) use VaapiDecoder as VideoDecoder;
use super::frame::{Plane, planar_frame};
use crate::{
    CodecError, EncodedChunk, TransferFunction, VideoDecoderConfig, VideoFrame, YuvColorTransform,
};
use cros_codecs::{
    DecodedFormat, Fourcc,
    codec::av1::parser::{BitDepth, ColorConfig, ObuAction, ObuType, ParsedObu, Parser},
    decoder::{
        BlockingMode, DecodedHandle, DecoderEvent, StreamInfo,
        stateless::{
            DecodeError, DynStatelessVideoDecoder, StatelessDecoder, StatelessVideoDecoder,
            av1::Av1,
        },
    },
    video_frame::{
        VideoFrame as DriverFrame,
        frame_pool::{FramePool, PooledVideoFrame},
        gbm_video_frame::{GbmDevice, GbmUsage, GbmVideoFrame},
    },
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Surface = PooledVideoFrame<GbmVideoFrame>;
pub(super) struct VaapiDecoder {
    decoder: DynStatelessVideoDecoder<Surface>,
    device: Arc<GbmDevice>,
    pool: Option<FramePool<GbmVideoFrame>>,
    parser: Parser,
    configuration: Vec<u8>,
    color: Option<(YuvColorTransform, TransferFunction)>,
    pending_color: Option<(YuvColorTransform, TransferFunction)>,
}
fn error(value: impl std::fmt::Display) -> CodecError {
    CodecError::Operation(format!("VA-API: {value}"))
}
fn cancelled(flag: &AtomicBool) -> Result<(), CodecError> {
    if flag.load(Ordering::Acquire) {
        Err(error("decode cancelled"))
    } else {
        Ok(())
    }
}

impl VaapiDecoder {
    pub fn new(config: &VideoDecoderConfig) -> Result<Self, CodecError> {
        let configuration = stream::configuration_obus(config.description.as_deref())?;
        let mut paths = std::fs::read_dir("/dev/dri")
            .map_err(error)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|name| name.starts_with("renderD"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut failures = Vec::new();
        for path in paths {
            match Self::open_device(&path, configuration.clone(), config) {
                Ok(decoder) => return Ok(decoder),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        Err(error(format!(
            "no usable AV1 render device: {}",
            failures.join("; ")
        )))
    }
    fn open_device(
        path: &Path,
        configuration: Vec<u8>,
        config: &VideoDecoderConfig,
    ) -> Result<Self, CodecError> {
        let display = cros_libva::Display::open_drm_display(path).map_err(error)?;
        let profile = cros_libva::VAProfile::VAProfileAV1Profile0;
        if !display
            .query_config_profiles()
            .map_err(error)?
            .contains(&profile)
            || !display
                .query_config_entrypoints(profile)
                .map_err(error)?
                .contains(&cros_libva::VAEntrypoint::VAEntrypointVLD)
        {
            return Err(error("driver does not support AV1 Profile 0 decode"));
        }
        let device = GbmDevice::open(path).map_err(error)?;
        // Some desktop GBM implementations do not support the allocation flags
        // required by cros-codecs. Detect this before consuming any bitstream,
        // so native dispatch can still safely choose rav1d.
        let resolution = cros_codecs::Resolution {
            width: config.coded_width,
            height: config.coded_height,
        };
        let probe = device
            .clone()
            .new_frame(
                Fourcc::from(b"NV12"),
                resolution,
                resolution,
                GbmUsage::Decode,
            )
            .map_err(error)?;
        let _surface = probe.to_native_handle(&display).map_err(error)?;
        let decoder = StatelessDecoder::<Av1, _>::new_vaapi(display, BlockingMode::Blocking)
            .map_err(error)?
            .into_trait_object();
        Ok(Self {
            decoder,
            device,
            pool: None,
            parser: Parser::default(),
            configuration,
            color: None,
            pending_color: None,
        })
    }
    pub fn decode(
        &mut self,
        chunk: EncodedChunk,
        cancel: &AtomicBool,
    ) -> Result<Vec<VideoFrame>, CodecError> {
        let mut frames = Vec::new();
        if !self.configuration.is_empty() {
            let configuration = std::mem::take(&mut self.configuration);
            self.feed(&configuration, chunk.timestamp, cancel, &mut frames)?;
        }
        self.feed(&chunk.data, chunk.timestamp, cancel, &mut frames)?;
        Ok(frames)
    }
    fn feed(
        &mut self,
        mut input: &[u8],
        timestamp: i64,
        cancel: &AtomicBool,
        frames: &mut Vec<VideoFrame>,
    ) -> Result<(), CodecError> {
        while !input.is_empty() {
            cancelled(cancel)?;
            let count = match self.parser.read_obu(input).map_err(error)? {
                ObuAction::Drop(count) => count as usize,
                ObuAction::Process(obu) => {
                    let count = obu.bytes_used;
                    if obu.header.obu_type == ObuType::SequenceHeader {
                        self.events(cancel, frames)?;
                        if let ParsedObu::SequenceHeader(sequence) =
                            self.parser.parse_obu(obu).map_err(error)?
                        {
                            if sequence.bit_depth != BitDepth::Depth8
                                || sequence.color_config.mono_chrome
                                || !sequence.color_config.subsampling_x
                                || !sequence.color_config.subsampling_y
                            {
                                return Err(error("only AV1 8-bit 4:2:0 output is supported"));
                            }
                            self.pending_color = Some(color(&sequence.color_config)?);
                        }
                    }
                    count
                }
            };
            let rest = stream::consumed(input, count)?;
            let mut unit = &input[..count];
            while !unit.is_empty() {
                cancelled(cancel)?;
                let pool = &mut self.pool;
                // Timestamps are opaque u64 tags to cros-codecs; preserve
                // signed microseconds bit-for-bit, including negative preroll.
                match self
                    .decoder
                    .decode(timestamp as u64, unit, &mut || pool.as_mut()?.alloc())
                {
                    Ok(n) => {
                        unit = stream::consumed(unit, n)?;
                        self.events(cancel, frames)?;
                    }
                    Err(DecodeError::CheckEvents) => {
                        if self.events(cancel, frames)? == 0 {
                            return Err(error("decoder requested an event but produced none"));
                        }
                    }
                    Err(DecodeError::NotEnoughOutputBuffers(_)) => {
                        if self.events(cancel, frames)? == 0 {
                            return Err(error("decoder exhausted its advertised surface pool"));
                        }
                    }
                    Err(e) => return Err(error(e)),
                }
            }
            input = rest;
        }
        Ok(())
    }
    pub fn flush(&mut self, cancel: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        cancelled(cancel)?;
        self.decoder.flush().map_err(error)?;
        let mut frames = Vec::new();
        self.events(cancel, &mut frames)?;
        Ok(frames)
    }
    fn events(
        &mut self,
        cancel: &AtomicBool,
        frames: &mut Vec<VideoFrame>,
    ) -> Result<usize, CodecError> {
        let mut count = 0;
        while let Some(event) = self.decoder.next_event() {
            cancelled(cancel)?;
            count += 1;
            match event {
                DecoderEvent::FormatChanged => {
                    let info = self
                        .decoder
                        .stream_info()
                        .ok_or_else(|| error("format event without stream information"))?
                        .clone();
                    self.resize(&info)?;
                    // Old-format frames are delivered before FormatChanged.
                    // Do not apply a new sequence's color matrix to those frames.
                    if let Some(color) = self.pending_color.take() {
                        self.color = Some(color);
                    }
                }
                DecoderEvent::FrameReady(handle) => {
                    handle.sync().map_err(error)?;
                    let frame = handle.video_frame();
                    let resolution = handle.display_resolution();
                    if frame.fourcc() != Fourcc::from(b"NV12") {
                        return Err(error("unexpected surface format"));
                    }
                    let pitches = frame.get_plane_pitch();
                    let mapping = frame.map().map_err(error)?;
                    let planes = mapping.get();
                    if pitches.len() < 2 || planes.len() < 2 {
                        return Err(error("NV12 surface is missing a plane"));
                    }
                    let (matrix, transfer) = self
                        .color
                        .ok_or_else(|| error("frame without a sequence color description"))?;
                    frames.push(planar_frame(
                        handle.timestamp() as i64,
                        resolution.width,
                        resolution.height,
                        [
                            Plane {
                                bytes: planes[0],
                                stride: pitches[0],
                                pixel_stride: 1,
                            },
                            Plane {
                                bytes: planes[1],
                                stride: pitches[1],
                                pixel_stride: 2,
                            },
                            Plane {
                                bytes: planes[1]
                                    .get(1..)
                                    .ok_or_else(|| error("empty chroma plane"))?,
                                stride: pitches[1],
                                pixel_stride: 2,
                            },
                        ],
                        matrix,
                        transfer,
                    )?);
                }
            }
        }
        Ok(count)
    }
    fn resize(&mut self, info: &StreamInfo) -> Result<(), CodecError> {
        if info.format != DecodedFormat::NV12
            || !(1..=64).contains(&info.min_num_frames)
            || info.display_resolution.width == 0
            || info.display_resolution.height == 0
        {
            return Err(error("unsupported stream format or surface count"));
        }
        let mut prepared = VecDeque::new();
        // Allocate fallibly before entering upstream's infallible pool callback.
        for _ in 0..info.min_num_frames {
            prepared.push_back(
                self.device
                    .clone()
                    .new_frame(
                        Fourcc::from(b"NV12"),
                        info.display_resolution,
                        info.coded_resolution,
                        GbmUsage::Decode,
                    )
                    .map_err(error)?,
            );
        }
        let mut pool = FramePool::new(move |_| {
            prepared
                .pop_front()
                .expect("one prepared frame per advertised pool slot")
        });
        pool.resize(info);
        self.pool = Some(pool);
        Ok(())
    }
}

fn color(config: &ColorConfig) -> Result<(YuvColorTransform, TransferFunction), CodecError> {
    use cros_codecs::codec::av1::parser::{MatrixCoefficients as M, TransferCharacteristics as T};
    let (kr, kb) = match config.matrix_coefficients {
        M::Bt709 | M::Unspecified => (0.2126, 0.0722),
        M::Bt601 | M::Bt470bg => (0.299, 0.114),
        M::Bt2020Ncl => (0.2627, 0.0593),
        _ => return Err(error("unsupported color matrix")),
    };
    let transfer = match config.transfer_characteristics {
        T::Bt709 | T::Bt601 | T::Unspecified | T::Bt202010Bit | T::Bt202012Bit => {
            TransferFunction::Bt1886
        }
        T::Linear => TransferFunction::Linear,
        T::Srgb => TransferFunction::Srgb,
        T::Bt470m => TransferFunction::Gamma22,
        T::Bt470bg => TransferFunction::Gamma28,
        _ => {
            return Err(error(
                "unsupported transfer function (HDR is not supported)",
            ));
        }
    };
    Ok((
        YuvColorTransform::from_luma_coefficients(kr, kb, !config.color_range),
        transfer,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cros_codecs::codec::av1::parser::{MatrixCoefficients, TransferCharacteristics};
    #[test]
    fn sdr_range_is_preserved_and_hdr_is_rejected() {
        let mut config = ColorConfig::default();
        config.matrix_coefficients = MatrixCoefficients::Bt709;
        config.transfer_characteristics = TransferCharacteristics::Bt709;
        config.color_range = false;
        let (limited, transfer) = color(&config).expect("SDR color");
        assert_eq!(transfer, TransferFunction::Bt1886);
        config.color_range = true;
        let (full, _) = color(&config).expect("full range");
        assert_eq!(full.luma, [1.0, 0.0]);
        assert!((16.0 / 255.0 * limited.luma[0] + limited.luma[1]).abs() < 1e-6);
        assert!((235.0 / 255.0 * limited.luma[0] + limited.luma[1] - 1.0).abs() < 1e-6);
        assert_ne!(limited.row_r[0], full.row_r[0]);
        config.transfer_characteristics = TransferCharacteristics::Smpte2084;
        assert!(color(&config).is_err());
        config.transfer_characteristics = TransferCharacteristics::Hlg;
        assert!(color(&config).is_err());
    }
    #[test]
    fn signed_timestamps_survive_opaque_driver_tags() {
        for timestamp in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(timestamp as u64 as i64, timestamp);
        }
    }
}
