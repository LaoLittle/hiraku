use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use crate::*;
use super::software;

#[cfg(test)]
mod tests;

trait Processor<F> {
    fn decode(&mut self, chunk: EncodedChunk, cancelled: &AtomicBool) -> Result<Vec<F>, CodecError>;
    fn flush(&mut self, cancelled: &AtomicBool) -> Result<Vec<F>, CodecError>;
}
trait Configuration<F>: Clone + Send + 'static {
    fn open(self) -> Result<Box<dyn Processor<F>>, CodecError>;
}
enum Command<C> { Configure(C), Decode(EncodedChunk), Flush(FlushId) }

struct Worker<C, F> {
    commands: Sender<Command<C>>,
    events: Receiver<DecoderEvent<F>>,
    cancel: Sender<()>,
    cancelled: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
}
impl<C, F> Drop for Worker<C, F> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.cancel.try_send(());
    }
}
impl<C: Configuration<F>, F: Send + 'static> Worker<C, F> {
    fn new() -> Result<Self, CodecError> {
        let (commands, input) = unbounded();
        let (output, events) = bounded(32);
        let (cancel, cancelled_event) = bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(AtomicUsize::new(0));
        let count = queued.clone();
        let flag = cancelled.clone();
        std::thread::Builder::new().name("hiraku-codec".into()).spawn(move || {
            let mut codec: Option<Box<dyn Processor<F>>> = None;
            let send = |event| {
                crossbeam_channel::select_biased! {
                    recv(cancelled_event) -> _ => false,
                    send(output, event) -> result => result.is_ok(),
                }
            };
            loop {
                let command = crossbeam_channel::select_biased! {
                    recv(cancelled_event) -> _ => break,
                    recv(input) -> command => match command { Ok(command) => command, Err(_) => break },
                };
                let result: Result<(), CodecError> = (|| {
                    match command {
                        Command::Configure(config) => { codec = Some(C::open(config)?); }
                        Command::Decode(chunk) => {
                            count.fetch_sub(1, Ordering::AcqRel);
                            let codec = codec.as_mut().ok_or(CodecError::InvalidState("backend is not configured"))?;
                            for frame in codec.decode(chunk, &flag)? {
                                if !send(DecoderEvent::Output(frame)) { return Err(CodecError::InvalidState("decoder cancelled")); }
                            }
                        }
                        Command::Flush(id) => {
                            let codec = codec.as_mut().ok_or(CodecError::InvalidState("backend is not configured"))?;
                            for frame in codec.flush(&flag)? {
                                if !send(DecoderEvent::Output(frame)) { return Err(CodecError::InvalidState("decoder cancelled")); }
                            }
                            if !send(DecoderEvent::Flushed(id)) { return Err(CodecError::InvalidState("decoder cancelled")); }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result { let _ = send(DecoderEvent::Error(error)); break; }
            }
        }).map_err(|e| CodecError::Operation(format!("failed to spawn codec worker: {e}")))?;
        Ok(Self { commands, events, cancel, cancelled, queued })
    }
    fn send(&self, command: Command<C>) -> Result<(), CodecError> {
        self.commands.send(command).map_err(|_| CodecError::InvalidState("codec worker has exited"))
    }
}

macro_rules! decoder {
    ($name:ident, $config:ty, $frame:ty) => {
        pub(crate) struct $name { worker: Option<Worker<$config, $frame>> }
        impl $name {
            pub fn new() -> Result<Self, CodecError> { Ok(Self { worker: None }) }
            pub fn configure(&mut self, config: $config) -> Result<(), CodecError> {
                if self.worker.is_none() { self.worker = Some(Worker::new()?); }
                self.worker.as_ref().expect("worker was initialized").send(Command::Configure(config))
            }
            pub fn decode(&mut self, chunk: EncodedChunk) -> Result<(), CodecError> {
                let worker = self.worker.as_ref().ok_or(CodecError::InvalidState("worker is absent"))?;
                worker.queued.fetch_add(1, Ordering::AcqRel);
                if let Err(error) = worker.send(Command::Decode(chunk)) {
                    worker.queued.fetch_sub(1, Ordering::AcqRel);
                    return Err(error);
                }
                Ok(())
            }
            pub fn flush(&mut self, id: FlushId) -> Result<(), CodecError> {
                self.worker.as_ref().ok_or(CodecError::InvalidState("worker is absent"))?.send(Command::Flush(id))
            }
            pub fn poll(&mut self) -> Option<DecoderEvent<$frame>> {
                self.worker.as_ref()?.events.try_recv().ok()
            }
            pub fn decode_queue_size(&self) -> usize {
                self.worker.as_ref().map_or(0, |w| w.queued.load(Ordering::Acquire))
            }
            pub fn pending_output(&self) -> usize { self.worker.as_ref().map_or(0, |w| w.events.len()) }
            pub fn close(&mut self) { self.worker = None; }
        }
    };
}
decoder!(VideoDecoder, VideoDecoderConfig, VideoFrame);
decoder!(AudioDecoder, AudioDecoderConfig, AudioData);

enum VideoCodec {
    Software(software::Video),
    #[cfg(all(target_os = "macos", feature = "hardware"))]
    VideoToolbox(super::macos::VideoToolboxDecoder),
    #[cfg(all(target_os = "windows", feature = "hardware"))]
    MediaFoundation(super::windows::MediaFoundationDecoder),
}

impl Configuration<VideoFrame> for VideoDecoderConfig {
    fn open(self) -> Result<Box<dyn Processor<VideoFrame>>, CodecError> {
        if !software::supports_video(&self) { return Err(CodecError::Unsupported(self.codec.0.clone())); }
        #[cfg(all(target_os = "macos", feature = "hardware"))]
        if self.hardware_acceleration != HardwareAcceleration::PreferSoftware
            && super::macos::av1_hardware_decode_supported()
            && let Some(description) = &self.description
            && let Ok(decoder) = super::macos::VideoToolboxDecoder::new(self.coded_width, self.coded_height, description)
        {
            return Ok(Box::new(VideoCodec::VideoToolbox(decoder)));
        }
        #[cfg(all(target_os = "windows", feature = "hardware"))]
        if self.hardware_acceleration != HardwareAcceleration::PreferSoftware
            && let Ok(decoder) = super::windows::MediaFoundationDecoder::new(self.coded_width, self.coded_height)
        {
            return Ok(Box::new(VideoCodec::MediaFoundation(decoder)));
        }
        Ok(Box::new(VideoCodec::Software(software::Video::new(&self)?)))
    }
}
impl Processor<VideoFrame> for VideoCodec {
    fn decode(&mut self, chunk: EncodedChunk, _cancelled: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        match self {
            Self::Software(codec) => codec.decode(chunk),
            #[cfg(all(target_os = "macos", feature = "hardware"))]
            Self::VideoToolbox(codec) => codec.decode(&chunk.data, chunk.timestamp, chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64, 1, 1_000_000).map_err(CodecError::Operation),
            #[cfg(all(target_os = "windows", feature = "hardware"))]
            Self::MediaFoundation(codec) => codec.decode(&chunk.data, chunk.timestamp, chunk.duration.unwrap_or(0).min(i64::MAX as u64) as i64, 1, 1_000_000, _cancelled).map_err(CodecError::Operation),
        }
    }
    fn flush(&mut self, _cancelled: &AtomicBool) -> Result<Vec<VideoFrame>, CodecError> {
        match self {
            Self::Software(codec) => codec.flush(),
            #[cfg(all(target_os = "macos", feature = "hardware"))]
            Self::VideoToolbox(codec) => codec.finish().map_err(CodecError::Operation),
            #[cfg(all(target_os = "windows", feature = "hardware"))]
            Self::MediaFoundation(codec) => codec.finish(_cancelled).map_err(CodecError::Operation),
        }
    }
}
impl Configuration<AudioData> for AudioDecoderConfig {
    fn open(self) -> Result<Box<dyn Processor<AudioData>>, CodecError> { Ok(Box::new(software::Audio::new(self)?)) }
}
impl Processor<AudioData> for software::Audio {
    fn decode(&mut self, chunk: EncodedChunk, _: &AtomicBool) -> Result<Vec<AudioData>, CodecError> { self.decode(chunk) }
    fn flush(&mut self, _: &AtomicBool) -> Result<Vec<AudioData>, CodecError> { self.flush() }
}
pub(crate) async fn video_config_supported(config: &VideoDecoderConfig) -> Result<bool, CodecError> {
    Ok(software::supports_video(config))
}
pub(crate) async fn audio_config_supported(config: &AudioDecoderConfig) -> Result<bool, CodecError> {
    Ok(software::supports_audio(config))
}
