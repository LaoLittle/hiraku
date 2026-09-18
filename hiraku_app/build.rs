fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("missing target OS");
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("missing target architecture");

    if target_os == "android" && target_arch == "aarch64" {
        println!("cargo::rustc-link-arg-cdylib=-Wl,-Bsymbolic");
    }
}
