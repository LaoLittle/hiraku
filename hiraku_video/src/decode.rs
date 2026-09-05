//! Optional container-to-codec adapter, driven by the host's update loop.
use crate::asset::EncodedMedia;
use crossbeam_channel::{Receiver, Sender, unbounded};
use hiraku_media::*;

#[derive(Debug)]
pub(crate) enum VideoEvent {
    Frame(VideoFrame),
    End,
    Error(String),
}

#[derive(Debug)]
pub(crate) enum AudioEvent {
    Samples(std::sync::Arc<[f32]>),
    End,
}

use crate::container::{DemuxedChunk, MatroskaDemuxer};

pub(crate) struct MediaDecoder {
    pub video: Receiver<VideoEvent>,
    pub audio: Receiver<AudioEvent>,
    video_sender: Sender<VideoEvent>,
    audio_sender: Sender<AudioEvent>,
    demuxer: MatroskaDemuxer,
    video_decoder: VideoDecoder,
    audio_decoder: AudioDecoder,
    pending: Option<DemuxedChunk>,
    flushing: bool,
    failed: bool,
    first_timestamp: Option<i64>,
}

impl MediaDecoder {
    pub fn new(media: &EncodedMedia, settings: DecodeSettings) -> Result<Self, CodecError> {
        let mut demuxer = MatroskaDemuxer::new(media.bytes.clone(), "mkv")
            .map_err(|e| CodecError::Operation(e.to_string()))?;
        demuxer.video_config.software = settings;
        let mut video_decoder = VideoDecoder::new()?;
        let mut audio_decoder = AudioDecoder::new()?;
        video_decoder.configure(demuxer.video_config.clone())?;
        audio_decoder.configure(demuxer.audio_config.clone())?;
        let (video_sender, video) = unbounded();
        let (audio_sender, audio) = unbounded();
        Ok(Self {
            video,
            audio,
            video_sender,
            audio_sender,
            demuxer,
            video_decoder,
            audio_decoder,
            pending: None,
            flushing: false,
            failed: false,
            first_timestamp: None,
        })
    }
    
    /// Nonblocking. Queue limits bound decode-ahead even while playback is paused.
    pub fn poll(&mut self) {
        if self.failed {
            return;
        }
        
        if let Err(error) = self.pump() {
            self.failed = true;
            self.video_decoder.close();
            self.audio_decoder.close();
            let _ = self.video_sender.send(VideoEvent::Error(error.to_string()));
            let _ = self.audio_sender.send(AudioEvent::End);
        }
    }
    
    fn pump(&mut self) -> Result<(), CodecError> {
        while self.video.len() < 3 {
            let Some(event) = self.video_decoder.poll() else {
                break;
            };
            
            match event {
                DecoderEvent::Output(mut frame) => {
                    let first = *self.first_timestamp.get_or_insert(frame.timestamp);
                    frame.timestamp = frame.timestamp.saturating_sub(first);
                    let _ = self.video_sender.send(VideoEvent::Frame(frame));
                }
                DecoderEvent::Flushed(_) => {
                    let _ = self.video_sender.send(VideoEvent::End);
                }
                DecoderEvent::Error(e) => return Err(e),
            }
        }
        
        while self.audio.len() < 24 {
            let Some(event) = self.audio_decoder.poll() else {
                break;
            };
            match event {
                DecoderEvent::Output(data) => {
                    let expected = &self.demuxer.audio_config;
                    if data.sample_rate != expected.sample_rate
                        || data.number_of_channels != expected.number_of_channels
                    {
                        return Err(CodecError::Operation(
                            "decoded audio format changed during playback".into(),
                        ));
                    }
                    let _ = self.audio_sender.send(AudioEvent::Samples(data.samples));
                }
                DecoderEvent::Flushed(_) => {
                    let _ = self.audio_sender.send(AudioEvent::End);
                }
                DecoderEvent::Error(e) => return Err(e),
            }
        }
        
        if self.flushing {
            return Ok(());
        }
        
        // Keep demux work bounded per ECS update as well as decoder queue size.
        for _ in 0..128 {
            if self.pending.is_none() {
                self.pending = self
                    .demuxer
                    .next_chunk()
                    .map_err(|e| CodecError::Operation(e.to_string()))?;
            }
            
            let Some(chunk) = self.pending.as_ref() else {
                self.video_decoder.flush()?;
                self.audio_decoder.flush()?;
                self.flushing = true;
                return Ok(());
            };
            
            let full = match chunk {
                DemuxedChunk::Video(_) => {
                    self.video.len()
                        + self.video_decoder.pending_output()
                        + self.video_decoder.decode_queue_size()
                        >= 3
                }
                DemuxedChunk::Audio(_) => {
                    self.audio.len()
                        + self.audio_decoder.pending_output()
                        + self.audio_decoder.decode_queue_size()
                        >= 24
                }
            };
            
            if full {
                return Ok(());
            }
            
            match self.pending.take().expect("pending chunk was inspected") {
                DemuxedChunk::Video(chunk) => self.video_decoder.decode(chunk)?,
                DemuxedChunk::Audio(chunk) => self.audio_decoder.decode(chunk)?,
            }
        }
        
        Ok(())
    }
}
