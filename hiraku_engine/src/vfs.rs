use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use bevy::{
    asset::io::{
        AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, VecReader,
        file::FileAssetReader,
    },
    prelude::*,
};
use futures_lite::stream;
use rhai::Map;
use thiserror::Error;
use zip::{ZipArchive, result::ZipError};

use crate::data::evaluate_rhai_map;

pub const DEFAULT_ASSET_ROOT: &str = "assets";
pub const DEFAULT_SETTINGS_PATH: &str = "hdp://main.hdp/settings.rhai";
pub const DEFAULT_STARTUP_SCRIPT: &str = "hdp://main.hdp/startup.rhai";
pub const ASSET_ROOT_PREFIX: &str = "assets:/";
pub const RESOURCE_ROOT_PREFIX: &str = "res:/";
pub const WORKSPACE_ROOT_PREFIX: &str = "workspace:/";
pub const DEFAULT_RESOURCE_ROOT: &str = "hdp://main.hdp/";
pub const HDP_SOURCE_ID: &str = "hdp";
pub const ASSET_SOURCE_ID: &str = "assets";
pub const WORKSPACE_SOURCE_ID: &str = "workspace";
pub const DEFAULT_BACKGROUNDS_DIR: &str = "backgrounds";
pub const DEFAULT_BGM_DIR: &str = "bgm";
pub const DEFAULT_SOUNDEFFECTS_DIR: &str = "soundeffects";
pub const DEFAULT_VOICE_DIR: &str = "voice";
pub const DEFAULT_CHARACTERS_DIR: &str = "characters";
pub const DEFAULT_FONTS_DIR: &str = "fonts";
pub const DEFAULT_TEXTURES_DIR: &str = "textures";

pub fn workspace_base_path() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| FileAssetReader::get_base_path())
}

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
    #[error("failed to load settings data `{path}`: {message}")]
    SettingsData { path: String, message: String },
}

#[derive(Debug, Default)]
struct BootSection {
    startup: Option<String>,
}

#[derive(Debug, Default)]
struct SettingsFile {
    startup: Option<String>,
    fonts: FontsSettings,
    backgrounds_dir: Option<String>,
    soundeffects_dir: Option<String>,
    bgm_dir: Option<String>,
    voice_dir: Option<String>,
    characters_dir: Option<String>,
    textures_dir: Option<String>,
    res_root: Option<String>,
    boot: BootSection,
}

#[derive(Debug, Default)]
struct FontsSettings {
    path: Option<String>,
}

fn settings_from_data(mut data: Map, path: &str) -> Result<SettingsFile, VfsError> {
    let startup = take_data_string(&mut data, "startup", path)?;
    let backgrounds_dir = take_data_string(&mut data, "backgrounds_dir", path)?;
    let soundeffects_dir = take_data_string(&mut data, "soundeffects_dir", path)?;
    let bgm_dir = take_data_string(&mut data, "bgm_dir", path)?;
    let voice_dir = take_data_string(&mut data, "voice_dir", path)?;
    let characters_dir = take_data_string(&mut data, "characters_dir", path)?;
    let textures_dir = take_data_string(&mut data, "textures_dir", path)?;
    let res_root = take_data_string(&mut data, "res_root", path)?;

    let fonts = if let Some(mut fonts) = take_data_map(&mut data, "fonts", path)? {
        let path_value = take_data_string(&mut fonts, "path", path)?;
        ensure_empty_data_map(fonts, path, "fonts")?;
        FontsSettings { path: path_value }
    } else {
        FontsSettings::default()
    };

    let boot = if let Some(mut boot) = take_data_map(&mut data, "boot", path)? {
        let startup = take_data_string(&mut boot, "startup", path)?;
        ensure_empty_data_map(boot, path, "boot")?;
        BootSection { startup }
    } else {
        BootSection::default()
    };

    ensure_empty_data_map(data, path, "settings")?;
    Ok(SettingsFile {
        startup,
        fonts,
        backgrounds_dir,
        soundeffects_dir,
        bgm_dir,
        voice_dir,
        characters_dir,
        textures_dir,
        res_root,
        boot,
    })
}

