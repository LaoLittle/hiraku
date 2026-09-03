#![allow(unused_imports)]
#![allow(clippy::all)]
#![allow(deprecated)]
#![allow(unused_mut)]
#![allow(unused)]

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedVideoChunkType {
    Key = "key",
    Delta = "delta",
}

#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareAcceleration {
    NoPreference = "no-preference",
    PreferHardware = "prefer-hardware",
    PreferSoftware = "prefer-software",
}

#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecState {
    Unconfigured = "unconfigured",
    Configured = "configured",
    Closed = "closed",
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(
        extends = ::js_sys::Object,
        js_name = "EncodedVideoChunk",
        typescript_type = "EncodedVideoChunk"
    )]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type EncodedVideoChunk;

    #[wasm_bindgen(method, getter, js_class = "EncodedVideoChunk", js_name = "type")]
    pub fn type_(this: &EncodedVideoChunk) -> EncodedVideoChunkType;

    #[wasm_bindgen(method, getter, js_class = "EncodedVideoChunk", js_name = "timestamp")]
    pub fn timestamp(this: &EncodedVideoChunk) -> f64;

    #[wasm_bindgen(method, getter, js_class = "EncodedVideoChunk", js_name = "duration")]
    pub fn duration(this: &EncodedVideoChunk) -> Option<f64>;

    #[wasm_bindgen(method, getter, js_class = "EncodedVideoChunk", js_name = "byteLength")]
    pub fn byte_length(this: &EncodedVideoChunk) -> u32;

    #[wasm_bindgen(catch, constructor, js_class = "EncodedVideoChunk")]
    pub fn new(init: &EncodedVideoChunkInit) -> Result<EncodedVideoChunk, JsValue>;

    #[wasm_bindgen(catch, method, js_class = "EncodedVideoChunk", js_name = "copyTo")]
    pub fn copy_to_with_buffer_source(
        this: &EncodedVideoChunk,
        destination: &::js_sys::Object,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "EncodedVideoChunk", js_name = "copyTo")]
    pub fn copy_to_with_u8_slice(
        this: &EncodedVideoChunk,
        destination: &mut [u8],
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "EncodedVideoChunk", js_name = "copyTo")]
    pub fn copy_to_with_u8_array(
        this: &EncodedVideoChunk,
        destination: &::js_sys::Uint8Array,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(
        extends = ::web_sys::EventTarget,
        extends = ::js_sys::Object,
        js_name = "VideoDecoder",
        typescript_type = "VideoDecoder"
    )]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type VideoDecoder;

    #[wasm_bindgen(method, getter, js_class = "VideoDecoder", js_name = "state")]
    pub fn state(this: &VideoDecoder) -> CodecState;

    #[wasm_bindgen(method, getter, js_class = "VideoDecoder", js_name = "decodeQueueSize")]
    pub fn decode_queue_size(this: &VideoDecoder) -> u32;

    #[wasm_bindgen(method, getter, js_class = "VideoDecoder", js_name = "ondequeue")]
    pub fn ondequeue(this: &VideoDecoder) -> Option<::js_sys::Function>;

    #[wasm_bindgen(method, setter, js_class = "VideoDecoder", js_name = "ondequeue")]
    pub fn set_ondequeue(this: &VideoDecoder, value: Option<&::js_sys::Function>);

    #[wasm_bindgen(catch, constructor, js_class = "VideoDecoder")]
    pub fn new(init: &VideoDecoderInit) -> Result<VideoDecoder, JsValue>;

    #[wasm_bindgen(catch, method, js_class = "VideoDecoder")]
    pub fn close(this: &VideoDecoder) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "VideoDecoder")]
    pub fn configure(this: &VideoDecoder, config: &VideoDecoderConfig) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "VideoDecoder")]
    pub fn decode(this: &VideoDecoder, chunk: &EncodedVideoChunk) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_class = "VideoDecoder")]
    pub fn flush(this: &VideoDecoder) -> ::js_sys::Promise;

    #[wasm_bindgen(catch, method, js_class = "VideoDecoder")]
    pub fn reset(this: &VideoDecoder) -> Result<(), JsValue>;
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "VideoColorSpaceInit")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type VideoColorSpaceInit;

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "EncodedVideoChunkInit")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type EncodedVideoChunkInit;

    #[wasm_bindgen(method, getter = "data")]
    pub fn get_data(this: &EncodedVideoChunkInit) -> ::js_sys::Object;

    #[wasm_bindgen(method, setter = "data")]
    pub fn set_data(this: &EncodedVideoChunkInit, val: &::js_sys::Object);

    #[wasm_bindgen(method, setter = "data")]
    pub unsafe fn set_data_u8_slice(this: &EncodedVideoChunkInit, val: &mut [u8]);

    #[wasm_bindgen(method, setter = "data")]
    pub fn set_data_u8_array(this: &EncodedVideoChunkInit, val: &::js_sys::Uint8Array);

    #[wasm_bindgen(method, getter = "duration")]
    pub fn get_duration(this: &EncodedVideoChunkInit) -> Option<f64>;

    #[wasm_bindgen(method, setter = "duration")]
    pub fn set_duration(this: &EncodedVideoChunkInit, val: u32);

    #[wasm_bindgen(method, setter = "duration")]
    pub fn set_duration_f64(this: &EncodedVideoChunkInit, val: f64);

    #[wasm_bindgen(method, getter = "timestamp")]
    pub fn get_timestamp(this: &EncodedVideoChunkInit) -> f64;

    #[wasm_bindgen(method, setter = "timestamp")]
    pub fn set_timestamp(this: &EncodedVideoChunkInit, val: i32);

    #[wasm_bindgen(method, setter = "timestamp")]
    pub fn set_timestamp_f64(this: &EncodedVideoChunkInit, val: f64);

    #[wasm_bindgen(method, getter = "transfer")]
    pub fn get_transfer(
        this: &EncodedVideoChunkInit,
    ) -> Option<::js_sys::Array>;

    #[wasm_bindgen(method, setter = "transfer")]
    pub fn set_transfer(this: &EncodedVideoChunkInit, val: &[::js_sys::ArrayBuffer]);

    #[wasm_bindgen(method, getter = "type")]
    pub fn get_type(this: &EncodedVideoChunkInit) -> EncodedVideoChunkType;

    #[wasm_bindgen(method, setter = "type")]
    pub fn set_type(this: &EncodedVideoChunkInit, val: EncodedVideoChunkType);

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "VideoDecoderConfig")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type VideoDecoderConfig;

    #[wasm_bindgen(method, getter = "codec")]
    pub fn get_codec(this: &VideoDecoderConfig) -> String;

    #[wasm_bindgen(method, setter = "codec")]
    pub fn set_codec(this: &VideoDecoderConfig, val: &str);

    #[wasm_bindgen(method, getter = "codedHeight")]
    pub fn get_coded_height(this: &VideoDecoderConfig) -> Option<u32>;

    #[wasm_bindgen(method, setter = "codedHeight")]
    pub fn set_coded_height(this: &VideoDecoderConfig, val: u32);

    #[wasm_bindgen(method, getter = "codedWidth")]
    pub fn get_coded_width(this: &VideoDecoderConfig) -> Option<u32>;

    #[wasm_bindgen(method, setter = "codedWidth")]
    pub fn set_coded_width(this: &VideoDecoderConfig, val: u32);

    #[wasm_bindgen(method, getter = "colorSpace")]
    pub fn get_color_space(this: &VideoDecoderConfig) -> Option<VideoColorSpaceInit>;

    #[wasm_bindgen(method, setter = "colorSpace")]
    pub fn set_color_space(this: &VideoDecoderConfig, val: &VideoColorSpaceInit);

    #[wasm_bindgen(method, getter = "description")]
    pub fn get_description(this: &VideoDecoderConfig) -> Option<::js_sys::Object>;

    #[wasm_bindgen(method, setter = "description")]
    pub fn set_description(this: &VideoDecoderConfig, val: &::js_sys::Object);

    #[wasm_bindgen(method, setter = "description")]
    pub unsafe fn set_description_u8_slice(this: &VideoDecoderConfig, val: &mut [u8]);

    #[wasm_bindgen(method, setter = "description")]
    pub fn set_description_u8_array(this: &VideoDecoderConfig, val: &::js_sys::Uint8Array);

    #[wasm_bindgen(method, getter = "displayAspectHeight")]
    pub fn get_display_aspect_height(this: &VideoDecoderConfig) -> Option<u32>;

    #[wasm_bindgen(method, setter = "displayAspectHeight")]
    pub fn set_display_aspect_height(this: &VideoDecoderConfig, val: u32);

    #[wasm_bindgen(method, getter = "displayAspectWidth")]
    pub fn get_display_aspect_width(this: &VideoDecoderConfig) -> Option<u32>;

    #[wasm_bindgen(method, setter = "displayAspectWidth")]
    pub fn set_display_aspect_width(this: &VideoDecoderConfig, val: u32);

    #[wasm_bindgen(method, getter = "flip")]
    pub fn get_flip(this: &VideoDecoderConfig) -> Option<bool>;

    #[wasm_bindgen(method, setter = "flip")]
    pub fn set_flip(this: &VideoDecoderConfig, val: bool);

    #[wasm_bindgen(method, getter = "hardwareAcceleration")]
    pub fn get_hardware_acceleration(this: &VideoDecoderConfig) -> Option<HardwareAcceleration>;

    #[wasm_bindgen(method, setter = "hardwareAcceleration")]
    pub fn set_hardware_acceleration(this: &VideoDecoderConfig, val: HardwareAcceleration);

    #[wasm_bindgen(method, getter = "optimizeForLatency")]
    pub fn get_optimize_for_latency(this: &VideoDecoderConfig) -> Option<bool>;

    #[wasm_bindgen(method, setter = "optimizeForLatency")]
    pub fn set_optimize_for_latency(this: &VideoDecoderConfig, val: bool);

    #[wasm_bindgen(method, getter = "rotation")]
    pub fn get_rotation(this: &VideoDecoderConfig) -> Option<f64>;

    #[wasm_bindgen(method, setter = "rotation")]
    pub fn set_rotation(this: &VideoDecoderConfig, val: f64);

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "VideoDecoderInit")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type VideoDecoderInit;

    #[wasm_bindgen(method, getter = "error")]
    pub fn get_error(this: &VideoDecoderInit) -> ::js_sys::Function;

    #[wasm_bindgen(method, setter = "error")]
    pub fn set_error(this: &VideoDecoderInit, val: &::js_sys::Function);

    #[wasm_bindgen(method, getter = "output")]
    pub fn get_output(this: &VideoDecoderInit) -> ::js_sys::Function;

    #[wasm_bindgen(method, setter = "output")]
    pub fn set_output(this: &VideoDecoderInit, val: &::js_sys::Function);
}

