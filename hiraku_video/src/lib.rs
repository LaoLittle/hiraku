//! Streaming Matroska/WebM/MP4 video playback for Bevy.
//!
//! Video and optional Opus audio are demuxed with Symphonia from Matroska (`.mkv`),
//! WebM (`.webm`) or MP4 (`.mp4`). Supported tracks depend on the container reader
//! and codec adapter. Decoding lives in hiraku-codec; this crate owns Bevy playback and presentation;
//! story semantics belong to the embedding engine.

mod asset;
mod audio;
mod color;
pub mod container;
mod decode;
mod player;
mod render;
mod upload;

pub use asset::{
    AlphaLayout, VideoAsset, VideoAssetLoader, VideoAssetLoaderError, VideoLoaderSettings,
    VideoMetadata,
};
pub use player::{
    HirakuVideoPlugin, VideoDecodeSettings, VideoEvent, VideoPlaybackId, VideoPlaybackState,
    VideoPlaybackSystems, VideoPlayer, VideoWorldView,
};
