use std::{
    num::{NonZeroU16, NonZeroU32},
    time::Duration,
};

use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::Asset,
    reflect::TypePath,
};
use crossbeam_channel::Receiver;
use hiraku_media::{AudioEvent, MediaMetadata};

#[derive(Asset, Clone, TypePath)]
pub(crate) struct VideoAudio {
    receiver: Receiver<AudioEvent>,
    metadata: MediaMetadata,
}

impl VideoAudio {
    pub fn new(receiver: Receiver<AudioEvent>, metadata: MediaMetadata) -> Self {
        Self { receiver, metadata }
    }
}

pub(crate) struct VideoAudioDecoder {
    receiver: Receiver<AudioEvent>,
    current: std::vec::IntoIter<f32>,
    metadata: MediaMetadata,
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
                Ok(AudioEvent::End | AudioEvent::Error(_)) | Err(_) => self.ended = true,
            }
        }
    }
}

impl Source for VideoAudioDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.current.len().max(1))
    }
    fn channels(&self) -> ChannelCount {
        NonZeroU16::new(self.metadata.channels).expect("validated media channels are non-zero")
    }
    fn sample_rate(&self) -> SampleRate {
        NonZeroU32::new(self.metadata.sample_rate).expect("validated media sample rate is non-zero")
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
