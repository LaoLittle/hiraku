mod bindgen;
use std::{cell::{Cell, RefCell}, collections::VecDeque, rc::Rc, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use crossbeam_channel::{Receiver, Sender, unbounded};
use js_sys::{Float32Array, Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PlaneLayout, VideoFrameCopyToOptions,
    VideoMatrixCoefficients, VideoPixelFormat, VideoTransferCharacteristics};
use crate::{AudioData, AudioDecoderConfig, CodecError, DecoderEvent, EncodedChunk, FlushId,
    HardwareAcceleration, TransferFunction, VideoDecoderConfig, VideoFrame, YuvColorTransform};
use bindgen as web;

pub(crate) struct VideoDecoder {
    handle: Option<Rc<VideoHandle>>,
    receiver: Option<Receiver<DecoderEvent<VideoFrame>>>,
}
struct VideoHandle {
    decoder: web::VideoDecoder,
    state: Rc<FrameCopyState>,
    _output: Closure<dyn FnMut(web_sys::VideoFrame)>,
    _error: Closure<dyn FnMut(JsValue)>,
}
impl Drop for VideoHandle {
    fn drop(&mut self) {
        self.state.cancellation.store(true, Ordering::Relaxed);
        let _ = self.decoder.close();
    }
}
impl VideoDecoder {
    pub fn new() -> Result<Self, CodecError> { Ok(Self { handle: None, receiver: None }) }
    pub fn configure(&mut self, config: VideoDecoderConfig) -> Result<(), CodecError> {
        if self.handle.is_none() {
            let (sender, receiver) = unbounded();
            let state = Rc::new(FrameCopyState {
                queue: RefCell::new(VecDeque::new()), running: Cell::new(false), pending: Cell::new(0),
                enqueued: Cell::new(0), completed: Cell::new(0),
                storage: RefCell::new(None), sender: sender.clone(), cancellation: Arc::new(AtomicBool::new(false)),
            });
            let output_state = state.clone();
            let output = Closure::wrap(Box::new(move |frame: web_sys::VideoFrame| {
                output_state.enqueue(frame);
            }) as Box<dyn FnMut(web_sys::VideoFrame)>);
            let error = Closure::wrap(Box::new(move |error: JsValue| {
                let _ = sender.send(DecoderEvent::Error(operation(error)));
            }) as Box<dyn FnMut(JsValue)>);
            let init = web::VideoDecoderInit::new(error.as_ref().unchecked_ref(), output.as_ref().unchecked_ref());
            let decoder = web::VideoDecoder::new(&init).map_err(operation)?;
            self.handle = Some(Rc::new(VideoHandle { decoder, state, _output: output, _error: error }));
            self.receiver = Some(receiver);
        }
        self.handle.as_ref().expect("decoder initialized").decoder.configure(&video_config(&config)).map_err(operation)
    }
    pub fn decode(&mut self, chunk: EncodedChunk) -> Result<(), CodecError> {
        let handle = self.handle.as_ref().ok_or(CodecError::InvalidState("browser decoder absent"))?;
        let bytes = Uint8Array::from(chunk.data.as_ref());
        let kind = match chunk.kind { crate::ChunkType::Key => web::EncodedVideoChunkType::Key,
            crate::ChunkType::Delta => web::EncodedVideoChunkType::Delta };
        let init = web::EncodedVideoChunkInit::new(bytes.unchecked_ref(), 0, kind);
        init.set_timestamp_f64(chunk.timestamp as f64);
        if let Some(duration) = chunk.duration { init.set_duration_f64(duration as f64); }
        handle.decoder.decode(&web::EncodedVideoChunk::new(&init).map_err(operation)?).map_err(operation)
    }
    pub fn flush(&mut self, id: FlushId) -> Result<(), CodecError> {
        let handle = self.handle.as_ref().ok_or(CodecError::InvalidState("browser decoder absent"))?.clone();
        let promise = handle.decoder.flush();
        spawn_local(async move {
            let result = JsFuture::from(promise).await.map_err(operation);
            let target = handle.state.enqueued.get();
            while result.is_ok() && handle.state.completed.get() < target && !handle.state.cancellation.load(Ordering::Relaxed) {
                if let Err(error) = yield_to_browser().await {
                    let _ = handle.state.sender.send(DecoderEvent::Error(CodecError::Operation(error)));
                    return;
                }
            }
            if !handle.state.cancellation.load(Ordering::Relaxed) {
                let event = match result { Ok(_) => DecoderEvent::Flushed(id), Err(error) => DecoderEvent::Error(error) };
                let _ = handle.state.sender.send(event);
            }
        });
        Ok(())
    }
    pub fn decode_queue_size(&self) -> usize { self.handle.as_ref().map_or(0, |h| h.decoder.decode_queue_size() as usize) }
    pub fn pending_output(&self) -> usize {
        self.receiver.as_ref().map_or(0, Receiver::len) + self.handle.as_ref().map_or(0, |h| h.state.pending.get() as usize)
    }
    pub fn poll(&mut self) -> Option<DecoderEvent<VideoFrame>> { self.receiver.as_ref()?.try_recv().ok() }
    pub fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.state.cancellation.store(true, Ordering::Relaxed);
            let _ = handle.decoder.close();
        }
        self.receiver = None;
    }
}

