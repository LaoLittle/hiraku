use std::{
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use bevy::{
    asset::io::{AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, VecReader},
    prelude::*,
};
use event_listener::Event;
use futures_lite::stream;
use hiraku_hdp::{Archive, HdpError};
use thiserror::Error;

use crate::data::evaluate_hson_map;

mod settings;
use settings::{SettingsFile, ensure_empty_data_map, settings_from_data, take_data_string};

pub const DEFAULT_ASSET_ROOT: &str = "assets";
pub const DEFAULT_SETTINGS_PATH: &str = "hdp://main.hdp/settings.hson";
pub const DEFAULT_STARTUP_SCRIPT: &str = "hdp://main.hdp/startup.hks";
pub const RESOURCE_ROOT_PREFIX: &str = "res:/";
pub const DEFAULT_RESOURCE_ROOT: &str = "hdp://main.hdp/";
pub const HDP_SOURCE_ID: &str = "hdp";
pub const DEFAULT_BACKGROUNDS_DIR: &str = "backgrounds";
pub const DEFAULT_BGM_DIR: &str = "bgm";
pub const DEFAULT_SOUNDEFFECTS_DIR: &str = "soundeffects";
pub const DEFAULT_VOICE_DIR: &str = "voice";
pub const DEFAULT_MOVIES_DIR: &str = "movies";
pub const DEFAULT_CHARACTERS_DIR: &str = "characters";
pub const DEFAULT_FONTS_DIR: &str = "fonts";
pub const DEFAULT_TEXTURES_DIR: &str = "textures";

pub fn workspace_base_path() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Resource, Clone)]
pub struct VfsResource(pub Arc<HdpVfs>);

/// Parsed archive published by Bevy's asynchronous `HdpArchiveLoader`.
#[derive(Debug)]
struct HdpArchiveState {
    archive: Arc<Archive>,
    archive_path: PathBuf,
    requested: Vec<AtomicBool>,
    available: Vec<Event>,
    failures: Vec<OnceLock<String>>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct HdpArchiveStore(Arc<OnceLock<Arc<HdpArchiveState>>>);

impl HdpArchiveStore {
    pub fn publish(
        &self,
        archive: Arc<Archive>,
        archive_path: PathBuf,
    ) -> Result<(), Arc<Archive>> {
        let volume_count = archive.index().volume_count as usize;
        self.0
            .set(Arc::new(HdpArchiveState {
                archive,
                archive_path,
                requested: (0..volume_count).map(|_| AtomicBool::new(false)).collect(),
                available: (0..volume_count).map(|_| Event::new()).collect(),
                failures: (0..volume_count).map(|_| OnceLock::new()).collect(),
            }))
            .map_err(|state| state.archive.clone())
    }

    fn archive(&self) -> Option<Arc<Archive>> {
        self.0.get().map(|state| state.archive.clone())
    }

    pub fn is_ready(&self) -> bool {
        self.archive().is_some()
    }

    pub(crate) fn requested_volumes(&self) -> Vec<(u32, PathBuf)> {
        let Some(state) = self.0.get() else {
            return Vec::new();
        };
        state
            .requested
            .iter()
            .enumerate()
            .filter(|(volume, requested)| {
                *volume != 0
                    && requested.load(Ordering::Acquire)
                    && !state.archive.is_volume_available(*volume as u32)
            })
            .map(|(volume, _)| {
                let mut path = state.archive_path.as_os_str().to_os_string();
                path.push(format!(".{volume:03}"));
                (volume as u32, PathBuf::from(path))
            })
            .collect()
    }

    pub(crate) fn provide_volume(&self, volume: u32, bytes: Arc<[u8]>) -> Result<(), HdpError> {
        let state = self.0.get().ok_or(HdpError::MissingVolume(volume))?;
        state.archive.provide_volume(volume, bytes)?;
        if let Some(event) = state.available.get(volume as usize) {
            event.notify(usize::MAX);
        }
        Ok(())
    }

    pub(crate) fn fail_volume(&self, volume: u32, message: String) {
        let Some(state) = self.0.get() else {
            return;
        };
        if let Some(failure) = state.failures.get(volume as usize) {
            let _ = failure.set(message);
        }
        if let Some(event) = state.available.get(volume as usize) {
            event.notify(usize::MAX);
        }
    }

