cfg_select! {
    target_arch = "wasm32" => {
        mod wasm;
        pub use wasm::PlatformStorage;
        mod indexed_db;
        pub use indexed_db::AsyncPlatformStorage;
        mod buffered_web;
        pub use buffered_web::{BufferedStorage, initialize_runtime, runtime_status};
    },
    _ => {
        mod native;
        pub use native::PlatformStorage;
        mod async_native;
        pub use async_native::AsyncPlatformStorage;
        mod buffered_native;
        pub use buffered_native::{BufferedStorage, initialize_runtime, runtime_status};
    }
}
