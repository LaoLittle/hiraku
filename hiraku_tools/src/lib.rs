//! Build-only asset tooling. Never depend on this crate from a runtime.
mod analysis;
pub mod pack;
pub use analysis::analyze;
use hiraku_hdp::dependencies::DEPENDENCY_MANIFEST;
use hiraku_hdp::{FileOptions, PackOptions, StreamPackageBuilder, WrittenPackage};
use std::{collections::BTreeMap, error::Error, fs, path::Path};

pub type ToolError = Box<dyn Error + Send + Sync>;

fn estimate_images(
    source: &Path,
    manifest: &mut hiraku_hdp::dependencies::DependencyManifest,
) -> Result<(), ToolError> {
    use std::io::Read;
    let paths: std::collections::BTreeSet<_> = manifest
        .scripts
        .values()
        .flatten()
        .chain(manifest.resident.iter())
        .collect();
    for path in paths {
        if !path.to_ascii_lowercase().ends_with(".png") {
            continue;
        }
        let mut header = [0; 24];
        let Ok(mut file) = fs::File::open(source.join(path)) else {
            continue;
        };
        if file.read_exact(&mut header).is_err()
            || &header[..8] != b"\x89PNG\r\n\x1a\n"
            || &header[12..16] != b"IHDR"
        {
            continue;
        }
        let width = u32::from_be_bytes(header[16..20].try_into().expect("PNG width")) as u64;
        let height = u32::from_be_bytes(header[20..24].try_into().expect("PNG height")) as u64;
        if let Some(bytes) = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n > 0)
        {
            manifest.image_bytes.insert(path.clone(), bytes);
        }
    }
    Ok(())
}

/// Inspect metadata without encoding images or producing a package.
pub fn analyze_directory(
    source: &Path,
) -> Result<hiraku_hdp::dependencies::DependencyManifest, ToolError> {
    let mut paths = Vec::new();
    collect(source, source, &mut paths)?;
    let mut documents = BTreeMap::new();
    for path in paths {
        if path.ends_with(".hks") || path.ends_with(".hson") {
            documents.insert(path.clone(), fs::read_to_string(source.join(path))?);
        }
    }
    let mut manifest = analyze(&documents)?;
    estimate_images(source, &mut manifest)?;
    Ok(manifest)
}

/// Generates one root manifest without modifying source assets. Asset bytes
/// still stream through HDP's bounded-memory compression spool.
pub fn pack_directory(
    source: &Path,
    output: &Path,
    options: PackOptions,
) -> Result<WrittenPackage, ToolError> {
    let mut paths = Vec::new();
    collect(source, source, &mut paths)?;
    paths.sort();
    if paths.iter().any(|p| p == DEPENDENCY_MANIFEST) {
        return Err(format!("{DEPENDENCY_MANIFEST} is generated; remove the source copy").into());
    }
    let mut documents = BTreeMap::new();
    for path in &paths {
        if path.ends_with(".hks") || path.ends_with(".hson") {
            documents.insert(path.clone(), fs::read_to_string(source.join(path))?);
        }
    }
    let mut manifest = analyze(&documents)?;
    estimate_images(source, &mut manifest)?;
    for (script, expressions) in &manifest.conservative {
        eprintln!("[HDP dependencies] {script}: unresolved references {expressions:?} (on-demand)");
    }
    let encoded = hiraku_script::hson::to_string(&manifest)?;
    let mut builder = StreamPackageBuilder::new(options)?;
    builder.add_reader(
        DEPENDENCY_MANIFEST,
        encoded.as_bytes(),
        FileOptions {
            bootstrap: true,
            ..Default::default()
        },
    )?;
    for path in paths {
        builder.add_reader(
            &path,
            fs::File::open(source.join(&path))?,
            FileOptions {
                bootstrap: path.ends_with(".hks") || path.ends_with(".hson"),
                ..Default::default()
            },
        )?;
    }
    Ok(builder.write_to(output)?)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), ToolError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(
                format!("symlink is not a package input: {}", entry.path().display()).into(),
            );
        }
        if ty.is_dir() {
            collect(root, &entry.path(), out)?;
        } else if ty.is_file() {
            out.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .ok_or("asset path must be UTF-8")?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_estimates_read_only_the_png_header_and_unknown_formats_stay_lazy() {
        let dir = tempfile::tempdir().expect("fixture");
        let mut header = [0u8; 24];
        header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        header[12..16].copy_from_slice(b"IHDR");
        header[16..20].copy_from_slice(&8192u32.to_be_bytes());
        header[20..24].copy_from_slice(&8192u32.to_be_bytes());
        fs::write(dir.path().join("alice.png"), header).expect("header fixture");
        let mut manifest = analyze(&BTreeMap::from([
            (
                "alice.texture.hson".into(),
                ".{ name: \"alice\", image: \"alice.png\" }".into(),
            ),
            ("scene.hks".into(), "bg(\"alice\")".into()),
        ]))
        .expect("analyze fixture");
        estimate_images(dir.path(), &mut manifest).expect("estimate");
        assert_eq!(manifest.image_bytes["alice.png"], 256 * 1024 * 1024);
        fs::write(dir.path().join("alice.png"), b"invalid").expect("invalid header");
        manifest.image_bytes.clear();
        estimate_images(dir.path(), &mut manifest).expect("unknown size is lazy");
        assert!(manifest.image_bytes.is_empty());
    }
    #[test]
    fn generated_manifest_is_unique_and_available_in_bootstrap_volume() {
        let dir = tempfile::tempdir().expect("temporary package fixture");
        let source = dir.path().join("source");
        fs::create_dir(&source).expect("source directory");
        fs::write(source.join("startup.hks"), "bg(\"room\")").expect("script");
        fs::write(
            source.join("room.texture.hson"),
            ".{ name: \"room\", image: \"room.png\" }",
        )
        .expect("descriptor");
        fs::write(source.join("room.png"), b"synthetic image bytes").expect("image payload");
        let output = dir.path().join("test.hdp");
        pack_directory(&source, &output, PackOptions::default()).expect("stream package");
        let archive =
            hiraku_hdp::Archive::from_first_volume(fs::read(output).expect("volume zero"))
                .expect("open volume zero");
        assert_eq!(
            archive
                .files()
                .filter(|p| p.ends_with(".manifest.hson"))
                .count(),
            1
        );
        let manifest: hiraku_hdp::dependencies::DependencyManifest =
            hiraku_script::hson::from_slice(
                &archive
                    .read_file(DEPENDENCY_MANIFEST)
                    .expect("bootstrap manifest"),
            )
            .expect("manifest schema");
        assert!(manifest.scripts["startup.hks"].contains("room.png"));
        assert!(!source.join(DEPENDENCY_MANIFEST).exists());
        assert_eq!(
            archive.read_file("room.png").expect("original image"),
            b"synthetic image bytes"
        );
    }
}
