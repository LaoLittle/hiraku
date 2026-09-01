//! Streaming Matroska/WebM video playback for Bevy.
//!
//! Hiraku intentionally supports one deterministic media profile: an AV1 video
//! track and an Opus audio track inside a Matroska (`.mkv`) or WebM (`.webm`)
//! container. Container loading, decoding and presentation live in this crate;
//! story semantics belong to the embedding engine.

mod asset;
mod color;
#[cfg(not(target_arch = "wasm32"))]
mod decode;
#[cfg(target_arch = "wasm32")]
#[path = "decode_wasm.rs"]
mod decode;
mod player;
mod render;

pub use asset::{VideoAsset, VideoAssetLoader, VideoAssetLoaderError, VideoMetadata};
pub use player::{HirakuVideoPlugin, VideoEvent, VideoPlaybackId, VideoPlaybackState, VideoPlayer};
