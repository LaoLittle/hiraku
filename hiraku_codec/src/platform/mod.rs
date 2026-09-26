mod audio_packet;
pub(crate) mod encoder;
pub(crate) use audio_packet::AudioPacketDecoder;

cfg_select! {
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    },
    _ => {
        cfg_select! {
            any(test,
                all(target_vendor = "apple", feature = "video-toolbox"),
                all(target_os = "windows", feature = "media-foundation"),
                all(target_os = "android", feature = "media-codec")) => { mod nal; },
            _ => {}
        }
        #[cfg(any(test, feature = "video-toolbox", feature = "media-foundation", feature = "media-codec", feature = "vaapi"))]
        mod vp9;
        mod audio;
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
