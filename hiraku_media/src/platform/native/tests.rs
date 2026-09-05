use crate::*;
use std::{sync::Arc, time::{Duration, Instant}};
use futures_lite::future::block_on;
// Synthetic 64x64 solid-red AV1 frames generated with SVT-AV1; no project assets.
const AV1: &[&[u8]] = &[
    &[0x12, 0x00, 0x0a, 0x0b, 0x02, 0x00, 0x00, 0x05, 0x15, 0x7f, 0xfc, 0x4a, 0xf9, 0x00, 0x40, 0x32, 0x15, 0x10, 0x00, 0xb0, 0x82, 0x05, 0x14, 0x20, 0x81, 0x00, 0x00, 0x03, 0x25, 0x02, 0xab, 0x5a, 0x7f, 0x0c, 0x9e, 0x64, 0x28, 0xdc],
    &[0x12, 0x00, 0x32, 0x11, 0x28, 0x02, 0x00, 0x40, 0x00, 0x00, 0x11, 0x8c, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x9c, 0x10, 0x32, 0x11, 0x30, 0x02, 0x00, 0x00, 0x00, 0x49, 0x23, 0x18, 0x00, 0x00, 0x02, 0x00, 0x03, 0x00, 0x00, 0x9c, 0xe8],
    &[0x12, 0x00, 0x1a, 0x01, 0x98],
    &[0x12, 0x00, 0x32, 0x10, 0x30, 0x06, 0x00, 0x04, 0x92, 0x49, 0x23, 0x18, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x97, 0xc0],
];

fn opus(timestamp: i64) -> EncodedAudioChunk {
    EncodedAudioChunk(EncodedChunk { kind: ChunkType::Key, timestamp, duration: Some(20_000),
        data: Arc::from([0xf8, 0xff, 0xfe]) })
}
fn poll_audio(decoder: &mut AudioDecoder) -> DecoderEvent<AudioData> {
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(event) = decoder.poll() { return event; }
        assert!(Instant::now() < limit, "audio decoder did not produce an event");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn opus_output_precedes_flush_and_preserves_timestamps() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    decoder.configure(AudioDecoderConfig::new("opus", 48_000, 2)).expect("configure");
    let mut chunk = opus(-20_000);
    chunk.0.data = Arc::from([0xfc, 0xff, 0xfe]);
    decoder.decode(chunk).expect("decode");
    let flush = decoder.flush().expect("flush");
    let event = poll_audio(&mut decoder);
    let DecoderEvent::Output(data) = event else { panic!("output must precede flush: {event:?}") };
    assert_eq!(data.timestamp, -20_000);
    assert_eq!(data.number_of_frames(), 960);
    assert!(matches!(poll_audio(&mut decoder), DecoderEvent::Flushed(id) if id == flush));
    assert_eq!(decoder.decode_queue_size(), 0);
}
#[test]
fn reset_discards_output_and_flushes_without_reusing_ids() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    decoder.configure(AudioDecoderConfig::new("opus", 48_000, 1)).expect("configure");
    decoder.decode(opus(1)).expect("decode");
    let old_flush = decoder.flush().expect("flush");
    decoder.reset().expect("reset");
    assert_eq!(decoder.state(), CodecState::Unconfigured);
    assert_eq!(decoder.decode_queue_size(), 0);
    assert!(decoder.poll().is_none());
    assert!(decoder.decode(opus(2)).is_err());
    decoder.configure(AudioDecoderConfig::new("opus", 48_000, 1)).expect("reconfigure");
    decoder.decode(opus(42)).expect("decode");
    let flush = decoder.flush().expect("flush");
    assert_ne!(flush, old_flush);
    let DecoderEvent::Output(data) = poll_audio(&mut decoder) else { panic!("expected output") };
    assert_eq!(data.timestamp, 42);
    assert!(matches!(poll_audio(&mut decoder), DecoderEvent::Flushed(id) if id == flush));
    decoder.close();
    assert_eq!(decoder.state(), CodecState::Closed);
    assert!(decoder.reset().is_err());
    assert!(decoder.configure(AudioDecoderConfig::new("opus", 48_000, 1)).is_err());
}
#[test]
fn codec_support_is_backend_policy_and_unknown_codec_closes_on_error() {
    let config = AudioDecoderConfig::new("future-codec", 48000, 2);
    assert!(!block_on(AudioDecoder::is_config_supported(&config)).expect("query").supported);
    assert!(!block_on(VideoDecoder::is_config_supported(&VideoDecoderConfig::new("vp09.00.10.08", 64, 64))).expect("query").supported);
    assert!(block_on(AudioDecoder::is_config_supported(&AudioDecoderConfig::new("opus", 48000, 2))).expect("query").supported);
    let mut decoder = AudioDecoder::new().expect("decoder");
    decoder.configure(config).expect("configuration is queued");
    assert!(matches!(poll_audio(&mut decoder), DecoderEvent::Error(CodecError::Unsupported(_))));
    assert_eq!(decoder.state(), CodecState::Closed);
}
#[test]
fn key_requirement_applies_after_configure_and_flush() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    assert!(decoder.flush().is_err());
    decoder.configure(AudioDecoderConfig::new("opus", 48000, 2)).expect("configure");
    let mut delta = opus(0); delta.0.kind = ChunkType::Delta;
    assert_eq!(decoder.decode(delta.clone()), Err(CodecError::KeyRequired));
    decoder.decode(opus(0)).expect("key");
    decoder.decode(delta.clone()).expect("delta after key");
    decoder.flush().expect("flush");
    assert_eq!(decoder.decode(delta), Err(CodecError::KeyRequired));
}