pub(crate) struct AudioDecoder {
    handle: Option<Rc<AudioHandle>>,
    receiver: Option<Receiver<DecoderEvent<AudioData>>>,
}
struct AudioHandle {
    decoder: web::AudioDecoder,
    sender: Sender<DecoderEvent<AudioData>>,
    cancelled: Cell<bool>,
    _output: Closure<dyn FnMut(web::AudioData)>,
    _error: Closure<dyn FnMut(JsValue)>,
}
impl Drop for AudioHandle { fn drop(&mut self) { let _ = self.decoder.close(); } }
impl AudioDecoder {
    pub fn new() -> Result<Self, CodecError> { Ok(Self { handle: None, receiver: None }) }
    pub fn configure(&mut self, config: AudioDecoderConfig) -> Result<(), CodecError> {
        if self.handle.is_none() {
            let (sender, receiver) = unbounded();
            let output_sender = sender.clone();
            let output = Closure::wrap(Box::new(move |data: web::AudioData| {
                let event = match audio_data(&data) {
                    Ok(data) => DecoderEvent::Output(data), Err(error) => DecoderEvent::Error(error),
                };
                data.close();
                let _ = output_sender.send(event);
            }) as Box<dyn FnMut(web::AudioData)>);
            let error_sender = sender.clone();
            let error = Closure::wrap(Box::new(move |error: JsValue| {
                let _ = error_sender.send(DecoderEvent::Error(operation(error)));
            }) as Box<dyn FnMut(JsValue)>);
            let init = web::AudioDecoderInit::new(error.as_ref().unchecked_ref(), output.as_ref().unchecked_ref());
            let decoder = web::AudioDecoder::new(&init).map_err(operation)?;
            self.handle = Some(Rc::new(AudioHandle { decoder, sender, cancelled: Cell::new(false), _output: output, _error: error }));
            self.receiver = Some(receiver);
        }
        self.handle.as_ref().expect("decoder initialized").decoder.configure(&audio_config(&config)).map_err(operation)
    }
    pub fn decode(&mut self, chunk: EncodedChunk) -> Result<(), CodecError> {
        let handle = self.handle.as_ref().ok_or(CodecError::InvalidState("browser decoder absent"))?;
        let bytes = Uint8Array::from(chunk.data.as_ref());
        let kind = match chunk.kind { crate::ChunkType::Key => web::EncodedAudioChunkType::Key,
            crate::ChunkType::Delta => web::EncodedAudioChunkType::Delta };
        let init = web::EncodedAudioChunkInit::new(bytes.unchecked_ref(), 0, kind);
        init.set_timestamp_f64(chunk.timestamp as f64);
        if let Some(duration) = chunk.duration { init.set_duration_f64(duration as f64); }
        handle.decoder.decode(&web::EncodedAudioChunk::new(&init).map_err(operation)?).map_err(operation)
    }
    pub fn flush(&mut self, id: FlushId) -> Result<(), CodecError> {
        let handle = self.handle.as_ref().ok_or(CodecError::InvalidState("browser decoder absent"))?.clone();
        let promise = handle.decoder.flush();
        spawn_local(async move {
            let event = match JsFuture::from(promise).await {
                Ok(_) => DecoderEvent::Flushed(id), Err(error) => DecoderEvent::Error(operation(error)),
            };
            if !handle.cancelled.get() { let _ = handle.sender.send(event); }
        });
        Ok(())
    }
    pub fn decode_queue_size(&self) -> usize { self.handle.as_ref().map_or(0, |h| h.decoder.decode_queue_size() as usize) }
    pub fn pending_output(&self) -> usize { self.receiver.as_ref().map_or(0, Receiver::len) }
    pub fn poll(&mut self) -> Option<DecoderEvent<AudioData>> { self.receiver.as_ref()?.try_recv().ok() }
    pub fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancelled.set(true);
            let _ = handle.decoder.close();
        }
        self.receiver = None;
    }
}
fn video_config(config: &VideoDecoderConfig) -> web::VideoDecoderConfig {
    let value = web::VideoDecoderConfig::new(&config.codec.0);
    value.set_coded_width(config.coded_width);
    value.set_coded_height(config.coded_height);
    value.set_optimize_for_latency(config.optimize_for_latency);
    value.set_hardware_acceleration(match config.hardware_acceleration {
        HardwareAcceleration::NoPreference => web::HardwareAcceleration::NoPreference,
        HardwareAcceleration::PreferHardware => web::HardwareAcceleration::PreferHardware,
        HardwareAcceleration::PreferSoftware => web::HardwareAcceleration::PreferSoftware,
    });
    if let Some(description) = &config.description {
        value.set_description_u8_array(&Uint8Array::from(description.as_ref()));
    }
    value
}
fn audio_config(config: &AudioDecoderConfig) -> web::AudioDecoderConfig {
    let value = web::AudioDecoderConfig::new(&config.codec.0, config.number_of_channels.into(), config.sample_rate);
    if let Some(description) = &config.description {
        value.set_description_u8_array(&Uint8Array::from(description.as_ref()));
    }
    value
}
pub(crate) async fn video_config_supported(config: &VideoDecoderConfig) -> Result<bool, CodecError> {
    support(web::VideoDecoder::is_config_supported(&video_config(config)).map_err(operation)?).await
}
pub(crate) async fn audio_config_supported(config: &AudioDecoderConfig) -> Result<bool, CodecError> {
    support(web::AudioDecoder::is_config_supported(&audio_config(config)).map_err(operation)?).await
}
async fn support(promise: Promise) -> Result<bool, CodecError> {
    let value = JsFuture::from(promise).await.map_err(operation)?;
    Ok(js_sys::Reflect::get(&value, &JsValue::from_str("supported")).map_err(operation)?.as_bool().unwrap_or(false))
}
fn audio_data(data: &web::AudioData) -> Result<AudioData, CodecError> {
    let options = web::AudioDataCopyToOptions::new(0);
    options.set_format(web::AudioSampleFormat::F32);
    let byte_len = data.allocation_size(&options).map_err(operation)?;
    if byte_len % 4 != 0 { return Err(CodecError::Operation("unaligned f32 PCM size".into())); }
    let storage = Float32Array::new_with_length(byte_len / 4);
    data.copy_to_with_buffer_source(storage.unchecked_ref(), &options).map_err(operation)?;
    let mut samples = vec![0.0; storage.length() as usize];
    storage.copy_to(&mut samples);
    Ok(AudioData { timestamp: data.timestamp() as i64, sample_rate: data.sample_rate() as u32,
        number_of_channels: u16::try_from(data.number_of_channels()).map_err(|_| CodecError::Operation("channel count overflow".into()))?,
        samples: samples.into() })
}
fn operation(error: JsValue) -> CodecError { CodecError::Operation(js_error(&error)) }

