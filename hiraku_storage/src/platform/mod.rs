cfg_select! {
    target_arch = "wasm32" => {
        mod wasm;
        pub use wasm::PlatformStorage;
    },
    _ => {
        mod native;
        pub use native::PlatformStorage;
    }
}
