//! Host-side packaging policy shared by build scripts and the command line.
use crate::ToolError;
use hiraku_hdp::{PackOptions, WrittenPackage};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub archive: PackOptions,
    /// Encode referenced textures as level-3 UASTC and rewrite packaged descriptors.
    pub uastc: bool,
}

pub fn directory(
    source: &Path,
    output: &Path,
    options: Options,
) -> Result<WrittenPackage, ToolError> {
    // Never ingest a previous output package on subsequent builds.
    let source = source.canonicalize()?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    if parent.canonicalize()?.starts_with(&source) {
        return Err("package output must be outside the source directory".into());
    }
    if options.uastc {
        #[cfg(feature = "uastc")]
        {
            let manifest = crate::analyze_directory(&source)?;
            let progress = hiraku_uastc_build::progress::TerminalProgress::new();
            let result = hiraku_uastc_build::pack_directory_with_progress(
                &source,
                output,
                options.archive,
                manifest,
                |event| progress.update(event),
            );
            progress.finish(result.as_ref().err().map(ToString::to_string));
            return result;
        }
        #[cfg(not(feature = "uastc"))]
        return Err("UASTC requires building hiraku-tools with --features uastc".into());
    }
    eprintln!("[HDP] Packing {} -> {}", source.display(), output.display());
    let result = crate::pack_directory(&source, output, options.archive)?;
    eprintln!("[HDP] Complete");
    Ok(result)
}
