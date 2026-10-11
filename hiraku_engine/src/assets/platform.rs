//! Filesystem-backed packages are optional; remote and browser sources use
//! Bevy's asset reader without assuming access to a local package file.
use bevy::asset::AssetPath;
use hiraku_hdp::{Archive, HdpError};
use std::path::Path;

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
pub(super) fn open_local(root: &Path, path: &AssetPath<'_>) -> Option<Result<Archive, HdpError>> {
    if path.source() != &bevy::asset::io::AssetSourceId::Default {
        return None;
    }
    let path = root.join(path.path());
    path.is_file().then(|| Archive::open(path))
}

#[cfg(target_os = "android")]
pub(super) fn open_local(_root: &Path, path: &AssetPath<'_>) -> Option<Result<Archive, HdpError>> {
    if path.source() != &bevy::asset::io::AssetSourceId::Default {
        return None;
    }
    crate::vfs::android_archive(path.path()).map(Ok)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn open_local(_root: &Path, _path: &AssetPath<'_>) -> Option<Result<Archive, HdpError>> {
    None
}
