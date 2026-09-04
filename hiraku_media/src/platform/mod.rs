cfg_select! {
    target_os = "macos" => {
        mod macos;
        mod native;
        pub(crate) use macos::decode;
    },
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::decode;
    },
    _ => {
        mod native;
        pub(crate) use native::decode;
    }
}

#[allow(dead_code)]
pub enum DecoderHandle {
    #[cfg(not(target_family = "wasm"))]
    Software(native::DecoderHandle),

    #[cfg(target_os = "macos")]
    VideoToolbox(macos::DecoderHandle),

    #[cfg(target_family = "wasm")]
    WebCodecs(wasm::DecoderHandle),
}