struct FrameCopyState {
    queue: RefCell<VecDeque<web_sys::VideoFrame>>,
    running: Cell<bool>,
    pending: Cell<u32>,
    enqueued: Cell<u64>,
    completed: Cell<u64>,
    storage: RefCell<Option<Uint8Array>>,
    sender: Sender<DecoderEvent<VideoFrame>>,
    cancellation: Arc<AtomicBool>,
}

impl FrameCopyState {
    fn enqueue(self: &Rc<Self>, frame: web_sys::VideoFrame) {
        self.enqueued.set(self.enqueued.get() + 1);
        self.pending.set(self.pending.get().saturating_add(1));
        self.queue.borrow_mut().push_back(frame);
        if !self.running.replace(true) {
            let state = self.clone();
            spawn_local(async move { state.process().await });
        }
    }

    async fn process(self: Rc<Self>) {
        loop {
            let Some(frame) = self.queue.borrow_mut().pop_front() else {
                self.running.set(false);
                return;
            };
            if self.cancellation.load(Ordering::Relaxed) {
                frame.close();
                self.pending.set(self.pending.get().saturating_sub(1));
                self.completed.set(self.completed.get() + 1);
                continue;
            }
            let decoded = decode_frame(&frame, &self).await;
            frame.close();
            self.pending.set(self.pending.get().saturating_sub(1));
            self.completed.set(self.completed.get() + 1);
            match decoded {
                Ok(frame) => {
                    let _ = self.sender.send(DecoderEvent::Output(frame));
                }
                Err(error) => {
                    let _ = self.sender.send(DecoderEvent::Error(CodecError::Operation(error)));
                }
            }
        }
    }