fn take_data_string(data: &mut Map, key: &str, path: &str) -> Result<Option<String>, VfsError> {
    data.remove(key)
        .map(|value| {
            value
                .try_cast::<String>()
                .ok_or_else(|| invalid_settings_data(path, format!("`{key}` must be a string")))
        })
        .transpose()
}

fn take_data_map(data: &mut Map, key: &str, path: &str) -> Result<Option<Map>, VfsError> {
    data.remove(key)
        .map(|value| {
            value
                .try_cast::<Map>()
                .ok_or_else(|| invalid_settings_data(path, format!("`{key}` must be a map")))
        })
        .transpose()
}

fn ensure_empty_data_map(data: Map, path: &str, section: &str) -> Result<(), VfsError> {
    if data.is_empty() {
        return Ok(());
    }

    let keys = data.keys().cloned().collect::<Vec<_>>().join(", ");
    Err(invalid_settings_data(
        path,
        format!("unknown {section} setting(s): {keys}"),
    ))
}

fn invalid_settings_data(path: &str, message: String) -> VfsError {
    VfsError::SettingsData {
        path: path.to_string(),
        message,
    }
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

    pub fn load_fonts_dir_path(&self) -> Result<String, VfsError> {
        let settings = match self.load_settings_file() {
            Ok(settings) => settings,
            Err(VfsError::NotFound(_)) => SettingsFile::default(),
            Err(err) => return Err(err),
        };
        let path = settings
            .fonts
            .path
            .unwrap_or_else(|| DEFAULT_FONTS_DIR.to_string());
        Ok(self.resolve_path(Some(&self.settings_path), &path))
    }

    pub fn load_font_paths(&self) -> Result<Vec<String>, VfsError> {
        let directory = self.load_fonts_dir_path()?;
        let mut paths = self
            .list_directory(&directory)?
            .into_iter()
            .filter(|path| {
                matches!(
                    Path::new(path)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map(|extension| extension.to_ascii_lowercase())
                        .as_deref(),
                    Some("otf") | Some("ttf")
                )
            })
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| {
            let lower = path.to_ascii_lowercase();
            let priority = if lower.contains("sourcehanserifsc") {
                0
            } else if lower.contains("sourcehansanssc") {
                1
            } else if lower.contains("sourcehansansjp") {
                2
            } else if lower.contains("regular") {
                3
            } else {
                4
            };
            (priority, lower)
        });
        Ok(paths)
    }

    pub fn load_characters_dir_path(&self) -> Result<Option<String>, VfsError> {
        Ok(Some(self.load_directory_path(
            None,
            |settings| settings.characters_dir.clone(),
            DEFAULT_CHARACTERS_DIR,
        )?))
    }

    pub fn load_textures_dir_path(&self) -> Result<String, VfsError> {
        self.load_directory_path(
            None,
            |settings| settings.textures_dir.clone(),
            DEFAULT_TEXTURES_DIR,
        )
    }

    pub fn load_backgrounds_dir_path(&self, base: Option<&str>) -> Result<String, VfsError> {
        self.load_directory_path(
            base,
            |settings| settings.backgrounds_dir.clone(),
            DEFAULT_BACKGROUNDS_DIR,
        )
    }

    pub fn load_bgm_dir_path(&self, base: Option<&str>) -> Result<String, VfsError> {
        self.load_directory_path(base, |settings| settings.bgm_dir.clone(), DEFAULT_BGM_DIR)
    }

    pub fn load_soundeffects_dir_path(&self, base: Option<&str>) -> Result<String, VfsError> {
        self.load_directory_path(
            base,
            |settings| settings.soundeffects_dir.clone(),
            DEFAULT_SOUNDEFFECTS_DIR,
        )
    }

    pub fn load_voice_dir_path(&self, base: Option<&str>) -> Result<String, VfsError> {
        self.load_directory_path(
            base,
            |settings| settings.voice_dir.clone(),
            DEFAULT_VOICE_DIR,
        )
    }

    pub fn resolve_background_path(
        &self,
        base: Option<&str>,
        requested: &str,
    ) -> Result<String, VfsError> {
        self.resolve_content_path(base, requested, |vfs, base| {
            vfs.load_backgrounds_dir_path(base)
        })
    }

    pub fn resolve_bgm_path(
        &self,
        base: Option<&str>,
        requested: &str,
    ) -> Result<String, VfsError> {
        self.resolve_content_path(base, requested, |vfs, base| vfs.load_bgm_dir_path(base))
    }

    pub fn resolve_soundeffect_path(
        &self,
        base: Option<&str>,
        requested: &str,
    ) -> Result<String, VfsError> {
        self.resolve_content_path(base, requested, |vfs, base| {
            vfs.load_soundeffects_dir_path(base)
        })
    }

    pub fn resolve_voice_path(
        &self,
        base: Option<&str>,
        requested: &str,
    ) -> Result<String, VfsError> {
        self.resolve_content_path(base, requested, |vfs, base| vfs.load_voice_dir_path(base))
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
        self.load_settings_file_at(None)
    }

    fn load_settings_file_at(&self, base: Option<&str>) -> Result<SettingsFile, VfsError> {
        let settings_path = self.settings_path_for_base(base);
        let settings_text = self.read_text(&settings_path)?;
        let data = evaluate_rhai_map(&settings_path, &settings_text).map_err(|error| {
            VfsError::SettingsData {
                path: settings_path.clone(),
                message: error.to_string(),
            }
        })?;
        settings_from_data(data, &settings_path)
    }

    fn settings_path_for_base(&self, base: Option<&str>) -> String {
        if let Some(base) = base
            && let Some((archive, _)) = split_hdp_asset_path(base)
        {
            return format!("hdp://{archive}/{}", self.settings_file_name());
        }

        self.settings_path.clone()
    }

    fn settings_file_name(&self) -> &str {
        Path::new(&self.settings_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings.rhai")
    }

    fn load_directory_path(
        &self,
        base: Option<&str>,
        configured: impl FnOnce(&SettingsFile) -> Option<String>,
        default: &str,
    ) -> Result<String, VfsError> {
        let settings_path = self.settings_path_for_base(base);
        let settings = match self.load_settings_file_at(base) {
            Ok(settings) => settings,
            Err(VfsError::NotFound(_)) => SettingsFile::default(),
            Err(err) => return Err(err),
        };
        let directory = configured(&settings).unwrap_or_else(|| default.to_string());
        Ok(self.resolve_path(Some(&settings_path), &directory))
    }

    fn resolve_content_path(
        &self,
        base: Option<&str>,
        requested: &str,
        directory: impl FnOnce(&Self, Option<&str>) -> Result<String, VfsError>,
    ) -> Result<String, VfsError> {
        if is_explicit_asset_uri(requested) {
            return Ok(self.resolve_path(base, requested));
        }

        let root = directory(self, base)?;
        Ok(join_virtual_root(&root, requested))
    }

    pub fn resolve_path(&self, base: Option<&str>, requested: &str) -> String {
        if let Some(stripped) = requested.strip_prefix(ASSET_ROOT_PREFIX) {
            return format!(
                "{ASSET_SOURCE_ID}://{}",
                normalize_relative_path(Path::new(stripped))
                    .to_string_lossy()
                    .replace('\\', "/")
            );
        }

        if let Some(stripped) = requested.strip_prefix(WORKSPACE_ROOT_PREFIX) {
            return format!(
                "{WORKSPACE_SOURCE_ID}://{}",
                normalize_relative_path(Path::new(stripped))
                    .to_string_lossy()
                    .replace('\\', "/")
            );
        }

        if requested.starts_with("hdp://")
            || requested.starts_with("assets://")
            || requested.starts_with("workspace://")
        {
            return normalize_virtual_asset_path(requested);
        }

        if let Some(stripped) = requested.strip_prefix(RESOURCE_ROOT_PREFIX) {
            let root = self
                .load_resource_root_path()
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_RESOURCE_ROOT.to_string());
            return join_virtual_root(&root, stripped);
        }

        if split_hdp_asset_path(requested).is_some() {
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

            return format!("hdp://{archive}/{entry}");
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
        if split_hdp_asset_path(requested).is_some() {
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
        if let Some(stripped) = path.strip_prefix("assets://") {
            let full_path = workspace_base_path()
                .join(DEFAULT_ASSET_ROOT)
                .join(stripped);
            return std::fs::read(&full_path)
                .map_err(|err| map_fs_not_found(err, full_path.display().to_string()));
        }

        if let Some(stripped) = path.strip_prefix("workspace://") {
            let full_path = workspace_base_path().join(stripped);
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
        std::fs::read(&full_path)
            .map_err(|err| map_fs_not_found(err, full_path.display().to_string()))
    }

    pub fn list_directory(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if let Some((archive, entry)) = split_hdp_asset_path(path) {
            return self
                .list_virtual_directory(Path::new(&format!("{archive}/{entry}")))
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|path| format!("hdp://{}", path.to_string_lossy()))
                        .collect()
                })
                .map_err(reader_to_vfs_error);
        }

        let (source, directory) = split_asset_source_uri(path)
            .map(|(source, directory)| (Some(source), PathBuf::from(directory)))
            .unwrap_or((None, PathBuf::from(path)));
        let full_path = match source {
            Some(ASSET_SOURCE_ID) => workspace_base_path()
                .join(DEFAULT_ASSET_ROOT)
                .join(&directory),
            Some(WORKSPACE_SOURCE_ID) => workspace_base_path().join(&directory),
            _ => self.root.join(&directory),
        };
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&full_path)
            .map_err(|err| map_fs_not_found(err, full_path.display().to_string()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let child = entry.path();
            let name = child
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| VfsError::NotFound(child.display().to_string()))?;
            let path = match source {
                Some(source) => format!("{source}://{}/{}", directory.to_string_lossy(), name),
                None => self
                    .root
                    .join(&directory)
                    .join(name)
                    .strip_prefix(&self.root)
                    .unwrap_or(child.as_path())
                    .to_string_lossy()
                    .replace('\\', "/"),
            };
            paths.push(path);
        }
        Ok(paths)
    }

    pub fn list_files_recursive(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if let Some((archive, entry)) = split_hdp_asset_path(path) {
            let archive_path = self.root.join(&archive);
            let file = File::open(&archive_path)
                .map_err(|err| map_fs_not_found(err, archive_path.display().to_string()))?;
            let mut zip = ZipArchive::new(file)?;
            let prefix = normalize_entry_prefix(&entry);
            let mut paths = Vec::new();
            for index in 0..zip.len() {
                let zip_file = zip.by_index(index)?;
                let name = zip_file.name();
                if !name.ends_with('/') && name.starts_with(&prefix) {
                    paths.push(format!("hdp://{archive}/{name}"));
                }
            }
            return Ok(paths);
        }

        let full_path = self.root.join(path);
        let mut paths = Vec::new();
        collect_files_recursive(&full_path, &self.root, &mut paths)?;
        Ok(paths)
    }

    fn list_virtual_directory(&self, path: &Path) -> Result<Vec<PathBuf>, AssetReaderError> {
        let Some((archive, entry)) = split_hdp_asset_path(&path.to_string_lossy()) else {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        };

        let archive_path = self.root.join(&archive);
        let file =
            File::open(&archive_path).map_err(|err| map_reader_fs_error(err, archive_path))?;
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
                format!("{archive}/{child_name}")
            } else {
                format!("{archive}/{entry}/{child_name}")
            };

            let child_path = PathBuf::from(normalize_reader_path(&child_path));
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

