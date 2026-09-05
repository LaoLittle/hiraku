//! Streaming Matroska/WebM video playback for Bevy.
//!
//! Hiraku intentionally supports one deterministic media profile: an AV1 video
//! track and an Opus audio track inside a Matroska (`.mkv`) or WebM (`.webm`)
//! container. Decoding lives in hiraku-media; this crate owns Bevy playback and presentation;
//! story semantics belong to the embedding engine.

mod asset;
mod audio;
mod color;
pub mod container;
mod decode;
mod player;
mod render;
mod upload;

pub use asset::{VideoAsset, VideoAssetLoader, VideoAssetLoaderError, VideoMetadata};
pub use player::{
    HirakuVideoPlugin, VideoDecodeSettings, VideoEvent, VideoPlaybackId, VideoPlaybackState,
    VideoPlayer,
};
