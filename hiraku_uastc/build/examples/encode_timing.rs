//! Offline timing only: no game launch and no output asset files.
use hiraku_uastc_build::{UASTC_LEVEL, encode_rgba_with_threads, encoder_threads};
use std::time::Instant;

fn main() -> hiraku_uastc_build::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: encode_timing IMAGE [THREADS]")?;
    let threads = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or_else(encoder_threads);
    let decode = Instant::now();
    let image = image::open(&path)?.to_rgba8();
    println!(
        "Image {}x{}: decode {:.3}s",
        image.width(),
        image.height(),
        decode.elapsed().as_secs_f64()
    );
    let encode = Instant::now();
    let bytes = encode_rgba_with_threads(image.as_raw(), image.width(), image.height(), threads)?;
    println!(
        "UASTC level {UASTC_LEVEL}, {threads} threads: {:.3}s, {} bytes (KTX2, no Zstd)",
        encode.elapsed().as_secs_f64(),
        bytes.len()
    );
    Ok(())
}