    fn storage(&self, byte_len: u32) -> Uint8Array {
        let mut storage = self.storage.borrow_mut();
        if storage.as_ref().is_none_or(|storage| storage.length() < byte_len) {
            *storage = Some(Uint8Array::new_with_length(byte_len));
        }
        storage
            .as_ref()
            .expect("frame storage must be allocated")
            .subarray(0, byte_len)
    }
}

impl Drop for FrameCopyState {
    fn drop(&mut self) {
        for frame in self.queue.get_mut().drain(..) {
            frame.close();
        }
    }
}

async fn decode_frame(frame: &web_sys::VideoFrame, state: &FrameCopyState) -> Result<VideoFrame, String> {
    let width = frame.coded_width();
    let height = frame.coded_height();
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let timestamp = frame.timestamp();
    let color_space = frame.color_space();
    let full_range = color_space.full_range().unwrap_or(false);
    let (kr, kb) = match color_space.matrix() {
        Some(VideoMatrixCoefficients::Smpte170m | VideoMatrixCoefficients::Bt470bg) => (0.299, 0.114),
        Some(VideoMatrixCoefficients::Bt2020Ncl) => (0.2627, 0.0593),
        _ => (0.2126, 0.0722),
    };
    let transfer = match color_space.transfer() {
        Some(VideoTransferCharacteristics::Linear) => TransferFunction::Linear,
        Some(VideoTransferCharacteristics::Iec6196621) => TransferFunction::Srgb,
        Some(VideoTransferCharacteristics::Pq | VideoTransferCharacteristics::Hlg) => {
            return Err("HDR WebCodecs video requires tone mapping, which is not implemented".into());
        }
        _ => TransferFunction::Bt1886,
    };
    let pixels = if frame.format() == Some(VideoPixelFormat::I420) {
        match copy_i420(frame, state, width, height).await {
            Ok((y, u, v)) => crate::VideoPixels::I420Planar { y, u, v },
            Err(_) => crate::VideoPixels::Rgba(copy_rgba(frame, state, width, height).await?),
        }
    } else {
        crate::VideoPixels::Rgba(copy_rgba(frame, state, width, height).await?)
    };
    Ok(VideoFrame {
        timestamp: timestamp as i64,
        width,
        height,
        chroma_width,
        chroma_height,
        color_transform: YuvColorTransform::from_luma_coefficients(kr, kb, !full_range),
        transfer,
        pixels,
    })
}

