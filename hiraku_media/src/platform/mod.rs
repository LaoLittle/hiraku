cfg_select! {
    all(target_os = "macos", feature = "hardware") => {
        mod macos;
        mod software;
        pub(crate) use macos::decode;
    },
    all(target_os = "windows", feature = "hardware") => {
        mod software;
        pub(crate) use software::docode;
    },
    target_family = "wasm" => {
        mod wasm;
        pub(crate) use wasm::decode;
    },
    _ => {
        mod software;
        pub(crate) use software::decode;
    }
}

#[allow(dead_code)]
pub enum DecoderHandle {
    #[cfg(not(target_family = "wasm"))]
    Software(software::DecoderHandle),

    #[cfg(all(target_os = "macos", feature = "hardware"))]
    VideoToolbox(macos::DecoderHandle),

    #[cfg(target_family = "wasm")]
    WebCodecs(wasm::DecoderHandle),
}
