use std::{collections::BTreeMap, sync::Arc};

use crate::vfs::HdpArchiveStore;
use bevy::{
    asset::{AssetLoader, AssetPath, LoadContext, LoadState, io::Reader},
    prelude::*,
    reflect::TypePath,
};
use thiserror::Error;

#[derive(Asset, TypePath, Debug, Clone)]
pub struct HdpArchive;

#[derive(TypePath)]
pub struct HdpArchiveLoader {
    store: HdpArchiveStore,
}

impl HdpArchiveLoader {
    pub fn new(store: HdpArchiveStore) -> Self {
        Self { store }
    }
}

#[derive(Debug, Error)]
pub enum HdpArchiveLoaderError {
    #[error("failed to read HDP archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid HDP archive: {0}")]
    Hdp(#[from] hiraku_hdp::HdpError),
    #[error("HDP archive bytes were already published")]
    AlreadyLoaded,
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
        let archive = Arc::new(hiraku_hdp::Archive::from_first_volume(first_volume)?);
        self.store
            .publish(archive, load_context.path().path().to_path_buf())
            .map_err(|_| HdpArchiveLoaderError::AlreadyLoaded)?;
        Ok(HdpArchive)
    }

    fn extensions(&self) -> &[&str] {
        &["hdp"]
    }
}

#[derive(Resource, Default)]
pub struct HdpVolumeLoads(BTreeMap<u32, Handle<BytesAsset>>);

pub fn stream_requested_hdp_volumes(
    asset_server: Res<AssetServer>,
    volume_assets: Res<Assets<BytesAsset>>,
    archive_store: Res<HdpArchiveStore>,
    mut loads: ResMut<HdpVolumeLoads>,
) {
    for (volume, path) in archive_store.requested_volumes() {
        loads
            .0
            .entry(volume)
            .or_insert_with(|| asset_server.load(AssetPath::from_path_buf(path)));
    }

    let ready = loads
        .0
        .iter()
        .filter_map(|(volume, handle)| {
            volume_assets
                .get(handle)
                .map(|asset| (*volume, asset.bytes.clone()))
        })
        .collect::<Vec<_>>();
    for (volume, bytes) in ready {
        match archive_store.provide_volume(volume, bytes) {
            Ok(()) => {
                loads.0.remove(&volume);
            }
            Err(error) => {
                error!("failed to load HDP volume {volume}: {error}");
                loads.0.remove(&volume);
            }
        }
    }

    let failed = loads
        .0
        .iter()
        .filter_map(|(volume, handle)| match asset_server.load_state(handle) {
            LoadState::Failed(error) => Some((*volume, error.to_string())),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (volume, error) in failed {
        archive_store.fail_volume(volume, error.clone());
        loads.0.remove(&volume);
        error!("failed to fetch HDP volume {volume}: {error}");
    }
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
    use std::path::PathBuf;

    use hiraku_hdp::{PackOptions, PackageBuilder};

    use super::*;

    #[test]
    fn first_volume_can_be_published_without_the_rest_of_the_package() {
        let mut builder = PackageBuilder::new();
        builder
            .add_file("startup.hks", b"narrate(\"ready\")")
            .expect("test file path must be valid");
        let package = builder
            .build(PackOptions::default())
            .expect("test package must build");

        let store = HdpArchiveStore::default();
        let archive = Arc::new(
            hiraku_hdp::Archive::from_first_volume(Arc::<[u8]>::from(package.volumes[0].clone()))
                .expect("first volume must open"),
        );
        store
            .publish(archive, PathBuf::from("main.hdp"))
            .expect("archive store must be empty");
        assert!(store.is_ready(), "archive must be published by ECS");
    }
}