async fn copy_i420(
    frame: &web_sys::VideoFrame,
    state: &FrameCopyState,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let y_len = width.checked_mul(height).ok_or_else(|| "WebCodecs Y plane size overflowed".to_string())?;
    let u_len = chroma_width.checked_mul(chroma_height).ok_or_else(|| "WebCodecs chroma plane size overflowed".to_string())?;
    let v_offset = y_len.checked_add(u_len).ok_or_else(|| "WebCodecs plane offset overflowed".to_string())?;
    let options = VideoFrameCopyToOptions::new();
    options.set_layout(&[
        PlaneLayout::new(0, width),
        PlaneLayout::new(y_len, chroma_width),
        PlaneLayout::new(v_offset, chroma_width),
    ]);
    let byte_len = frame.allocation_size_with_options(&options)
        .map_err(|error| format!("WebCodecs I420 allocation failed: {}", js_error(&error)))?;
    let storage = state.storage(byte_len);
    JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options)).await
        .map_err(|error| format!("WebCodecs I420 copy failed: {}", js_error(&error)))?;
    Ok((
        uint8_array_to_vec(&storage.subarray(0, y_len)),
        uint8_array_to_vec(&storage.subarray(y_len, v_offset)),
        uint8_array_to_vec(&storage.subarray(v_offset, byte_len)),
    ))
}

async fn copy_rgba(frame: &web_sys::VideoFrame, state: &FrameCopyState, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let options = VideoFrameCopyToOptions::new();
    options.set_format(VideoPixelFormat::Rgba);
    options.set_layout(&[PlaneLayout::new(0, width.saturating_mul(4))]);
    if let Ok(byte_len) = frame.allocation_size_with_options(&options) {
        let storage = state.storage(byte_len);
        if JsFuture::from(frame.copy_to_with_u8_array_and_options(&storage, &options)).await.is_ok() {
            return Ok(uint8_array_to_vec(&storage));
        }
    }
    copy_rgba_with_canvas(frame, width, height)
}

fn copy_rgba_with_canvas(frame: &web_sys::VideoFrame, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let document = web_sys::window().and_then(|window| window.document())
        .ok_or_else(|| "browser document is unavailable".to_string())?;
    let canvas: HtmlCanvasElement = document.create_element("canvas")
        .map_err(|error| format!("failed to create fallback canvas: {}", js_error(&error)))?
        .dyn_into().map_err(|_| "fallback canvas element has the wrong type".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context: CanvasRenderingContext2d = canvas.get_context("2d")
        .map_err(|error| format!("failed to get fallback canvas: {}", js_error(&error)))?
        .ok_or_else(|| "fallback 2D canvas is unavailable".to_string())?
        .dyn_into().map_err(|_| "fallback canvas context has the wrong type".to_string())?;
    context.draw_image_with_video_frame_and_dw_and_dh(frame, 0.0, 0.0, width.into(), height.into())
        .map_err(|error| format!("fallback canvas draw failed: {}", js_error(&error)))?;
    context.get_image_data(0.0, 0.0, width as f64, height as f64)
        .map(|image| image.data().0)
        .map_err(|error| format!("fallback canvas read failed: {}", js_error(&error)))
}

async fn yield_to_browser() -> Result<(), String> {
    let promise = Promise::new(&mut |resolve, reject| {
        let Some(window) = web_sys::window() else {
            let _ = reject.call1(&JsValue::UNDEFINED, &JsValue::from_str("window is unavailable"));
            return;
        };
        if let Err(error) = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0) {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
        .map_err(|error| format!("browser decode yield failed: {}", js_error(&error)))
}

fn uint8_array_to_vec(array: &Uint8Array) -> Vec<u8> {
    let mut bytes = vec![0; array.length() as usize];
    array.copy_to(&mut bytes);
    bytes
}

fn js_error(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
