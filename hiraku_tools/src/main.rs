use hiraku_tools::{
    ToolError,
    pack::{self, Options},
};
use std::path::PathBuf;

const HELP: &str = "hiraku-tools pack SOURCE OUTPUT [--uastc|--no-uastc] [--volume-size BYTES] [--chunk-size BYTES] [--compression zstd|none] [--compression-level N]\nUASTC is opt-in and requires --features uastc. Volume size 0 produces a single file. Source files are never modified.";

fn main() {
    if let Err(error) = run() {
        eprintln!("HDP packaging failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), ToolError> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().unwrap_or_default();
    if command == "--help" || command == "-h" {
        println!("{HELP}");
        return Ok(());
    }
    if command != "pack" {
        return Err(HELP.into());
    }
    let source = PathBuf::from(args.next().ok_or(HELP)?);
    let output = PathBuf::from(args.next().ok_or(HELP)?);
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        let flag = flag.to_str().ok_or("option must be UTF-8")?;
        if flag == "--uastc" {
            options.uastc = true;
            continue;
        }
        if flag == "--no-uastc" {
            options.uastc = false;
            continue;
        }
        if !matches!(
            flag,
            "--volume-size" | "--chunk-size" | "--compression" | "--compression-level"
        ) {
            return Err(format!("unknown option: {flag}\n{HELP}").into());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let value = value.to_str().ok_or("option value must be UTF-8")?;
        match flag {
            "--volume-size" => {
                let n = value.parse::<usize>()?;
                options.archive.max_volume_size = (n != 0).then_some(n);
            }
            "--chunk-size" => options.archive.chunk_size = value.parse()?,
            "--compression-level" => options.archive.compression.level = value.parse()?,
            "--compression" => {
                options.archive.compression.method = match value {
                    "zstd" => hiraku_hdp::CompressionMethod::ZSTD,
                    "none" => hiraku_hdp::CompressionMethod::STORED,
                    _ => return Err("compression must be zstd or none".into()),
                }
            }
            _ => unreachable!("validated option"),
        }
    }
    pack::directory(&source, &output, options)?;
    Ok(())
}
