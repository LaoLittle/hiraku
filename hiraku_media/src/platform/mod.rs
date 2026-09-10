cfg_select! {
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    },
    _ => {
        mod software;
        mod native;
        cfg_select! {
            all(target_os = "macos", feature = "video-toolbox") => {
                mod macos;
            },
            all(target_os = "android", feature = "media-codec") => {
                mod frame;
                mod android;
            },
            all(target_os = "windows", feature = "media-foundation") => {
                mod windows;
            },
            all(target_os = "linux", feature = "vaapi") => {
                mod frame;
                mod linux;
            },
            _ => {}
        }
        pub(crate) use native::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    }
}
