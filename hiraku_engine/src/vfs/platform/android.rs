use std::{
    ffi::CString,
    io,
    os::fd::{BorrowedFd, OwnedFd},
    os::unix::fs::FileExt,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use bevy::{
    asset::{
        AssetApp,
        io::{
            AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
            ReaderNotSeekableError, SeekableReader, VecReader,
        },
    },
    prelude::App,
};
use event_listener::Event;
use futures_lite::stream;
use hiraku_hdp::Archive;

struct SelectedPackage {
    archive: Archive,
    main: std::fs::File,
}

#[derive(Default)]
struct PackageSelection {
    requested: AtomicBool,
    result: OnceLock<Result<SelectedPackage, String>>,
    changed: Event,
}

fn selection() -> &'static PackageSelection {
    static SELECTION: OnceLock<PackageSelection> = OnceLock::new();
    SELECTION.get_or_init(PackageSelection::default)
}

fn request_selection() {
    let state = selection();
    if state.requested.swap(true, Ordering::AcqRel) {
        return;
    }
    let result = (|| -> io::Result<()> {
        let application = app()?;
        let vm = unsafe { jni::JavaVM::from_raw(application.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let activity = unsafe {
                jni::objects::JObject::from_raw(env, application.activity_as_ptr().cast())
            };
            env.call_method(
                &activity,
                jni::jni_str!("selectHdpPackage"),
                jni::jni_sig!("()V"),
                &[],
            )?;
            Ok(())
        })
        .map_err(io::Error::other)
    })();
    if let Err(error) = result {
        let _ = state
            .result
            .set(Err(format!("failed to open Android HDP picker: {error}")));
        state.changed.notify(usize::MAX);
    }
}

async fn wait_for_selection() -> io::Result<&'static SelectedPackage> {
    request_selection();
    loop {
        let listener = selection().changed.listen();
        if let Some(result) = selection().result.get() {
            return result
                .as_ref()
                .map_err(|message| io::Error::other(message.clone()));
        }
        listener.await;
    }
}

pub(super) fn selected_archive(path: &Path) -> Option<Archive> {
    if path != Path::new("main.hdp") {
        return None;
    }
    selection()
        .result
        .get()?
        .as_ref()
        .ok()
        .map(|package| package.archive.clone())
}

fn asset_path(path: &Path) -> io::Result<CString> {
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "asset paths must be relative and cannot traverse parent directories",
        ));
    }
    CString::new(
        path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "asset path is not UTF-8")
        })?,
    )
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "asset path contains NUL"))
}

fn app() -> io::Result<&'static bevy::android::android_activity::AndroidApp> {
    bevy::android::ANDROID_APP
        .get()
        .ok_or_else(|| io::Error::other("Android application context is unavailable"))
}

fn apk_files(path: &Path, recursive: bool) -> io::Result<Vec<PathBuf>> {
    use jni::{
        JavaVM,
        objects::{IntoAuto, JObject, JObjectArray, JString, JValue},
    };
    let name = asset_path(path)?;
    let application = app()?;
    if !recursive {
        let manager = application.asset_manager();
        return manager.open_dir(&name).map_or(Ok(Vec::new()), |directory| {
            directory
                .map(|name| {
                    name.to_str().map(|name| path.join(name)).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "APK asset name is not UTF-8")
                    })
                })
                .collect()
        });
    }
    // Android owns the VM and the Activity global reference for the app lifetime.
    let vm = unsafe { JavaVM::from_raw(application.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<Vec<PathBuf>> {
        let activity = unsafe { JObject::from_raw(env, application.activity_as_ptr().cast()) };
        let manager = env
            .call_method(
                &activity,
                jni::jni_str!("getAssets"),
                jni::jni_sig!("()Landroid/content/res/AssetManager;"),
                &[],
            )?
            .l()?;
        let manager = manager.auto();
        let mut pending = vec![path.to_path_buf()];
        let mut files = Vec::new();
        let native_manager = application.asset_manager();
        while let Some(directory) = pending.pop() {
            let name = env.new_string(directory.to_string_lossy())?;
            let name = name.auto();
            let children = env
                .call_method(
                    &manager,
                    jni::jni_str!("list"),
                    jni::jni_sig!("(Ljava/lang/String;)[Ljava/lang/String;"),
                    &[JValue::Object(name.as_ref())],
                )?
                .l()?;
            let children = env.cast_local::<JObjectArray<JString>>(children)?;
            let children = children.auto();
            for index in 0..children.len(env)? {
                let name = children.get_element(env, index)?;
                let name = name.auto();
                let name = name.try_to_string(env)?;
                let child = directory.join(name);
                let child_name = asset_path(&child)
                    .map_err(|_| jni::errors::Error::NullPtr("invalid APK asset path"))?;
                if native_manager.open(&child_name).is_some() {
                    files.push(child);
                } else if recursive {
                    pending.push(child);
                }
            }
        }
        Ok(files)
    })
    .map_err(io::Error::other)
}

