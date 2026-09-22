mod audio_packet;
pub(crate) use audio_packet::AudioPacketDecoder;
// Pure metadata policy is testable without an Android NDK or decoder device.
#[cfg(test)]
#[path = "android/color.rs"]
mod android_color_tests;

cfg_select! {
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    },
    _ => {
        mod software;
        mod native;
        cfg_select! {
            all(target_vendor = "apple", feature = "video-toolbox") => {
                mod macos;
                use macos as imp;
            },
            all(target_os = "android", feature = "media-codec") => {
                mod frame;
                mod android;
                use android as imp;
            },
            all(target_os = "windows", feature = "media-foundation") => {
                mod windows;
                use windows as imp;
            },
            all(target_os = "linux", feature = "vaapi") => {
                mod frame;
                mod linux;
                use linux as imp;
            },
            _ => { mod unavailable; use unavailable as imp; }
        }
        pub(crate) use native::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    }
}