impl EncodedVideoChunkInit {
    pub fn new(data: &::js_sys::Object, timestamp: i32, type_: EncodedVideoChunkType) -> Self {
        let mut ret: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        ret.set_data(data);
        ret.set_timestamp(timestamp);
        ret.set_type(type_);
        ret
    }

    pub unsafe fn new_with_u8_slice(
        data: &mut [u8],
        timestamp: i32,
        type_: EncodedVideoChunkType,
    ) -> Self {
        let mut ret: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        unsafe { ret.set_data_u8_slice(data); }
        ret.set_timestamp(timestamp);
        ret.set_type(type_);
        ret
    }

    pub fn new_with_u8_array(
        data: &::js_sys::Uint8Array,
        timestamp: i32,
        type_: EncodedVideoChunkType,
    ) -> Self {
        let mut ret: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        ret.set_data_u8_array(data);
        ret.set_timestamp(timestamp);
        ret.set_type(type_);
        ret
    }

    pub fn data(&mut self, val: &::js_sys::Object) -> &mut Self {
        self.set_data(val);
        self
    }

    pub fn duration(&mut self, val: u32) -> &mut Self {
        self.set_duration(val);
        self
    }

    pub fn timestamp(&mut self, val: i32) -> &mut Self {
        self.set_timestamp(val);
        self
    }

