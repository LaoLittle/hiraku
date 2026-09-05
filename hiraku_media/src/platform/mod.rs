cfg_select! {
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    },
    _ => {
        mod software;
        mod native;
        #[cfg(all(target_os = "macos", feature = "hardware"))]
        mod macos;
        #[cfg(all(target_os = "windows", feature = "hardware"))]
        mod windows;
        pub(crate) use native::{VideoDecoder, AudioDecoder, video_config_supported, audio_config_supported};
    }
}
