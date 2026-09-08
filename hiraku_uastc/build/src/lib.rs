//! Host-only asset pipeline. Depend on this crate from `[build-dependencies]`.
use hiraku_hdp::{FileOptions, PackOptions, StreamPackageBuilder, WrittenPackage};
use hiraku_script::hson::{self, HsonValue};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
mod encoder;
pub mod progress;
#[cfg(test)]
mod tests;
pub use encoder::{UASTC_LEVEL, encode_rgba, encode_rgba_with_threads, encoder_threads};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn collect(root: &Path, at: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(at)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            return Err(format!(
                "asset symlinks are not supported: {}",
                entry.path().display()
            )
            .into());
        }
        if entry.file_type()?.is_dir() {
            collect(root, &entry.path(), out)?;
        } else {
            out.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga" | "ktx2" | "dds" | "basis"
            )
        })
}

/// Rewrites packaged descriptors only. Source files are never modified.
/// Every canonical source texture is encoded once, even across descriptors.
pub fn pack_directory(
    root: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: PackOptions,
) -> Result<WrittenPackage> {
    pack_directory_with_progress(root, output, options, |_| {})
}

/// Report phase/file progress without choosing a terminal or logging framework.
pub fn pack_directory_with_progress(
    root: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: PackOptions,
    mut report: impl FnMut(progress::PackProgress),
) -> Result<WrittenPackage> {
    use progress::PackProgress;
    report(PackProgress::new("scan", 0, 0, "Discovering assets"));
    let root = root.as_ref().canonicalize()?;
    let mut paths = Vec::new();
    collect(&root, &root, &mut paths)?;
    paths.sort();
    let mut textures = BTreeMap::<PathBuf, String>::new();
    let mut manifests = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        if !path.to_string_lossy().ends_with(".texture.hson") {
            continue;
        }
        report(PackProgress::new(
            "manifest",
            index,
            paths.len(),
            path.display().to_string(),
        ));
        let mut value: HsonValue = hson::from_slice(&fs::read(root.join(path))?)?;
        let HsonValue::Map(fields) = &mut value else {
            return Err(format!("{}: expected texture map", path.display()).into());
        };
        let Some(HsonValue::String(image)) = fields.get_mut("image") else {
            return Err(format!("{}: expected image path", path.display()).into());
        };
        if image.contains("://") || Path::new(image).is_absolute() {
            return Err(format!(
                "{}: texture must use a package-relative path",
                path.display()
            )
            .into());
        }
        let source = root
            .join(path.parent().unwrap_or(Path::new("")))
            .join(&*image)
            .canonicalize()?;
        let relative = source
            .strip_prefix(&root)
            .map_err(|_| "texture escapes package root")?;
        if !source.is_file() {
            return Err("texture is not a file".into());
        }
        let identity = relative.to_string_lossy().replace('\\', "/");
        let target = textures.entry(source).or_insert_with(|| {
            format!(
                "textures/encoded/{}.uastc.ktx2",
                blake3::hash(identity.as_bytes()).to_hex()
            )
        });
        let depth = path
            .parent()
            .unwrap_or(Path::new(""))
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count();
        *image = format!("{}{target}", "../".repeat(depth));
        manifests.insert(path.clone(), hson::to_vec(&value)?);
    }
    report(PackProgress::new(
        "plan",
        0,
        textures.len(),
        format!(
            "{} files, {} texture manifests, {} unique referenced textures",
            paths.len(),
            manifests.len(),
            textures.len()
        ),
    ));
    let mut writer = StreamPackageBuilder::new(options)?;
    for (index, path) in paths.iter().enumerate() {
        let name = path.to_string_lossy().replace('\\', "/");
        let options = FileOptions {
            bootstrap: name.ends_with(".hks") || name.ends_with(".hson"),
            ..Default::default()
        };
        if let Some(bytes) = manifests.get(path) {
            report(PackProgress::new("assets", index, paths.len(), &name));
            writer.add_reader(&name, bytes.as_slice(), options)?;
        } else if !image_path(path) {
            report(PackProgress::new("assets", index, paths.len(), &name));
            writer.add_reader(&name, fs::File::open(root.join(path))?, options)?;
        }
    }
    let texture_count = textures.len();
    for (index, (source, target)) in textures.into_iter().enumerate() {
        let name = source.strip_prefix(&root)?.display().to_string();
        report(PackProgress::new("image", index, texture_count, &name));
        if source.to_string_lossy().ends_with(".uastc.ktx2") {
            report(PackProgress::new("compress", index, texture_count, &name));
            writer.add_reader(&target, fs::File::open(source)?, FileOptions::default())?;
        } else {
            let image = image::open(&source)
                .map_err(|error| format!("{}: {error}", source.display()))?
                .to_rgba8();
            report(PackProgress::new(
                "encode",
                index,
                texture_count,
                format!(
                    "{name} ({}x{}, {:.1} MiB RGBA; level {}, {} threads)",
                    image.width(),
                    image.height(),
                    image.as_raw().len() as f64 / 1048576.0,
                    UASTC_LEVEL,
                    encoder_threads(),
                ),
            ));
            let encoded = encode_rgba(image.as_raw(), image.width(), image.height())?;
            report(PackProgress::new(
                "compress",
                index,
                texture_count,
                format!("{name} ({:.1} MiB KTX2)", encoded.len() as f64 / 1048576.0),
            ));
            writer.add_reader(&target, encoded.as_slice(), FileOptions::default())?;
        }
        report(PackProgress::new(
            "texture-done",
            index + 1,
            texture_count,
            name,
        ));
    }
    report(PackProgress::new(
        "publish",
        0,
        0,
        output.as_ref().display().to_string(),
    ));
    let package = writer.write_to(output)?;
    report(PackProgress::new(
        "done",
        texture_count,
        texture_count,
        format!(
            "{} volumes, {:.1} MiB HDP",
            package.volume_count(),
            package.stored_size() as f64 / 1048576.0
        ),
    ));
    Ok(package)
}
