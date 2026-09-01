use std::{
    collections::VecDeque,
    num::{NonZeroU16, NonZeroU32},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::Asset,
    reflect::TypePath,
    tasks::AsyncComputeTaskPool,
};
use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};
use opus_rs::OpusDecoder;
use rav1d::{Decoder as Av1Decoder, PixelLayout, PlanarImageComponent, Rav1dError, Settings};
use symphonia::core::codecs::{
    audio::well_known as audio_codecs, video::well_known as video_codecs,
};

use crate::{
    VideoAsset, VideoMetadata,
    asset::open_container,
    color::{
        TRANSFER_BT709, TRANSFER_GAMMA_22, TRANSFER_GAMMA_28, TRANSFER_LINEAR, TRANSFER_SRGB,
        YuvColorTransform,
    },
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const AUDIO_QUEUE_CAPACITY: usize = 24;

#[derive(Debug)]
pub(crate) struct DecodedFrame {
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub color_transform: YuvColorTransform,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum DecodeEvent {
    Frame(DecodedFrame),
    End,
    Error(String),
}

enum AudioEvent {
    Samples(Vec<f32>),
    End,
}

#[derive(Asset, Clone, TypePath)]
pub(crate) struct VideoAudio {
    receiver: Receiver<AudioEvent>,
    metadata: VideoMetadata,
}

pub(crate) struct VideoAudioDecoder {
    receiver: Receiver<AudioEvent>,
    current: std::vec::IntoIter<f32>,
    metadata: VideoMetadata,
    ended: bool,
}

impl Iterator for VideoAudioDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(sample) = self.current.next() {
                return Some(sample);
            }
            if self.ended {
                return None;
            }
            match self.receiver.recv() {
                Ok(AudioEvent::Samples(samples)) => self.current = samples.into_iter(),
                Ok(AudioEvent::End) | Err(_) => self.ended = true,
            }
        }
    }
}

impl Source for VideoAudioDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.current.len().max(1))
    }

    fn channels(&self) -> ChannelCount {
        NonZeroU16::new(self.metadata.channels).expect("validated video channels are non-zero")
    }

    fn sample_rate(&self) -> SampleRate {
        NonZeroU32::new(self.metadata.sample_rate).expect("validated video sample rate is non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Decodable for VideoAudio {
    type Decoder = VideoAudioDecoder;

    fn decoder(&self) -> Self::Decoder {
        VideoAudioDecoder {
            receiver: self.receiver.clone(),
            current: Vec::new().into_iter(),
            metadata: self.metadata,
            ended: false,
        }
    }
}

pub(crate) struct DecodeStream {
    pub video: Receiver<DecodeEvent>,
    pub audio: VideoAudio,
    pub cancellation: Arc<AtomicBool>,
}

pub(crate) fn spawn_decoder(asset: &VideoAsset) -> DecodeStream {
    let (video_sender, video_receiver) = bounded(VIDEO_QUEUE_CAPACITY);
    let (audio_sender, audio_receiver) = bounded(AUDIO_QUEUE_CAPACITY);
    let bytes = asset.bytes.clone();
    let metadata = asset.metadata;
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancellation = cancellation.clone();
    AsyncComputeTaskPool::get()
        .spawn(async move {
            if let Err(error) = decode_stream(
                bytes,
                metadata,
                &video_sender,
                &audio_sender,
                &worker_cancellation,
            ) {
                if !worker_cancellation.load(Ordering::Relaxed) {
                    let _ = video_sender.try_send(DecodeEvent::Error(error));
                }
                let _ = audio_sender.try_send(AudioEvent::End);
            }
        })
        .detach();
    DecodeStream {
        video: video_receiver,
        audio: VideoAudio {
            receiver: audio_receiver,
            metadata,
        },
        cancellation,
    }
}

fn decode_stream(
    bytes: Arc<[u8]>,
    metadata: VideoMetadata,
    video_sender: &Sender<DecodeEvent>,
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
    let decoder_threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    settings.set_n_threads(u32::try_from(decoder_threads).unwrap_or(u32::MAX));
    settings.set_max_frame_delay(VIDEO_QUEUE_CAPACITY as u32);
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
    send_cancellable(video_sender, DecodeEvent::End, cancellation)?;
    send_cancellable(audio_sender, AudioEvent::End, cancellation)?;
    Ok(())
}

fn drain_pictures(
    decoder: &mut Av1Decoder,
    sender: &Sender<DecodeEvent>,
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
            DecodeEvent::Frame(picture_to_yuv420(
                &picture,
                timestamp.saturating_sub(first),
            )?),
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

fn picture_to_yuv420(
    picture: &rav1d::Picture,
    timestamp: Duration,
) -> Result<DecodedFrame, String> {
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
    let copy_plane = |component, plane_width: u32, plane_height: u32| {
        let source = picture.plane(component);
        let stride = picture.stride(component) as usize;
        let row_width = plane_width as usize;
        let mut output = Vec::with_capacity(row_width * plane_height as usize);
        for row in 0..plane_height as usize {
            let start = row * stride;
            output.extend_from_slice(&source[start..start + row_width]);
        }
        output
    };
    Ok(DecodedFrame {
        timestamp,
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform: picture_color_transform(picture),
        y: copy_plane(PlanarImageComponent::Y, width, height),
        u: copy_plane(PlanarImageComponent::U, chroma_width, chroma_height),
        v: copy_plane(PlanarImageComponent::V, chroma_width, chroma_height),
    })
}

fn picture_color_transform(picture: &rav1d::Picture) -> YuvColorTransform {
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
    let transfer = match picture.transfer_characteristic() {
        TransferCharacteristic::Linear => TRANSFER_LINEAR,
        TransferCharacteristic::SRGB => TRANSFER_SRGB,
        TransferCharacteristic::BT470M => TRANSFER_GAMMA_22,
        TransferCharacteristic::BT470BG => TRANSFER_GAMMA_28,
        TransferCharacteristic::BT1886
        | TransferCharacteristic::Unspecified
        | TransferCharacteristic::Reserved0
        | TransferCharacteristic::Reserved
        | TransferCharacteristic::ST170M
        | TransferCharacteristic::ST240M
        | TransferCharacteristic::XVYCC
        | TransferCharacteristic::BT1361E
        | TransferCharacteristic::BT2020Ten
        | TransferCharacteristic::BT2020Twelve
        | TransferCharacteristic::Logarithmic100
        | TransferCharacteristic::Logarithmic316
        | TransferCharacteristic::PerceptualQuantizer
        | TransferCharacteristic::ST428
        | TransferCharacteristic::HybridLogGamma => TRANSFER_BT709,
    };
    YuvColorTransform::from_luma_coefficients(
        kr,
        kb,
        picture.color_range() == YUVRange::Limited,
        transfer,
    )
}

pub(crate) fn drain_ready_frames(
    receiver: &Receiver<DecodeEvent>,
    queue: &mut VecDeque<DecodedFrame>,
) -> Option<Result<(), String>> {
    let mut terminal = None;
    while queue.len() < VIDEO_QUEUE_CAPACITY {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        match event {
            DecodeEvent::Frame(frame) => queue.push_back(frame),
            DecodeEvent::End => terminal = Some(Ok(())),
            DecodeEvent::Error(error) => terminal = Some(Err(error)),
        }
    }
    terminal
}