fn collect_files_recursive(
    directory: &Path,
    root: &Path,
    paths: &mut Vec<String>,
) -> Result<(), VfsError> {
    for entry in std::fs::read_dir(directory)
        .map_err(|err| map_fs_not_found(err, directory.display().to_string()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files_recursive(&path, root, paths)?;
        } else if entry.file_type()?.is_file() {
            paths.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

pub fn hdp_asset_source_builder(root: impl Into<String>) -> AssetSourceBuilder {
    let root = root.into();
    AssetSourceBuilder::new(move || Box::new(HdpAssetReader::new(root.clone())))
}

pub fn file_asset_source_builder(root: impl Into<PathBuf>) -> AssetSourceBuilder {
    let root = root.into();
    AssetSourceBuilder::new(move || Box::new(FileAssetReader::new(root.clone())))
}

pub struct HdpAssetReader {
    vfs: HdpVfs,
}

impl HdpAssetReader {
    pub fn new(root: impl Into<String>) -> Self {
        let root = root.into();
        Self {
            vfs: HdpVfs::new(workspace_base_path().join(&root)),
        }
    }

    fn read_virtual_bytes(&self, path: &Path) -> Result<Vec<u8>, AssetReaderError> {
        self.vfs
            .read_bytes(&path.to_string_lossy())
            .map_err(vfs_to_reader_error)
    }
}

impl AssetReader for HdpAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<VecReader, AssetReaderError> {
        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            let bytes = self.read_virtual_bytes(path)?;
            return Ok(VecReader::new(bytes));
        }

        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<VecReader, AssetReaderError> {
        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }

        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            let items = self.vfs.list_virtual_directory(path)?;
            return Ok(Box::new(stream::iter(items)));
        }

        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            return self.vfs.is_virtual_directory(path);
        }

        Ok(false)
    }
}