#[test]
fn reset_unblocks_a_worker_with_a_full_output_queue() {
    let mut decoder = AudioDecoder::new().expect("decoder");
    decoder.configure(AudioDecoderConfig::new("opus", 48000, 1)).expect("configure");
    for timestamp in 0..100 { decoder.decode(opus(timestamp)).expect("enqueue"); }
    let limit = Instant::now() + Duration::from_secs(3);
    while decoder.pending_output() < 32 {
        assert!(Instant::now() < limit, "worker did not fill its bounded output queue");
        std::thread::yield_now();
    }
    decoder.reset().expect("reset full queue");
    decoder.configure(AudioDecoderConfig::new("opus", 48000, 1)).expect("reconfigure");
    decoder.decode(opus(1000)).expect("enqueue new generation");
    let DecoderEvent::Output(data) = poll_audio(&mut decoder) else { panic!("expected new output") };
    assert_eq!(data.timestamp, 1000);
}
#[test]
fn av1_delayed_frames_survive_flush() {
    let mut decoder = VideoDecoder::new().expect("decoder");
    let mut config = VideoDecoderConfig::new("av01.0.04M.08", 64, 64);
    config.hardware_acceleration = HardwareAcceleration::PreferSoftware;
    config.software = DecodeSettings { decoder_threads: Some(4), max_frame_delay: Some(3) };
    decoder.configure(config).expect("configure");
    for (index, bytes) in AV1.iter().enumerate() {
        decoder.decode(EncodedVideoChunk(EncodedChunk {
            kind: if index == 0 { ChunkType::Key } else { ChunkType::Delta },
            timestamp: index as i64 * 41667 - 41667, duration: Some(41667), data: Arc::from(*bytes),
        })).expect("decode");
    }
    let flush = decoder.flush().expect("flush");
    let mut timestamps = Vec::new();
    let limit = Instant::now() + Duration::from_secs(5);
    loop {
        match decoder.poll() {
            Some(DecoderEvent::Output(frame)) => {
                assert_eq!((frame.width, frame.height), (64, 64));
                timestamps.push(frame.timestamp);
            }
            Some(DecoderEvent::Flushed(id)) => { assert_eq!(id, flush); break; }
            Some(DecoderEvent::Error(error)) => panic!("{error}"),
            None => { assert!(Instant::now() < limit, "video flush timed out"); std::thread::sleep(Duration::from_millis(1)); }
        }
    }
    assert_eq!(timestamps.len(), 4, "drain must preserve delayed pictures");
    assert_eq!(timestamps, [-41667, 0, 41667, 83334]);
}