    pub fn transfer(&mut self, val: &[::js_sys::ArrayBuffer]) -> &mut Self {
        self.set_transfer(val);
        self
    }

    pub fn type_(&mut self, val: EncodedVideoChunkType) -> &mut Self {
        self.set_type(val);
        self
    }
}

impl VideoDecoderConfig {
    pub fn new(codec: &str) -> Self {
        let mut ret: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        ret.set_codec(codec);
        ret
    }

    pub fn codec(&mut self, val: &str) -> &mut Self {
        self.set_codec(val);
        self
    }

    pub fn coded_height(&mut self, val: u32) -> &mut Self {
        self.set_coded_height(val);
        self
    }

    pub fn coded_width(&mut self, val: u32) -> &mut Self {
        self.set_coded_width(val);
        self
    }

    pub fn color_space(&mut self, val: &VideoColorSpaceInit) -> &mut Self {
        self.set_color_space(val);
        self
    }

    pub fn description(&mut self, val: &::js_sys::Object) -> &mut Self {
        self.set_description(val);
        self
    }

    pub fn display_aspect_height(&mut self, val: u32) -> &mut Self {
        self.set_display_aspect_height(val);
        self
    }

    pub fn display_aspect_width(&mut self, val: u32) -> &mut Self {
        self.set_display_aspect_width(val);
        self
    }

