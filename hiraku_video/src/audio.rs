use std::{
    num::{NonZeroU16, NonZeroU32},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::{VideoMetadata as MediaMetadata, decode::AudioEvent};
use bevy::{
    audio::{ChannelCount, Decodable, SampleRate, Source},
    prelude::Asset,
    reflect::TypePath,
};
use crossbeam_channel::{Receiver, TryRecvError};

#[derive(Asset, Clone, TypePath)]
pub(crate) struct VideoAudio {
    receiver: Receiver<AudioEvent>,
    metadata: MediaMetadata,
    played_samples: Arc<AtomicU64>,
}

impl VideoAudio {
    pub fn new(receiver: Receiver<AudioEvent>, metadata: MediaMetadata) -> Self {
        Self {
            receiver,
            metadata,
            played_samples: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(
            self.played_samples.load(Ordering::Relaxed) as f64
                / f64::from(self.metadata.sample_rate)
                / f64::from(self.metadata.channels),
        )
    }
}

pub(crate) struct VideoAudioDecoder {
    receiver: Receiver<AudioEvent>,
    current: Arc<[f32]>,
    offset: usize,
    silence_remaining: usize,
    played_samples: Arc<AtomicU64>,
    metadata: MediaMetadata,
    ended: bool,
}

impl Iterator for VideoAudioDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.silence_remaining != 0 {
            self.silence_remaining -= 1;
            return Some(0.0);
        }
        loop {
            if let Some(sample) = self.current.get(self.offset).copied() {
                self.offset += 1;
                self.played_samples.fetch_add(1, Ordering::Relaxed);
                return Some(sample);
            }
            if self.ended {
                return None;
            }
            match self.receiver.try_recv() {
                Ok(AudioEvent::Samples(samples)) => {
                    self.current = samples;
                    self.offset = 0;
                }
                Ok(AudioEvent::End) | Err(TryRecvError::Disconnected) => self.ended = true,
                // Browser decoding progresses on the event loop. Never block
                // the audio callback waiting for it; silence does not advance
                // the media clock, so video also waits during underruns.
                Err(TryRecvError::Empty) => {
                    self.silence_remaining = usize::from(self.metadata.channels) - 1;
                    return Some(0.0);
                }
            }
        }
    }
}

impl Source for VideoAudioDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(
            self.current
                .len()
                .saturating_sub(self.offset)
                .max(usize::from(self.metadata.channels)),
        )
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
            current: Arc::from([]),
            offset: 0,
            silence_remaining: 0,
            played_samples: self.played_samples.clone(),
            metadata: self.metadata,
            ended: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underrun_is_nonblocking_and_preserves_stereo_alignment_and_clock() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let audio = VideoAudio::new(
            receiver,
            MediaMetadata {
                width: 1,
                height: 1,
                sample_rate: 48000,
                channels: 2,
            },
        );
        let mut decoder = audio.decoder();
        assert_eq!(decoder.next(), Some(0.0));
        sender
            .send(AudioEvent::Samples(Arc::from([0.25, -0.25])))
            .expect("PCM receiver");
        // Finish the silent stereo frame before starting the newly arrived PCM.
        assert_eq!(decoder.next(), Some(0.0));
        assert_eq!(audio.position(), Duration::ZERO);
        assert_eq!(decoder.next(), Some(0.25));
        assert_eq!(decoder.next(), Some(-0.25));
        assert_eq!(audio.played_samples.load(Ordering::Relaxed), 2);
        sender.send(AudioEvent::End).expect("PCM receiver");
        assert_eq!(decoder.next(), None);
    }
}
