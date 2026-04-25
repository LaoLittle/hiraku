use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use bevy::{
    asset::io::{
        file::FileAssetReader, AssetReader, AssetReaderError, AssetSourceBuilder, PathStream,
        Reader, VecReader,
    },
    prelude::*,
};
use futures_lite::stream;
use serde::Deserialize;
use thiserror::Error;
use zip::{result::ZipError, ZipArchive};

pub const DEFAULT_ASSET_ROOT: &str = "assets";
pub const DEFAULT_SETTINGS_PATH: &str = "main.hdp!settings.toml";
pub const DEFAULT_STARTUP_SCRIPT: &str = "main.hdp!startup.rhai";
pub const ASSET_ROOT_PREFIX: &str = "assets:/";
pub const RESOURCE_ROOT_PREFIX: &str = "res:/";
pub const WORKSPACE_ROOT_PREFIX: &str = "workspace:/";
pub const DEFAULT_RESOURCE_ROOT: &str = "main.hdp!";
const WORKSPACE_ASSET_ALIAS: &str = "__hiraku_workspace_assets__/";
const WORKSPACE_ROOT_ALIAS: &str = "__hiraku_workspace_root__/";

#[derive(Resource, Clone)]
pub struct VfsResource(pub Arc<HdpVfs>);

#[derive(Debug, Clone)]
pub struct HdpVfs {
    root: PathBuf,
    settings_path: String,
    default_startup_script: String,
}

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to read archive: {0}")]
    Zip(#[from] ZipError),
    #[error("file is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("failed to parse settings.toml: {0}")]
    SettingsParse(#[from] toml::de::Error),
}

#[derive(Debug, Deserialize, Default)]
struct BootSection {
    startup: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SettingsFile {
    startup: Option<String>,
    ui_font: Option<String>,
    characters_dir: Option<String>,
    res_root: Option<String>,
    #[serde(default)]
    boot: BootSection,
}

impl HdpVfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::new_with_config(root, DEFAULT_SETTINGS_PATH, DEFAULT_STARTUP_SCRIPT)
    }

    pub fn new_with_config(
        root: impl Into<PathBuf>,
        settings_path: impl Into<String>,
        default_startup_script: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            settings_path: settings_path.into(),
            default_startup_script: default_startup_script.into(),
        }
    }

    pub fn settings_path(&self) -> &str {
        &self.settings_path
    }

    pub fn default_startup_script(&self) -> &str {
        &self.default_startup_script
    }

    pub fn load_startup_script_path(&self) -> Result<String, VfsError> {
        match self.load_settings_file() {
            Ok(settings) => {
                let startup = settings
                    .startup
                    .or(settings.boot.startup)
                    .unwrap_or_else(|| self.default_startup_script.clone());

                Ok(self.resolve_path(Some(&self.settings_path), &startup))
            }
            Err(VfsError::NotFound(_)) => Ok(self.default_startup_script.clone()),
            Err(err) => Err(err),
        }
    }

    pub fn load_ui_font_path(&self) -> Result<Option<String>, VfsError> {
        match self.load_settings_file() {
            Ok(settings) => Ok(settings
                .ui_font
                .map(|path| self.resolve_path(Some(&self.settings_path), &path))),
            Err(VfsError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn load_characters_dir_path(&self) -> Result<Option<String>, VfsError> {
        match self.load_settings_file() {
            Ok(settings) => Ok(settings
                .characters_dir
                .map(|path| self.resolve_path(Some(&self.settings_path), &path))),
            Err(VfsError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn load_resource_root_path(&self) -> Result<Option<String>, VfsError> {
        match self.load_settings_file() {
            Ok(settings) => Ok(settings
                .res_root
                .map(|path| self.resolve_resource_root_path(Some(&self.settings_path), &path))),
            Err(VfsError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn load_settings_file(&self) -> Result<SettingsFile, VfsError> {
        let settings_text = self.read_text(&self.settings_path)?;
        Ok(toml::from_str(&settings_text)?)
    }

    pub fn resolve_path(&self, base: Option<&str>, requested: &str) -> String {
        if let Some(stripped) = requested.strip_prefix(ASSET_ROOT_PREFIX) {
            return format!("{WORKSPACE_ASSET_ALIAS}{}", normalize_relative_path(Path::new(stripped)).to_string_lossy().replace('\\', "/"));
        }

        if let Some(stripped) = requested.strip_prefix(WORKSPACE_ROOT_PREFIX) {
            return format!("{WORKSPACE_ROOT_ALIAS}{}", normalize_relative_path(Path::new(stripped)).to_string_lossy().replace('\\', "/"));
        }

        if let Some(stripped) = requested.strip_prefix(RESOURCE_ROOT_PREFIX) {
            let root = self
                .load_resource_root_path()
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_RESOURCE_ROOT.to_string());
            return join_virtual_root(&root, stripped);
        }

        if requested.contains(".hdp!") {
            return normalize_virtual_asset_path(requested);
        }

        let requested_path = Path::new(requested);

        if let Some(base) = base
            && let Some((archive, entry)) = split_hdp_asset_path(base)
        {
            let mut internal = PathBuf::new();
            if let Some(parent) = Path::new(&entry).parent() {
                internal.push(parent);
            }
            internal.push(requested_path);

            let normalized = normalize_relative_path(&internal);
            let entry = normalized.to_string_lossy().replace('\\', "/");

            if entry.is_empty() {
                return archive;
            }

            return format!("{archive}!{entry}");
        }

        if let Some(base) = base {
            let mut joined = PathBuf::new();
            if let Some(parent) = Path::new(base).parent() {
                joined.push(parent);
            }
            joined.push(requested_path);
            return normalize_relative_path(&joined)
                .to_string_lossy()
                .replace('\\', "/");
        }

        normalize_virtual_asset_path(requested)
    }

    fn resolve_resource_root_path(&self, base: Option<&str>, requested: &str) -> String {
        if requested.contains(".hdp!") {
            return normalize_resource_root_path(requested);
        }

        let resolved = self.resolve_path(base, requested);
        normalize_resource_root_path(&resolved)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.read_bytes(path).is_ok()
    }

    pub fn read_text(&self, path: &str) -> Result<String, VfsError> {
        Ok(String::from_utf8(self.read_bytes(path)?)?)
    }

    pub fn read_bytes(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        if let Some(stripped) = path.strip_prefix(WORKSPACE_ASSET_ALIAS) {
            let full_path = FileAssetReader::get_base_path().join(DEFAULT_ASSET_ROOT).join(stripped);
            return std::fs::read(&full_path)
                .map_err(|err| map_fs_not_found(err, full_path.display().to_string()));
        }

        if let Some(stripped) = path.strip_prefix(WORKSPACE_ROOT_ALIAS) {
            let full_path = FileAssetReader::get_base_path().join(stripped);
            return std::fs::read(&full_path)
                .map_err(|err| map_fs_not_found(err, full_path.display().to_string()));
        }

        if let Some((archive, entry)) = split_hdp_asset_path(path) {
            let archive_path = self.root.join(&archive);
            let file = File::open(&archive_path)
                .map_err(|err| map_fs_not_found(err, archive_path.display().to_string()))?;
            let mut zip_archive = ZipArchive::new(file)?;
            let mut zip_file = zip_archive
                .by_name(&entry)
                .map_err(|err| map_zip_not_found(err, format!("{archive}!{entry}")))?;
            let mut bytes = Vec::new();
            zip_file.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }

        let full_path = self.root.join(path);
        std::fs::read(&full_path).map_err(|err| map_fs_not_found(err, full_path.display().to_string()))
    }

    fn list_virtual_directory(&self, path: &Path) -> Result<Vec<PathBuf>, AssetReaderError> {
        let Some((archive, entry)) = split_hdp_asset_path(&path.to_string_lossy()) else {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        };

        let archive_path = self.root.join(&archive);
        let file = File::open(&archive_path).map_err(|err| map_reader_fs_error(err, archive_path))?;
        let mut zip = ZipArchive::new(file).map_err(zip_to_reader_error)?;
        let directory_prefix = normalize_entry_prefix(&entry);
        let mut items = Vec::new();

        for index in 0..zip.len() {
            let zip_file = zip.by_index(index).map_err(zip_to_reader_error)?;
            let name = zip_file.name();

            if !name.starts_with(&directory_prefix) {
                continue;
            }

            let remainder = &name[directory_prefix.len()..];
            if remainder.is_empty() {
                continue;
            }

            let child_name = remainder.split('/').next().unwrap_or_default();
            if child_name.is_empty() {
                continue;
            }

            let child_path = if entry.is_empty() {
                format!("{archive}!{child_name}")
            } else {
                format!("{archive}!{entry}/{child_name}")
            };

            let child_path = PathBuf::from(normalize_virtual_asset_path(&child_path));
            if !items.contains(&child_path) {
                items.push(child_path);
            }
        }

        if items.is_empty() {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }

        Ok(items)
    }

    fn is_virtual_directory(&self, path: &Path) -> Result<bool, AssetReaderError> {
        match self.list_virtual_directory(path) {
            Ok(entries) => Ok(!entries.is_empty()),
            Err(AssetReaderError::NotFound(_)) => Ok(false),
            Err(err) => Err(err),
        }
    }
}

pub fn default_asset_source_builder(root: impl Into<String>) -> AssetSourceBuilder {
    let root = root.into();
    AssetSourceBuilder::new(move || Box::new(HdpAssetReader::new(root.clone())))
}

pub struct HdpAssetReader {
    vfs: HdpVfs,
    fallback: FileAssetReader,
}

impl HdpAssetReader {
    pub fn new(root: impl Into<String>) -> Self {
        let root = root.into();
        Self {
            vfs: HdpVfs::new(FileAssetReader::get_base_path().join(&root)),
            fallback: FileAssetReader::new(root),
        }
    }

    fn read_virtual_bytes(&self, path: &Path) -> Result<Vec<u8>, AssetReaderError> {
        self.vfs
            .read_bytes(&path.to_string_lossy())
            .map_err(vfs_to_reader_error)
    }
}

impl AssetReader for HdpAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if let Some(stripped) = path.to_string_lossy().strip_prefix(WORKSPACE_ASSET_ALIAS) {
            let full_path = FileAssetReader::get_base_path().join(DEFAULT_ASSET_ROOT).join(stripped);
            let bytes = std::fs::read(&full_path)
                .map_err(|err| map_reader_fs_error(err, full_path.clone()))?;
            return Ok(Box::new(VecReader::new(bytes)) as Box<dyn Reader>);
        }

        if let Some(stripped) = path.to_string_lossy().strip_prefix(WORKSPACE_ROOT_ALIAS) {
            let full_path = FileAssetReader::get_base_path().join(stripped);
            let bytes = std::fs::read(&full_path)
                .map_err(|err| map_reader_fs_error(err, full_path.clone()))?;
            return Ok(Box::new(VecReader::new(bytes)) as Box<dyn Reader>);
        }

        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            let bytes = self.read_virtual_bytes(path)?;
            return Ok(Box::new(VecReader::new(bytes)) as Box<dyn Reader>);
        }

        let reader = self.fallback.read(path).await?;
        Ok(Box::new(reader) as Box<dyn Reader>)
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if path.to_string_lossy().starts_with(WORKSPACE_ASSET_ALIAS)
            || path.to_string_lossy().starts_with(WORKSPACE_ROOT_ALIAS)
        {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }

        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }

        let reader = self.fallback.read_meta(path).await?;
        Ok(Box::new(reader) as Box<dyn Reader>)
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        if path.to_string_lossy().starts_with(WORKSPACE_ASSET_ALIAS)
            || path.to_string_lossy().starts_with(WORKSPACE_ROOT_ALIAS)
        {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }

        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            let items = self.vfs.list_virtual_directory(path)?;
            return Ok(Box::new(stream::iter(items)));
        }

        self.fallback.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        if path.to_string_lossy().starts_with(WORKSPACE_ASSET_ALIAS)
            || path.to_string_lossy().starts_with(WORKSPACE_ROOT_ALIAS)
        {
            return Ok(false);
        }

        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            return self.vfs.is_virtual_directory(path);
        }

        self.fallback.is_directory(path).await
    }
}

pub fn split_hdp_asset_path(raw: &str) -> Option<(String, String)> {
    let marker = ".hdp!";
    let index = raw.find(marker)?;
    let archive = normalize_relative_path(Path::new(&raw[..index + 4]))
        .to_string_lossy()
        .replace('\\', "/");
    let entry = normalize_relative_path(Path::new(&raw[index + marker.len()..]))
        .to_string_lossy()
        .replace('\\', "/");
    Some((archive, entry))
}

fn normalize_virtual_asset_path(path: &str) -> String {
    if let Some((archive, entry)) = split_hdp_asset_path(path) {
        if entry.is_empty() {
            archive
        } else {
            format!("{archive}!{entry}")
        }
    } else {
        normalize_relative_path(Path::new(path))
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn join_virtual_root(root: &str, requested: &str) -> String {
    let requested_path = Path::new(requested);

    if let Some((archive, entry)) = split_hdp_asset_path(root) {
        let mut internal = PathBuf::from(entry);
        internal.push(requested_path);
        let normalized = normalize_relative_path(&internal);
        let entry = normalized.to_string_lossy().replace('\\', "/");
        if entry.is_empty() {
            archive
        } else {
            format!("{archive}!{entry}")
        }
    } else {
        let mut combined = PathBuf::from(root);
        combined.push(requested_path);
        normalize_virtual_asset_path(&combined.to_string_lossy())
    }
}

fn normalize_resource_root_path(path: &str) -> String {
    if let Some((archive, entry)) = split_hdp_asset_path(path) {
        if entry.is_empty() {
            format!("{archive}!")
        } else {
            format!("{archive}!{}", entry.trim_matches('/'))
        }
    } else if path.ends_with(".hdp") {
        normalize_virtual_asset_path(&format!("{path}!")) + "!"
    } else {
        normalize_virtual_asset_path(path)
    }
}

fn normalize_entry_prefix(entry: &str) -> String {
    if entry.is_empty() {
        String::new()
    } else {
        format!("{}/", entry.trim_matches('/'))
    }
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }

    normalized
}

fn map_fs_not_found(error: std::io::Error, path: String) -> VfsError {
    if error.kind() == std::io::ErrorKind::NotFound {
        VfsError::NotFound(path)
    } else {
        VfsError::Io(error)
    }
}

fn map_zip_not_found(error: ZipError, path: String) -> VfsError {
    if matches!(error, ZipError::FileNotFound) {
        VfsError::NotFound(path)
    } else {
        VfsError::Zip(error)
    }
}

fn map_reader_fs_error(error: std::io::Error, path: PathBuf) -> AssetReaderError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AssetReaderError::NotFound(path)
    } else {
        AssetReaderError::from(error)
    }
}

fn zip_to_reader_error(error: ZipError) -> AssetReaderError {
    match error {
        ZipError::FileNotFound => AssetReaderError::NotFound(PathBuf::new()),
        other => AssetReaderError::from(std::io::Error::other(other.to_string())),
    }
}

fn vfs_to_reader_error(error: VfsError) -> AssetReaderError {
    match error {
        VfsError::NotFound(path) => AssetReaderError::NotFound(PathBuf::from(path)),
        VfsError::Io(error) => AssetReaderError::from(error),
        VfsError::Zip(error) => AssetReaderError::from(std::io::Error::other(error.to_string())),
        VfsError::Utf8(error) => AssetReaderError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )),
        VfsError::SettingsParse(error) => AssetReaderError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )),
    }
}
