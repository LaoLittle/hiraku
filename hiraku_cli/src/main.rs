use std::{error::Error, path::PathBuf};

use clap::{Parser, Subcommand};
use hiraku_hdp::{
    Archive, CompressionMethod, CompressionOptions, FileOptions, PackOptions, pack_directory_with,
    write_package,
};

#[derive(Parser)]
#[command(name = "hiraku", about = "Hiraku content tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a Hiraku Data Package from a directory.
    HdpPack {
        source: PathBuf,
        output: PathBuf,
        /// Maximum physical volume size in bytes. Omit for one desktop file.
        #[arg(long)]
        volume_size: Option<usize>,
        #[arg(long, default_value_t = 1024 * 1024)]
        chunk_size: usize,
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
        /// Path or directory prefix that must be stored in volume zero.
        #[arg(long)]
        bootstrap: Vec<String>,
    },
    /// Print the index of an HDP package.
    HdpList { package: PathBuf },
    /// Decode and checksum every file in an HDP package.
    HdpVerify { package: PathBuf },
}

fn main() -> Result<(), Box<dyn Error>> {
    match Cli::parse().command {
        Command::HdpPack {
            source,
            output,
            volume_size,
            chunk_size,
            zstd_level,
            bootstrap,
        } => {
            let package = pack_directory_with(
                source,
                PackOptions {
                    chunk_size,
                    max_volume_size: volume_size,
                    compression: CompressionOptions {
                        method: CompressionMethod::ZSTD,
                        level: zstd_level,
                        ..Default::default()
                    },
                },
                |path| FileOptions {
                    bootstrap: bootstrap.iter().any(|prefix| path_matches(path, prefix)),
                    ..Default::default()
                },
            )?;
            write_package(&output, &package)?;
            let stored = package.volumes.iter().map(Vec::len).sum::<usize>();
            let decoded = package
                .index
                .files
                .values()
                .map(|file| file.decoded_size)
                .sum::<u64>();
            println!(
                "packed {} files into {} volume(s): {} -> {} bytes",
                package.index.files.len(),
                package.volumes.len(),
                decoded,
                stored
            );
        }
        Command::HdpList { package } => {
            let archive = Archive::open(package)?;
            for file in archive.index().files.values() {
                let stored = file
                    .chunks
                    .iter()
                    .map(|chunk| chunk.stored_size)
                    .sum::<u64>();
                println!(
                    "{}\t{}\t{}\t{} chunk(s)",
                    file.path,
                    file.decoded_size,
                    stored,
                    file.chunks.len()
                );
            }
        }
        Command::HdpVerify { package } => {
            let archive = Archive::open(package)?;
            for path in archive.files() {
                archive.read_file(path)?;
            }
            println!(
                "verified {} files across {} volume(s)",
                archive.index().files.len(),
                archive.index().volume_count
            );
        }
    }
    Ok(())
}

fn path_matches(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_matches('/');
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}