pub fn split_hdp_asset_path(raw: &str) -> Option<(String, String)> {
    let raw = raw.strip_prefix("hdp://").unwrap_or(raw);
    let marker = ".hdp/";
    let (archive, entry) = if let Some(index) = raw.find(marker) {
        (&raw[..index + 4], &raw[index + marker.len()..])
    } else if raw.ends_with(".hdp") {
        (raw, "")
    } else {
        return None;
    };
    let archive = normalize_relative_path(Path::new(archive))
        .to_string_lossy()
        .replace('\\', "/");
    let entry = normalize_relative_path(Path::new(entry))
        .to_string_lossy()
        .replace('\\', "/");
    Some((archive, entry))
}

fn normalize_virtual_asset_path(path: &str) -> String {
    if let Some((archive, entry)) = split_hdp_asset_path(path) {
        if entry.is_empty() {
            format!("hdp://{archive}/")
        } else {
            format!("hdp://{archive}/{entry}")
        }
    } else if let Some((source, path)) = split_asset_source_uri(path) {
        let path = normalize_relative_path(Path::new(path))
            .to_string_lossy()
            .replace('\\', "/");
        format!("{source}://{path}")
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
            format!("hdp://{archive}/")
        } else {
            format!("hdp://{archive}/{entry}")
        }
    } else if let Some((source, entry)) = split_asset_source_uri(root) {
        let mut combined = PathBuf::from(entry);
        combined.push(requested_path);
        let path = normalize_relative_path(&combined)
            .to_string_lossy()
            .replace('\\', "/");
        format!("{source}://{path}")
    } else {
        let mut combined = PathBuf::from(root);
        combined.push(requested_path);
        normalize_virtual_asset_path(&combined.to_string_lossy())
    }
}

