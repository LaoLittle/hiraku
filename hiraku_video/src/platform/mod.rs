cfg_select! {
    target_arch = "wasm32" => {
        mod wasm;
        pub(crate) use wasm::*;
    },
    _ => {
        mod native;
        pub(crate) use native::*;
    }
}