    pub fn flip(&mut self, val: bool) -> &mut Self {
        self.set_flip(val);
        self
    }

    pub fn hardware_acceleration(&mut self, val: HardwareAcceleration) -> &mut Self {
        self.set_hardware_acceleration(val);
        self
    }

    pub fn optimize_for_latency(&mut self, val: bool) -> &mut Self {
        self.set_optimize_for_latency(val);
        self
    }

    pub fn rotation(&mut self, val: f64) -> &mut Self {
        self.set_rotation(val);
        self
    }
}

impl VideoDecoderInit {
    pub fn new(error: &::js_sys::Function, output: &::js_sys::Function) -> Self {
        let mut ret: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        ret.set_error(error);
        ret.set_output(output);
        ret
    }

    pub fn error(&mut self, val: &::js_sys::Function) -> &mut Self {
        self.set_error(val);
        self
    }

    pub fn output(&mut self, val: &::js_sys::Function) -> &mut Self {
        self.set_output(val);
        self
    }
}

// Audio WebCodecs is still gated by `web_sys_unstable_apis`. Keep the small
// surface Hiraku needs here so downstream builds never require a global rustc
// cfg flag.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedAudioChunkType {
    Key = "key",
    Delta = "delta",
}

#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
    U8 = "u8",
    S16 = "s16",
    S32 = "s32",
    F32 = "f32",
    U8Planar = "u8-planar",
    S16Planar = "s16-planar",
    S32Planar = "s32-planar",
    F32Planar = "f32-planar",
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "EncodedAudioChunk")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type EncodedAudioChunk;

    #[wasm_bindgen(catch, constructor, js_class = "EncodedAudioChunk")]
    pub fn new(init: &EncodedAudioChunkInit) -> Result<EncodedAudioChunk, JsValue>;

    #[wasm_bindgen(
        extends = ::web_sys::EventTarget,
        extends = ::js_sys::Object,
        js_name = "AudioDecoder"
    )]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type AudioDecoder;

    #[wasm_bindgen(catch, constructor, js_class = "AudioDecoder")]
    pub fn new(init: &AudioDecoderInit) -> Result<AudioDecoder, JsValue>;

    #[wasm_bindgen(method, getter, js_class = "AudioDecoder", js_name = "decodeQueueSize")]
    pub fn decode_queue_size(this: &AudioDecoder) -> u32;

    #[wasm_bindgen(catch, method, js_class = "AudioDecoder")]
    pub fn close(this: &AudioDecoder) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "AudioDecoder")]
    pub fn configure(this: &AudioDecoder, config: &AudioDecoderConfig) -> Result<(), JsValue>;

    #[wasm_bindgen(catch, method, js_class = "AudioDecoder")]
    pub fn decode(this: &AudioDecoder, chunk: &EncodedAudioChunk) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_class = "AudioDecoder")]
    pub fn flush(this: &AudioDecoder) -> ::js_sys::Promise;

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "AudioData")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type AudioData;

    #[wasm_bindgen(method, getter, js_class = "AudioData", js_name = "numberOfFrames")]
    pub fn number_of_frames(this: &AudioData) -> u32;

    #[wasm_bindgen(method, getter, js_class = "AudioData", js_name = "numberOfChannels")]
    pub fn number_of_channels(this: &AudioData) -> u32;

    #[wasm_bindgen(method, getter, js_class = "AudioData", js_name = "sampleRate")]
    pub fn sample_rate(this: &AudioData) -> f32;

    #[wasm_bindgen(catch, method, js_class = "AudioData", js_name = "allocationSize")]
    pub fn allocation_size(
        this: &AudioData,
        options: &AudioDataCopyToOptions,
    ) -> Result<u32, JsValue>;

    #[wasm_bindgen(catch, method, js_class = "AudioData", js_name = "copyTo")]
    pub fn copy_to_with_buffer_source(
        this: &AudioData,
        destination: &::js_sys::Object,
        options: &AudioDataCopyToOptions,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_class = "AudioData")]
    pub fn close(this: &AudioData);
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "EncodedAudioChunkInit")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type EncodedAudioChunkInit;

    #[wasm_bindgen(method, setter = "data")]
    pub fn set_data(this: &EncodedAudioChunkInit, value: &::js_sys::Object);

    #[wasm_bindgen(method, setter = "duration")]
    pub fn set_duration_f64(this: &EncodedAudioChunkInit, value: f64);

    #[wasm_bindgen(method, setter = "timestamp")]
    pub fn set_timestamp_f64(this: &EncodedAudioChunkInit, value: f64);

    #[wasm_bindgen(method, setter = "type")]
    pub fn set_type(this: &EncodedAudioChunkInit, value: EncodedAudioChunkType);

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "AudioDecoderConfig")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type AudioDecoderConfig;

    #[wasm_bindgen(method, setter = "codec")]
    pub fn set_codec(this: &AudioDecoderConfig, value: &str);

    #[wasm_bindgen(method, setter = "numberOfChannels")]
    pub fn set_number_of_channels(this: &AudioDecoderConfig, value: u32);

    #[wasm_bindgen(method, setter = "sampleRate")]
    pub fn set_sample_rate(this: &AudioDecoderConfig, value: u32);

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "AudioDecoderInit")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type AudioDecoderInit;

    #[wasm_bindgen(method, setter = "error")]
    pub fn set_error(this: &AudioDecoderInit, value: &::js_sys::Function);

    #[wasm_bindgen(method, setter = "output")]
    pub fn set_output(this: &AudioDecoderInit, value: &::js_sys::Function);

    #[wasm_bindgen(extends = ::js_sys::Object, js_name = "AudioDataCopyToOptions")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type AudioDataCopyToOptions;

    #[wasm_bindgen(method, setter = "format")]
    pub fn set_format(this: &AudioDataCopyToOptions, value: AudioSampleFormat);

    #[wasm_bindgen(method, setter = "planeIndex")]
    pub fn set_plane_index(this: &AudioDataCopyToOptions, value: u32);
}

impl EncodedAudioChunkInit {
    pub fn new(
        data: &::js_sys::Object,
        timestamp: i32,
        type_: EncodedAudioChunkType,
    ) -> Self {
        let value: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        value.set_data(data);
        value.set_timestamp_f64(f64::from(timestamp));
        value.set_type(type_);
        value
    }
}

impl AudioDecoderConfig {
    pub fn new(codec: &str, number_of_channels: u32, sample_rate: u32) -> Self {
        let value: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        value.set_codec(codec);
        value.set_number_of_channels(number_of_channels);
        value.set_sample_rate(sample_rate);
        value
    }
}

impl AudioDecoderInit {
    pub fn new(error: &::js_sys::Function, output: &::js_sys::Function) -> Self {
        let value: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        value.set_error(error);
        value.set_output(output);
        value
    }
}

impl AudioDataCopyToOptions {
    pub fn new(plane_index: u32) -> Self {
        let value: Self = ::wasm_bindgen::JsCast::unchecked_into(::js_sys::Object::new());
        value.set_plane_index(plane_index);
        value
    }
}