    async fn wait_for_volume(&self, volume: u32) -> Result<(), VfsError> {
        let state = self
            .0
            .get()
            .ok_or_else(|| VfsError::NotFound("HDP archive".into()))?;
        let requested = state
            .requested
            .get(volume as usize)
            .ok_or(HdpError::MissingVolume(volume))?;
        let available = state
            .available
            .get(volume as usize)
            .ok_or(HdpError::MissingVolume(volume))?;
        loop {
            if state.archive.is_volume_available(volume) {
                return Ok(());
            }
            if let Some(message) = state.failures[volume as usize].get() {
                return Err(VfsError::Hdp(HdpError::InvalidFormat(format!(
                    "failed to load volume {volume}: {message}"
                ))));
            }
            let listener = available.listen();
            requested.store(true, Ordering::Release);
            if state.archive.is_volume_available(volume) {
                return Ok(());
            }
            if let Some(message) = state.failures[volume as usize].get() {
                return Err(VfsError::Hdp(HdpError::InvalidFormat(format!(
                    "failed to load volume {volume}: {message}"
                ))));
            }
            listener.await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct HdpVfs {
    root: PathBuf,
    settings_path: String,
    default_startup_script: String,
    archive_store: HdpArchiveStore,
}

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to read HDP archive: {0}")]
    Hdp(#[from] HdpError),
    #[error("file is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("failed to load settings data `{path}`: {message}")]
    SettingsData { path: String, message: String },
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
            archive_store: HdpArchiveStore::default(),
        }
    }

    pub fn new_with_config_and_store(
        root: impl Into<PathBuf>,
        settings_path: impl Into<String>,
        default_startup_script: impl Into<String>,
        archive_store: HdpArchiveStore,
    ) -> Self {
        Self {
            root: root.into(),
            settings_path: settings_path.into(),
            default_startup_script: default_startup_script.into(),
            archive_store,
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
            .list_files_recursive(&directory)?
            .into_iter()
            .filter(|path| path.ends_with(".font.hson"))
            .map(|descriptor_path| {
                let source = self.read_text(&descriptor_path)?;
                let mut data = evaluate_hson_map(&descriptor_path, &source)
                    .map_err(|error| invalid_settings_data(&descriptor_path, error.to_string()))?;
                let font =
                    take_data_string(&mut data, "font", &descriptor_path)?.ok_or_else(|| {
                        invalid_settings_data(
                            &descriptor_path,
                            "font descriptor requires `font`".to_string(),
                        )
                    })?;
                ensure_empty_data_map(data, &descriptor_path, "font descriptor")?;
                Ok(self.resolve_path(Some(&descriptor_path), &font))
            })
            .collect::<Result<Vec<_>, VfsError>>()?;
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

    pub fn load_movies_dir_path(&self, base: Option<&str>) -> Result<String, VfsError> {
        self.load_directory_path(
            base,
            |settings| settings.movies_dir.clone(),
            DEFAULT_MOVIES_DIR,
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
        let data = evaluate_hson_map(&settings_path, &settings_text).map_err(|error| {
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
            .unwrap_or("settings.hson")
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
        if requested.starts_with("hdp://") {
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
        if let Some((archive_name, entry)) = split_hdp_asset_path(path) {
            if let Some(archive) = self.archive_store.archive() {
                return archive
                    .read_file(&entry)
                    .map_err(|error| map_hdp_not_found(error, format!("{archive_name}!{entry}")));
            }
            return Err(VfsError::NotFound(archive_name));
        }

        let full_path = self.root.join(path);
        std::fs::read(&full_path)
            .map_err(|err| map_fs_not_found(err, full_path.display().to_string()))
    }

    async fn read_bytes_async(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let Some((archive_name, entry)) = split_hdp_asset_path(path) else {
            return self.read_bytes(path);
        };
        loop {
            let Some(archive) = self.archive_store.archive() else {
                return Err(VfsError::NotFound(archive_name));
            };
            match archive.read_file(&entry) {
                Ok(bytes) => return Ok(bytes),
                Err(HdpError::MissingVolume(volume)) => {
                    self.archive_store.wait_for_volume(volume).await?;
                }
                Err(error) => {
                    return Err(map_hdp_not_found(error, format!("{archive_name}!{entry}")));
                }
            }
        }
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

        let directory = PathBuf::from(path);
        let full_path = self.root.join(&directory);
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
            let path = self
                .root
                .join(&directory)
                .join(name)
                .strip_prefix(&self.root)
                .unwrap_or(child.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            paths.push(path);
        }
        Ok(paths)
    }

    pub fn list_files_recursive(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if let Some((archive_name, entry)) = split_hdp_asset_path(path) {
            if let Some(archive) = self.archive_store.archive() {
                return Ok(list_archive_files(&archive, &archive_name, &entry));
            }
            return Err(VfsError::NotFound(archive_name));
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

        let items = if let Some(package) = self.archive_store.archive() {
            list_archive_directory(&package, &archive, &entry)
        } else {
            return Err(AssetReaderError::NotFound(PathBuf::from(archive)));
        };
        if items.is_empty() {
            Err(AssetReaderError::NotFound(path.to_path_buf()))
        } else {
            Ok(items)
        }
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

fn list_archive_files(package: &Archive, archive: &str, entry: &str) -> Vec<String> {
    let prefix = normalize_entry_prefix(entry);
    package
        .files()
        .filter(|name| name.starts_with(&prefix))
        .map(|name| format!("hdp://{archive}/{name}"))
        .collect()
}

fn list_archive_directory(package: &Archive, archive: &str, entry: &str) -> Vec<PathBuf> {
    let directory_prefix = normalize_entry_prefix(entry);
    let mut items = Vec::new();

    for name in package.files() {
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

    items
}

pub fn hdp_asset_source_builder(
    root: impl Into<String>,
    archive_store: HdpArchiveStore,
) -> AssetSourceBuilder {
    let root = root.into();
    AssetSourceBuilder::new(move || {
        Box::new(HdpAssetReader::new(root.clone(), archive_store.clone()))
    })
}

pub struct HdpAssetReader {
    vfs: HdpVfs,
}

impl HdpAssetReader {
    pub fn new(root: impl Into<String>, archive_store: HdpArchiveStore) -> Self {
        let root = root.into();
        Self {
            vfs: HdpVfs::new_with_config_and_store(
                PathBuf::from(root),
                DEFAULT_SETTINGS_PATH,
                DEFAULT_STARTUP_SCRIPT,
                archive_store,
            ),
        }
    }

    async fn read_virtual_bytes_async(&self, path: &Path) -> Result<Vec<u8>, AssetReaderError> {
        self.vfs
            .read_bytes_async(&path.to_string_lossy())
            .await
            .map_err(vfs_to_reader_error)
    }
}

impl AssetReader for HdpAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<VecReader, AssetReaderError> {
        if split_hdp_asset_path(&path.to_string_lossy()).is_some() {
            let bytes = self.read_virtual_bytes_async(path).await?;
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

fn is_explicit_asset_uri(path: &str) -> bool {
    path.starts_with("hdp://") || path.starts_with(RESOURCE_ROOT_PREFIX)
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

fn map_hdp_not_found(error: HdpError, path: String) -> VfsError {
    match error {
        HdpError::MissingFile(_) => VfsError::NotFound(path),
        other => VfsError::Hdp(other),
    }
}

fn vfs_to_reader_error(error: VfsError) -> AssetReaderError {
    match error {
        VfsError::NotFound(path) => AssetReaderError::NotFound(PathBuf::from(path)),
        VfsError::Io(error) => AssetReaderError::from(error),
        VfsError::Hdp(error) => {
            AssetReaderError::from(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        }
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
    use hiraku_hdp::{
        CompressionMethod, CompressionOptions, FileOptions, PackOptions, PackageBuilder,
    };

    #[test]
    fn normalizes_hdp_asset_source_uris() {
        let vfs = HdpVfs::new("assets");

        assert_eq!(
            vfs.resolve_path(None, "hdp://main.hdp/path/../bg.png"),
            "hdp://main.hdp/bg.png"
        );
    }

    #[test]
    fn resolves_relative_content_names_from_settings_directories() {
        let root = std::env::temp_dir().join(format!("hiraku-vfs-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        std::fs::write(
            root.join("settings.hson"),
            ".{ backgroundsDir: \"art/backgrounds\", soundeffectsDir: \"audio/sfx\", fonts: .{ path: \"font-pack\" } }",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("font-pack")).unwrap();
        std::fs::write(root.join("font-pack/Regular.otf"), b"font").unwrap();
        std::fs::write(
            root.join("font-pack/regular.font.hson"),
            ".{ font: \"Regular.otf\" }",
        )
        .unwrap();
        std::fs::write(root.join("font-pack/readme.txt"), b"ignored").unwrap();

        let vfs = HdpVfs::new_with_config(&root, "settings.hson", "startup.hks");
        assert_eq!(
            vfs.resolve_background_path(Some("scripts/chapter.story.hks"), "forest.png")
                .unwrap(),
            "art/backgrounds/forest.png"
        );
        assert_eq!(
            vfs.resolve_soundeffect_path(Some("scripts/chapter.story.hks"), "click.wav")
                .unwrap(),
            "audio/sfx/click.wav"
        );
        assert_eq!(
            vfs.load_font_paths().unwrap(),
            vec!["font-pack/Regular.otf".to_string()]
        );

        let _ = std::fs::remove_file(root.join("settings.hson"));
        let _ = std::fs::remove_file(root.join("font-pack/Regular.otf"));
        let _ = std::fs::remove_file(root.join("font-pack/regular.font.hson"));
        let _ = std::fs::remove_file(root.join("font-pack/readme.txt"));
        let _ = std::fs::remove_dir(root.join("font-pack"));
        let _ = std::fs::remove_dir(&root);
    }

    #[test]
    fn resolves_relative_content_names_inside_an_hdp() {
        let vfs = HdpVfs::new("assets");

        assert_eq!(
            vfs.resolve_background_path(
                Some("hdp://side-story.hdp/scripts/chapter.story.hks"),
                "forest.png"
            )
            .unwrap(),
            "hdp://side-story.hdp/backgrounds/forest.png"
        );
    }

    #[test]
    fn reads_hdp_uri_through_the_archive_reader() {
        let mut builder = PackageBuilder::new();
        builder
            .add_file("backgrounds/forest.png", b"hdp-test")
            .unwrap();
        let package = builder.build(PackOptions::default()).unwrap();
        let archive = Archive::from_bytes(Arc::<[u8]>::from(package.volumes[0].clone())).unwrap();

        let store = HdpArchiveStore::default();
        store
            .publish(Arc::new(archive), PathBuf::from("main.hdp"))
            .expect("test archive must only be published once");
        let vfs = HdpVfs::new_with_config_and_store(
            "assets",
            DEFAULT_SETTINGS_PATH,
            DEFAULT_STARTUP_SCRIPT,
            store,
        );
        assert_eq!(
            vfs.read_bytes("hdp://main.hdp/backgrounds/forest.png")
                .unwrap(),
            b"hdp-test"
        );
    }

    #[test]
    fn async_reader_requests_and_resumes_streamed_volumes() {
        let mut builder = PackageBuilder::new();
        builder
            .add_file_with_options(
                "startup.hks",
                vec![b's'; 128],
                FileOptions {
                    bootstrap: true,
                    compression: Some(CompressionOptions {
                        method: CompressionMethod::STORED,
                        ..Default::default()
                    }),
                },
            )
            .unwrap();
        builder
            .add_file_with_options(
                "voice/test.ogg",
                vec![b'v'; 1500],
                FileOptions {
                    compression: Some(CompressionOptions {
                        method: CompressionMethod::STORED,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let package = builder
            .build(PackOptions {
                chunk_size: 300,
                max_volume_size: Some(800),
                ..Default::default()
            })
            .unwrap();
        let archive = Archive::from_first_volume(Arc::<[u8]>::from(package.volumes[0].clone()))
            .expect("first volume must open");
        let store = HdpArchiveStore::default();
        store
            .publish(Arc::new(archive), PathBuf::from("main.hdp"))
            .expect("archive store must be empty");
        let vfs = HdpVfs::new_with_config_and_store(
            "assets",
            DEFAULT_SETTINGS_PATH,
            DEFAULT_STARTUP_SCRIPT,
            store.clone(),
        );

        let read = vfs.read_bytes_async("hdp://main.hdp/voice/test.ogg");
        let provide = async {
            for (volume, bytes) in package.volumes.iter().enumerate().skip(1) {
                while !store
                    .requested_volumes()
                    .iter()
                    .any(|(requested, _)| *requested == volume as u32)
                {
                    futures_lite::future::yield_now().await;
                }
                store
                    .provide_volume(volume as u32, Arc::<[u8]>::from(bytes.clone()))
                    .expect("requested volume must validate");
            }
        };
        let (bytes, ()) = futures_lite::future::block_on(futures_lite::future::zip(read, provide));
        assert_eq!(bytes.unwrap(), vec![b'v'; 1500]);
    }
}
