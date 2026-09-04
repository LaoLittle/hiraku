use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crossbeam_channel::{SendTimeoutError, Sender, bounded};
use opus_rs::OpusDecoder;
use rav1d::{Decoder as Av1Decoder, PixelLayout, PlanarImageComponent, Rav1dError, Settings};
use symphonia::core::codecs::{
    audio::well_known as audio_codecs, video::well_known as video_codecs,
};

use crate::{
    AudioEvent, DecodeSettings, DecodeStream, EncodedMedia, MediaMetadata, TransferFunction,
    VideoEvent, VideoFrame, VideoPixels, YuvColorTransform,
    container::open_container,
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const AUDIO_QUEUE_CAPACITY: usize = 24;

pub(crate) struct DecoderHandle;

pub(crate) fn decode(media: &EncodedMedia, settings: &DecodeSettings) -> DecodeStream {
    let (video_sender, video_receiver) = bounded(VIDEO_QUEUE_CAPACITY);
    let (audio_sender, audio_receiver) = bounded(AUDIO_QUEUE_CAPACITY);
    let bytes = media.bytes.clone();
    let metadata = media.metadata;
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let (decoder_threads, max_frame_delay) = resolve_decode_settings(settings, available);
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancellation = cancellation.clone();
    std::thread::Builder::new()
        .name("hiraku-av1-decoder".into())
        .spawn(move || {
            if let Err(error) = decode_stream(
                bytes,
                metadata,
                decoder_threads,
                max_frame_delay,
                &video_sender,
                &audio_sender,
                &worker_cancellation,
            ) {
                if !worker_cancellation.load(Ordering::Relaxed) {
                    let _ = video_sender.try_send(VideoEvent::Error(error));
                }
                let _ = audio_sender.try_send(AudioEvent::End);
            }
        })
        .expect("failed to spawn the AV1 decoder thread");

    DecodeStream {
        video: video_receiver,
        audio: audio_receiver,
        metadata,
        cancellation,
        queued_frames: None,
        handle: crate::DecoderHandle(
            super::DecoderHandle::Software(DecoderHandle)
        ),
    }
}

fn resolve_decode_settings(settings: &DecodeSettings, available: usize) -> (u32, u32) {
    let automatic_threads = match available {
        0 | 1 => 1,
        2..=4 => 2,
        5..=8 => 4,
        9..=12 => 5,
        13..=16 => 6,
        _ => 8,
    };
    let threads = settings
        .decoder_threads
        .unwrap_or(automatic_threads)
        .clamp(1, 256);
    let automatic_delay = if available <= 4 { 2 } else { 3 };
    let frame_delay = settings
        .max_frame_delay
        .unwrap_or(automatic_delay)
        .max(1)
        .min(threads);
    (threads, frame_delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_decoder_parallelism_is_conservative() {
        let settings = DecodeSettings::default();
        assert_eq!(resolve_decode_settings(&settings, 2), (2, 2));
        assert_eq!(resolve_decode_settings(&settings, 4), (2, 2));
        assert_eq!(resolve_decode_settings(&settings, 8), (4, 3));
        assert_eq!(resolve_decode_settings(&settings, 16), (6, 3));
        assert_eq!(resolve_decode_settings(&settings, 64), (8, 3));
    }

    #[test]
    fn explicit_frame_delay_cannot_exceed_decoder_parallelism() {
        assert_eq!(
            resolve_decode_settings(&DecodeSettings { decoder_threads: Some(4), max_frame_delay: Some(20) }, 64),
            (4, 4)
        );
        assert_eq!(
            resolve_decode_settings(&DecodeSettings { decoder_threads: Some(1), max_frame_delay: Some(1) }, 64),
            (1, 1)
        );
    }
}

fn decode_stream(
    bytes: Arc<[u8]>,
    metadata: MediaMetadata,
    decoder_threads: u32,
    max_frame_delay: u32,
    video_sender: &Sender<VideoEvent>,
    audio_sender: &Sender<AudioEvent>,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut format = open_container(bytes, "mkv").map_err(|error| error.to_string())?;
    let mut video_track = None;
    let mut audio_track = None;
    let mut time_base = (1_u32, 1_u32);
    for track in format.tracks() {
        let Some(parameters) = &track.codec_params else {
            continue;
        };
        if let Some(video) = parameters.video()
            && video.codec == video_codecs::CODEC_ID_AV1
        {
            video_track = Some(track.id);
            if let Some(base) = track.time_base {
                time_base = (base.numer.get(), base.denom.get());
            }
        }
        if let Some(audio) = parameters.audio()
            && audio.codec == audio_codecs::CODEC_ID_OPUS
        {
            audio_track = Some(track.id);
        }
    }
    let video_track = video_track.ok_or_else(|| "AV1 video track disappeared".to_string())?;
    let audio_track = audio_track.ok_or_else(|| "Opus audio track disappeared".to_string())?;

    let mut settings = Settings::new();
    settings.set_n_threads(decoder_threads);
    settings.set_max_frame_delay(max_frame_delay);
    let mut video_decoder =
        Av1Decoder::with_settings(&settings).map_err(|error| error.to_string())?;
    let mut audio_decoder = OpusDecoder::new(metadata.sample_rate as i32, metadata.channels.into())
        .map_err(|error| format!("failed to initialize Opus decoder: {error}"))?;
    let mut first_timestamp = None;

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("video playback was cancelled".into());
        }
        match format.next_packet() {
            Ok(Some(packet)) if packet.track_id == video_track => {
                let mut result = video_decoder.send_data(
                    packet.data.to_vec().into_boxed_slice(),
                    None,
                    Some(packet.pts.get()),
                    Some(packet.dur.get() as i64),
                );
                loop {
                    drain_pictures(
                        &mut video_decoder,
                        video_sender,
                        time_base,
                        &mut first_timestamp,
                        cancellation,
                    )?;
                    match result {
                        Ok(()) => break,
                        Err(Rav1dError::TryAgain) => result = video_decoder.send_pending_data(),
                        Err(error) => return Err(format!("AV1 decode failed: {error}")),
                    }
                }
            }
            Ok(Some(packet)) if packet.track_id == audio_track => {
                let frame_size = (metadata.sample_rate / 1_000 * 120) as usize;
                let mut samples = vec![0.0; frame_size * usize::from(metadata.channels)];
                let decoded = audio_decoder
                    .decode(&packet.data, frame_size, &mut samples)
                    .map_err(|error| format!("Opus decode failed: {error}"))?;
                samples.truncate(decoded * usize::from(metadata.channels));
                send_cancellable(audio_sender, AudioEvent::Samples(samples), cancellation)?;
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => return Err(format!("Matroska demux failed: {error}")),
        }
    }
    video_decoder.flush();
    drain_pictures(
        &mut video_decoder,
        video_sender,
        time_base,
        &mut first_timestamp,
        cancellation,
    )?;
    send_cancellable(video_sender, VideoEvent::End, cancellation)?;
    send_cancellable(audio_sender, AudioEvent::End, cancellation)?;
    Ok(())
}

fn drain_pictures(
    decoder: &mut Av1Decoder,
    sender: &Sender<VideoEvent>,
    time_base: (u32, u32),
    first_timestamp: &mut Option<Duration>,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    while let Ok(picture) = decoder.get_picture() {
        let timestamp = picture.timestamp().unwrap_or(0);
        let seconds = timestamp as f64 * f64::from(time_base.0) / f64::from(time_base.1);
        let timestamp = Duration::from_secs_f64(seconds.max(0.0));
        let first = *first_timestamp.get_or_insert(timestamp);
        send_cancellable(
            sender,
            VideoEvent::Frame(picture_to_yuv420(picture, timestamp.saturating_sub(first))?),
            cancellation,
        )?;
    }
    Ok(())
}

fn send_cancellable<T>(
    sender: &Sender<T>,
    mut value: T,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("video playback was cancelled".into());
        }
        match sender.send_timeout(value, Duration::from_millis(20)) {
            Ok(()) => return Ok(()),
            Err(SendTimeoutError::Timeout(returned)) => value = returned,
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err("video playback was closed".into());
            }
        }
    }
}

fn picture_to_yuv420(picture: rav1d::Picture, timestamp: Duration) -> Result<VideoFrame, String> {
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
    picture: &rav1d::Picture,
) -> Result<(YuvColorTransform, TransferFunction), String> {
    use rav1d::pixel::{MatrixCoefficients, TransferCharacteristic, YUVRange};

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
