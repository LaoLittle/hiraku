mod video_toolbox;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crossbeam_channel::{SendTimeoutError, Sender, bounded};
use opus_rs::OpusDecoder;
use symphonia::core::codecs::{
    audio::well_known as audio_codecs, video::well_known as video_codecs,
};

use self::video_toolbox::VideoToolboxDecoder;
use crate::{
    AudioEvent, DecodeSettings, DecodeStream, EncodedMedia, MediaMetadata, VideoEvent,
    container::open_container,
};

const VIDEO_QUEUE_CAPACITY: usize = 3;
const AUDIO_QUEUE_CAPACITY: usize = 24;

pub(crate) struct DecoderHandle;

pub(crate) fn decode(media: &EncodedMedia, settings: &DecodeSettings) -> DecodeStream {
    if !video_toolbox::av1_hardware_decode_supported() {
        return super::native::decode(media, settings);
    }
    
    let (video_sender, video_receiver) = bounded(VIDEO_QUEUE_CAPACITY);
    let (audio_sender, audio_receiver) = bounded(AUDIO_QUEUE_CAPACITY);
    let bytes = media.bytes.clone();
    let metadata = media.metadata;
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancellation = cancellation.clone();

    std::thread::Builder::new()
        .name("hiraku-videotoolbox-decoder".into())
        .spawn(move || {
            if let Err(error) = decode_stream(
                bytes,
                metadata,
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
        .expect("failed to spawn the VideoToolbox decoder thread");

    DecodeStream {
        video: video_receiver,
        audio: audio_receiver,
        metadata,
        cancellation: cancellation.clone(),
        queued_frames: None,
        handle: crate::DecoderHandle(super::DecoderHandle::VideoToolbox(DecoderHandle)),
    }
}

fn decode_stream(
    bytes: Arc<[u8]>,
    metadata: MediaMetadata,
    video_sender: &Sender<VideoEvent>,
    audio_sender: &Sender<AudioEvent>,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut format = open_container(bytes, "mkv").map_err(|error| error.to_string())?;

    let mut video_track = None;
    let mut audio_track = None;
    let mut video_time_base = None;
    let mut av1c = None;

    for track in format.tracks() {
        let Some(parameters) = &track.codec_params else {
            continue;
        };

        if let Some(video) = parameters.video()
            && video.codec == video_codecs::CODEC_ID_AV1
        {
            video_track = Some(track.id);
            video_time_base = track
                .time_base
                .map(|base| (base.numer.get(), base.denom.get()));

            // Symphonia 0.6.1 exposes AV1 decoder configuration as VideoExtraData.
            // The user's current Matroska files expose exactly one AV1 decoder config entry.
            // If the exact well-known AV1 extra-data ID is enabled in your Symphonia build, prefer
            // filtering by that ID here; `.first()` preserves the already-tested PoC behavior.
            av1c = video.extra_data.first().map(|extra| extra.data.clone());
        }

        if let Some(audio) = parameters.audio()
            && audio.codec == audio_codecs::CODEC_ID_OPUS
        {
            audio_track = Some(track.id);
        }
    }

    let video_track = video_track.ok_or_else(|| "AV1 video track disappeared".to_string())?;
    let audio_track = audio_track.ok_or_else(|| "Opus audio track disappeared".to_string())?;
    let (time_base_numer, time_base_denom) =
        video_time_base.ok_or_else(|| "AV1 track has no time base".to_string())?;
    let av1c = av1c.ok_or_else(|| "AV1 track has no decoder configuration (av1C)".to_string())?;

    let mut video_decoder = VideoToolboxDecoder::new(metadata.width, metadata.height, &av1c)?;
    let mut audio_decoder = OpusDecoder::new(metadata.sample_rate as i32, metadata.channels.into())
        .map_err(|error| format!("failed to initialize Opus decoder: {error}"))?;
    let mut first_video_timestamp = None;

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("video playback was cancelled".into());
        }

        match format.next_packet() {
            Ok(Some(packet)) if packet.track_id == video_track => {
                let frames = video_decoder.decode(
                    &packet.data,
                    packet.pts.get(),
                    packet.dur.get() as i64,
                    time_base_numer,
                    time_base_denom,
                )?;
                send_video_frames(
                    frames,
                    video_sender,
                    &mut first_video_timestamp,
                    cancellation,
                )?;
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

    let frames = video_decoder.finish()?;
    send_video_frames(
        frames,
        video_sender,
        &mut first_video_timestamp,
        cancellation,
    )?;

    send_cancellable(video_sender, VideoEvent::End, cancellation)?;
    send_cancellable(audio_sender, AudioEvent::End, cancellation)?;
    Ok(())
}

fn send_video_frames(
    frames: Vec<crate::VideoFrame>,
    sender: &Sender<VideoEvent>,
    first_timestamp: &mut Option<Duration>,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    for mut frame in frames {
        let first = *first_timestamp.get_or_insert(frame.timestamp);
        frame.timestamp = frame.timestamp.saturating_sub(first);
        send_cancellable(sender, VideoEvent::Frame(frame), cancellation)?;
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