pub(super) fn read(_root: &Path, path: &Path) -> io::Result<Vec<u8>> {
    let name = asset_path(path)?;
    let manager = app()?.asset_manager();
    manager
        .open(&name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))?
        .buffer()
        .map(<[u8]>::to_vec)
}

pub(super) fn list_files(_root: &Path, path: &Path, recursive: bool) -> io::Result<Vec<PathBuf>> {
    apk_files(path, recursive)
}

struct AndroidAssetReader;

struct DocumentReader {
    file: std::fs::File,
    offset: u64,
}

impl futures_lite::io::AsyncRead for DocumentReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.file.read_at(bytes, self.offset) {
                Ok(count) => {
                    self.offset += count as u64;
                    return Poll::Ready(Ok(count));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl Reader for DocumentReader {
    fn seekable(&mut self) -> Result<&mut dyn SeekableReader, ReaderNotSeekableError> {
        Err(ReaderNotSeekableError)
    }
}

impl AssetReader for AndroidAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader + 'a>, AssetReaderError> {
        match read(Path::new(""), path) {
            Ok(bytes) => return Ok(Box::new(VecReader::new(bytes))),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && path == Path::new("main.hdp") => {}
            Err(error) => return Err(reader_error(error, path)),
        }
        let package = wait_for_selection().await?;
        Ok(Box::new(DocumentReader {
            file: package.main.try_clone()?,
            offset: 0,
        }))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<VecReader, AssetReaderError> {
        let mut name = path.as_os_str().to_os_string();
        name.push(".meta");
        let meta = PathBuf::from(name);
        read(Path::new(""), &meta)
            .map(VecReader::new)
            .map_err(|error| reader_error(error, &meta))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        let files =
            list_files(Path::new(""), path, false).map_err(|error| reader_error(error, path))?;
        Ok(Box::new(stream::iter(files.into_iter().filter(|path| {
            path.extension().is_none_or(|extension| extension != "meta")
        }))))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        let manager = app()?.asset_manager();
        let name = asset_path(path)?;
        if manager.open(&name).is_some() {
            return Ok(false);
        }
        Ok(!list_files(Path::new(""), path, false)?.is_empty())
    }
}

fn reader_error(error: io::Error, path: &Path) -> AssetReaderError {
    if error.kind() == io::ErrorKind::NotFound {
        AssetReaderError::NotFound(path.to_path_buf())
    } else {
        error.into()
    }
}

pub(super) fn register_asset_source(app: &mut App, _root: &str) {
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(|| Box::new(AndroidAssetReader)),
    );
}

fn duplicate_descriptor(fd: i32) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid document descriptor",
        ));
    }
    // The picker owns this descriptor until the synchronous native callback returns.
    unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_hiraku_app_HdpPicker_volumeCount(
    mut env: jni::EnvUnowned<'_>,
    _class: jni::objects::JClass<'_>,
    fd: jni::sys::jint,
) -> jni::sys::jint {
    env.with_env(|_| -> jni::errors::Result<i32> {
        let result = duplicate_descriptor(fd)
            .map(std::fs::File::from)
            .map_err(hiraku_hdp::HdpError::Io)
            .and_then(|mut file| Archive::read_volume_count(&mut file));
        match result {
            Ok(count) => Ok(i32::try_from(count).unwrap_or(-1)),
            Err(error) => {
                bevy::log::error!("invalid selected HDP: {error}");
                Ok(-1)
            }
        }
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_hiraku_app_HdpPicker_install<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: jni::objects::JClass<'local>,
    descriptors: jni::objects::JIntArray<'local>,
) -> jni::objects::JString<'local> {
    env.with_env(|env| -> jni::errors::Result<jni::objects::JString<'_>> {
        let mut raw = vec![0; descriptors.len(env)?];
        descriptors.get_region(env, 0, &mut raw)?;
        let result = (|| -> Result<SelectedPackage, hiraku_hdp::HdpError> {
            let files = raw
                .into_iter()
                .map(duplicate_descriptor)
                .map(|result| result.map(std::fs::File::from))
                .collect::<io::Result<Vec<_>>>()?;
            let main = files
                .first()
                .ok_or(hiraku_hdp::HdpError::MissingVolume(0))?
                .try_clone()?;
            let archive = Archive::open_volume_files(files)?;
            Ok(SelectedPackage { archive, main })
        })();
        match result {
            Ok(package) => {
                if selection().result.set(Ok(package)).is_err() {
                    return env.new_string("An HDP package is already selected");
                }
                selection().changed.notify(usize::MAX);
                Ok(jni::objects::JString::null())
            }
            Err(error) => env.new_string(error.to_string()),
        }
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_hiraku_app_HdpPicker_cancel(
    mut env: jni::EnvUnowned<'_>,
    _class: jni::objects::JClass<'_>,
) {
    env.with_env(|_| -> jni::errors::Result<()> {
        let _ = selection()
            .result
            .set(Err("HDP selection was cancelled".into()));
        selection().changed.notify(usize::MAX);
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
