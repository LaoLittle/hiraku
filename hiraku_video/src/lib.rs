//! Streaming Matroska/WebM video playback for Bevy.
//!
//! Hiraku intentionally supports one deterministic media profile: an AV1 video
//! track and an Opus audio track inside a Matroska (`.mkv`) or WebM (`.webm`)
//! container. Container loading, decoding and presentation live in this crate;
//! story semantics belong to the embedding engine.

mod asset;
mod color;
mod player;
mod render;
mod platform;

pub use asset::{VideoAsset, VideoAssetLoader, VideoAssetLoaderError, VideoMetadata};
pub use player::VideoDecodeSettings;
pub use player::{HirakuVideoPlugin, VideoEvent, VideoPlaybackId, VideoPlaybackState, VideoPlayer};
