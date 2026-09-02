use std::{
    collections::VecDeque,
    iter::Empty,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::Asset,
    reflect::TypePath,
};
use crossbeam_channel::{Receiver, bounded};

use crate::{
    VideoAsset, VideoMetadata,
    color::{TransferFunction, YuvColorTransform},
};

#[derive(Debug)]
pub(crate) struct DecodedFrame {
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub color_transform: YuvColorTransform,
    pub transfer: TransferFunction,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

#[derive(Debug)]
#[allow(dead_code, reason = "keeps the target-specific decoder ABI identical")]
pub(crate) enum DecodeEvent {
    Frame(DecodedFrame),
    End,
    Error(String),
}

#[derive(Asset, Clone, TypePath)]
pub(crate) struct VideoAudio {
    metadata: VideoMetadata,
}

pub(crate) struct EmptyVideoAudio {
    samples: Empty<f32>,
    metadata: VideoMetadata,
}

impl Iterator for EmptyVideoAudio {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        self.samples.next()
    }
}

impl Source for EmptyVideoAudio {
    fn current_span_len(&self) -> Option<usize> {
        Some(0)
    }

    fn channels(&self) -> ChannelCount {
        std::num::NonZeroU16::new(self.metadata.channels)
            .expect("validated video channels are non-zero")
    }

    fn sample_rate(&self) -> SampleRate {
        std::num::NonZeroU32::new(self.metadata.sample_rate)
            .expect("validated video sample rate is non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::ZERO)
    }
}

impl Decodable for VideoAudio {
    type Decoder = EmptyVideoAudio;

    fn decoder(&self) -> Self::Decoder {
        EmptyVideoAudio {
            samples: std::iter::empty(),
            metadata: self.metadata,
        }
    }
}

pub(crate) struct DecodeStream {
    pub video: Receiver<DecodeEvent>,
    pub audio: VideoAudio,
    pub cancellation: Arc<AtomicBool>,
}

pub(crate) fn spawn_decoder(asset: &VideoAsset) -> DecodeStream {
    let (sender, receiver) = bounded(1);
    sender
        .send(DecodeEvent::Error(
            "AV1 playback on Web requires the planned WebCodecs backend".into(),
        ))
        .expect("fresh decoder event receiver must be connected");
    DecodeStream {
        video: receiver,
        audio: VideoAudio {
            metadata: asset.metadata,
        },
        cancellation: Arc::new(AtomicBool::new(false)),
    }
}

pub(crate) fn drain_ready_frames(
    receiver: &Receiver<DecodeEvent>,
    _queue: &mut VecDeque<DecodedFrame>,
) -> Option<Result<(), String>> {
    match receiver.try_recv() {
        Ok(DecodeEvent::Error(error)) => Some(Err(error)),
        Ok(DecodeEvent::End) => Some(Ok(())),
        Ok(DecodeEvent::Frame(_)) | Err(_) => None,
    }
}
