use std::path::{Path, PathBuf};

use bevy::prelude::App;

#[cfg(target_os = "android")]
mod android;

pub(crate) fn register_asset_source(app: &mut App, root: &str) {
    #[cfg(target_os = "android")]
    android::register_asset_source(app, root);
    #[cfg(not(target_os = "android"))]
    let _ = (app, root);
}

#[cfg(target_os = "android")]
pub(crate) fn android_archive(path: &Path) -> Option<hiraku_hdp::Archive> {
    android::selected_archive(path)
}

pub(super) fn read(root: &Path, path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(target_os = "android")]
    return android::read(root, path);
    #[cfg(not(target_os = "android"))]
    std::fs::read(root.join(path))
}

pub(super) fn list_files(
    root: &Path,
    path: &Path,
    recursive: bool,
) -> std::io::Result<Vec<PathBuf>> {
    #[cfg(target_os = "android")]
    return android::list_files(root, path, recursive);
    #[cfg(not(target_os = "android"))]
    {
        let mut files = Vec::new();
        collect_files(root, path, recursive, &mut files)?;
        Ok(files)
    }
}

#[cfg(not(target_os = "android"))]
fn collect_files(
    root: &Path,
    path: &Path,
    recursive: bool,
    files: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root.join(path))? {
        let entry = entry?;
        let child = path.join(entry.file_name());
        if entry.file_type()?.is_file() {
            files.push(child);
        } else if recursive && entry.file_type()?.is_dir() {
            collect_files(root, &child, true, files)?;
        }
    }
    Ok(())
}
