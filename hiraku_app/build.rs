fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("missing target OS");

    match &*target_os {
        "android" | "linux" => {
            println!("cargo::rustc-link-arg-cdylib=-Wl,-Bsymbolic");
        }
        _ => {}
    }
}
