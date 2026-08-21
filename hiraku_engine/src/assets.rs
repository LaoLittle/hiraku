use std::{ffi::OsString, path::PathBuf, sync::Arc};

use crate::vfs::HdpArchiveStore;
use bevy::{
    asset::{AssetLoader, AssetPath, LoadContext, io::Reader},
    prelude::*,
    reflect::TypePath,
};
use thiserror::Error;

#[derive(Asset, TypePath, Debug, Clone)]
pub struct HdpArchive {
    first_volume: Arc<[u8]>,
    remaining_volumes: Vec<Handle<BytesAsset>>,
    assembly_finished: bool,
}

#[derive(Default, TypePath)]
pub struct HdpArchiveLoader;

#[derive(Debug, Error)]
pub enum HdpArchiveLoaderError {
    #[error("failed to read HDP archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid HDP archive: {0}")]
    Hdp(#[from] hiraku_hdp::HdpError),
}

impl AssetLoader for HdpArchiveLoader {
    type Asset = HdpArchive;
    type Settings = ();
    type Error = HdpArchiveLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let first_volume = Arc::<[u8]>::from(bytes);
        let index = hiraku_hdp::Archive::read_index(&first_volume)?;
        let mut remaining_volumes =
            Vec::with_capacity(index.volume_count.saturating_sub(1) as usize);
        for volume in 1..index.volume_count {
            let path = numbered_volume_path(load_context.path().path(), volume);
            let source = load_context.path().source().clone_owned();
            let asset_path = AssetPath::from_path_buf(path).with_source(source);
            remaining_volumes.push(load_context.load::<BytesAsset>(asset_path));
        }
        Ok(HdpArchive {
            first_volume,
            remaining_volumes,
            assembly_finished: false,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["hdp"]
    }
}

pub fn assemble_hdp_archives(
    mut archives: ResMut<Assets<HdpArchive>>,
    volume_assets: Res<Assets<BytesAsset>>,
    archive_store: Res<HdpArchiveStore>,
) {
    if archive_store.is_ready() {
        return;
    }

    for (_, pending) in archives.iter_mut() {
        if pending.assembly_finished {
            continue;
        }
        let Some(remaining) = pending
            .remaining_volumes
            .iter()
            .map(|handle| volume_assets.get(handle))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };

        let volumes = std::iter::once(pending.first_volume.clone())
            .chain(remaining.into_iter().map(|volume| volume.bytes.clone()));
        pending.assembly_finished = true;
        match hiraku_hdp::Archive::from_volumes(volumes) {
            Ok(archive) => {
                if archive_store.publish(Arc::new(archive)).is_err() {
                    warn!("an HDP archive was already published");
                }
            }
            Err(error) => error!("failed to assemble HDP archive volumes: {error}"),
        }
    }
}

fn numbered_volume_path(path: &std::path::Path, volume: u32) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(format!(".{volume:03}"));
    PathBuf::from(name)
}

#[derive(Asset, TypePath, Debug, Clone)]
pub struct BytesAsset {
    pub bytes: Arc<[u8]>,
}

#[derive(Default, TypePath)]
pub struct BytesAssetLoader;

#[derive(Debug, Error)]
pub enum BytesAssetLoaderError {
    #[error("failed to read file bytes: {0}")]
    Io(#[from] std::io::Error),
}

impl AssetLoader for BytesAssetLoader {
    type Asset = BytesAsset;
    type Settings = ();
    type Error = BytesAssetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(BytesAsset {
            bytes: Arc::<[u8]>::from(bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use hiraku_hdp::{PackOptions, PackageBuilder};

    use super::*;

    #[test]
    fn ecs_assembles_and_publishes_an_archive_without_external_coordination() {
        let mut builder = PackageBuilder::new();
        builder
            .add_file("startup.story.hks", b"narrate(\"ready\")")
            .expect("test file path must be valid");
        let package = builder
            .build(PackOptions::default())
            .expect("test package must build");

        let store = HdpArchiveStore::default();
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .insert_resource(store.clone())
            .init_asset::<HdpArchive>()
            .init_asset::<BytesAsset>()
            .add_systems(Update, assemble_hdp_archives);
        let _archive_handle =
            app.world_mut()
                .resource_mut::<Assets<HdpArchive>>()
                .add(HdpArchive {
                    first_volume: Arc::<[u8]>::from(package.volumes[0].clone()),
                    remaining_volumes: Vec::new(),
                    assembly_finished: false,
                });

        app.update();

        assert!(store.is_ready(), "archive must be published by ECS");
    }

    #[test]
    fn numbered_volumes_are_siblings_of_the_first_volume() {
        assert_eq!(
            numbered_volume_path(std::path::Path::new("nested/main.hdp"), 12),
            PathBuf::from("nested/main.hdp.012")
        );
    }
}