fn normalize_resource_root_path(path: &str) -> String {
    if let Some((archive, entry)) = split_hdp_asset_path(path) {
        if entry.is_empty() {
            format!("hdp://{archive}/")
        } else {
            format!("hdp://{archive}/{}", entry.trim_matches('/'))
        }
    } else if path.ends_with(".hdp") {
        normalize_virtual_asset_path(&format!("hdp://{path}/"))
    } else {
        normalize_virtual_asset_path(path)
    }
}

fn split_asset_source_uri(raw: &str) -> Option<(&str, &str)> {
    let (source, path) = raw.split_once("://")?;
    (!source.is_empty() && !path.is_empty()).then_some((source, path))
}

fn is_explicit_asset_uri(path: &str) -> bool {
    path.starts_with("hdp://")
        || path.starts_with("assets://")
        || path.starts_with("workspace://")
        || path.starts_with(ASSET_ROOT_PREFIX)
        || path.starts_with(WORKSPACE_ROOT_PREFIX)
        || path.starts_with(RESOURCE_ROOT_PREFIX)
}

fn normalize_reader_path(path: &str) -> String {
    normalize_relative_path(Path::new(path))
        .to_string_lossy()
        .replace('\\', "/")
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
        VfsError::Utf8(error) => {
            AssetReaderError::from(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        }
        VfsError::SettingsData { path, message } => AssetReaderError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to load settings data `{path}`: {message}"),
        )),
    }
}

fn reader_to_vfs_error(error: AssetReaderError) -> VfsError {
    match error {
        AssetReaderError::NotFound(path) => VfsError::NotFound(path.to_string_lossy().to_string()),
        AssetReaderError::Io(error) => VfsError::Io(std::io::Error::other(error.to_string())),
        other => VfsError::Io(std::io::Error::other(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    fn normalizes_bevy_asset_source_uris() {
        let vfs = HdpVfs::new("assets");

        assert_eq!(
            vfs.resolve_path(None, "hdp://main.hdp/path/../bg.png"),
            "hdp://main.hdp/bg.png"
        );
        assert_eq!(
            vfs.resolve_path(None, "assets:/images/bg.png"),
            "assets://images/bg.png"
        );
        assert_eq!(
            vfs.resolve_path(None, "workspace:/examples/demo.rhai"),
            "workspace://examples/demo.rhai"
        );
    }

    #[test]
    fn resolves_relative_content_names_from_settings_directories() {
        let root = std::env::temp_dir().join(format!("hiraku-vfs-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        std::fs::write(
            root.join("settings.rhai"),
            "#{ backgrounds_dir: \"art/backgrounds\", soundeffects_dir: \"audio/sfx\", fonts: #{ path: \"font-pack\" } }",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("font-pack")).unwrap();
        std::fs::write(root.join("font-pack/Regular.otf"), b"font").unwrap();
        std::fs::write(root.join("font-pack/readme.txt"), b"ignored").unwrap();

        let vfs = HdpVfs::new_with_config(&root, "settings.rhai", "startup.rhai");
        assert_eq!(
            vfs.resolve_background_path(Some("scripts/chapter.rhai"), "forest.png")
                .unwrap(),
            "art/backgrounds/forest.png"
        );
        assert_eq!(
            vfs.resolve_soundeffect_path(Some("scripts/chapter.rhai"), "click.wav")
                .unwrap(),
            "audio/sfx/click.wav"
        );
        assert_eq!(
            vfs.load_font_paths().unwrap(),
            vec!["font-pack/Regular.otf".to_string()]
        );

        let _ = std::fs::remove_file(root.join("settings.rhai"));
        let _ = std::fs::remove_file(root.join("font-pack/Regular.otf"));
        let _ = std::fs::remove_file(root.join("font-pack/readme.txt"));
        let _ = std::fs::remove_dir(root.join("font-pack"));
        let _ = std::fs::remove_dir(&root);
    }

    #[test]
    fn resolves_relative_content_names_inside_an_hdp() {
        let vfs = HdpVfs::new("assets");

        assert_eq!(
            vfs.resolve_background_path(
                Some("hdp://side-story.hdp/scripts/chapter.rhai"),
                "forest.png"
            )
            .unwrap(),
            "hdp://side-story.hdp/backgrounds/forest.png"
        );
    }

    #[test]
    fn reads_hdp_uri_through_the_archive_reader() {
        let root = std::env::temp_dir().join(format!("hiraku-hdp-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let archive = std::fs::File::create(root.join("main.hdp")).unwrap();
        let mut zip = ZipWriter::new(archive);
        zip.start_file("backgrounds/forest.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hdp-test").unwrap();
        zip.finish().unwrap();

        let vfs = HdpVfs::new(&root);
        assert_eq!(
            vfs.read_bytes("hdp://main.hdp/backgrounds/forest.png")
                .unwrap(),
            b"hdp-test"
        );

        let _ = std::fs::remove_file(root.join("main.hdp"));
        let _ = std::fs::remove_dir(&root);
    }
}
