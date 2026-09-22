//! Loose character parts are CPU-only assets. Prefetch and scene ownership
//! share this typed source; only the generated atlas is uploaded to the GPU.
use bevy::ecs::{message::MessageCursor, system::SystemParam};
use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    image::{CompressedImageFormats, ImageLoader, ImageLoaderError, ImageLoaderSettings},
    prelude::*,
};

#[derive(SystemParam)]
pub(super) struct AtlasSources<'w, 's> {
    pub assets: Option<Res<'w, Assets<AtlasSource>>>,
    events: Option<Res<'w, Messages<AssetEvent<AtlasSource>>>>,
    cursor: Local<'s, MessageCursor<AssetEvent<AtlasSource>>>,
}

impl AtlasSources<'_, '_> {
    pub fn changed(&mut self) -> Vec<AssetId<AtlasSource>> {
        let Some(events) = &self.events else {
            return Vec::new();
        };
        self.cursor
            .read(events)
            .filter_map(|event| match event {
                AssetEvent::Added { id }
                | AssetEvent::Modified { id }
                | AssetEvent::Removed { id } => Some(*id),
                _ => None,
            })
            .collect()
    }
}

#[derive(Asset, TypePath)]
pub(crate) struct AtlasSource(pub Image);

#[derive(Default, TypePath)]
pub(super) struct AtlasSourceLoader;

#[derive(Debug, thiserror::Error)]
pub(super) enum AtlasSourceError {
    #[error(transparent)]
    Image(#[from] ImageLoaderError),
    #[error(transparent)]
    Uastc(#[from] hiraku_uastc::UastcError),
    #[error("failed to read character atlas source: {0}")]
    Io(#[from] std::io::Error),
}

impl AssetLoader for AtlasSourceLoader {
    type Asset = AtlasSource;
    type Settings = ImageLoaderSettings;
    type Error = AtlasSourceError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut settings = settings.clone();
        settings.asset_usage = RenderAssetUsages::MAIN_WORLD;
        if context
            .path()
            .path()
            .to_string_lossy()
            .ends_with(".uastc.ktx2")
        {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            return Ok(AtlasSource(hiraku_uastc::decode_with_settings(
                &bytes,
                CompressedImageFormats::NONE,
                &settings,
            )?));
        }
        let image = ImageLoader::new(CompressedImageFormats::NONE)
            .load(reader, &settings, context)
            .await?;
        Ok(AtlasSource(image))
    }

    // Select by requested asset type only; never replace the normal PNG loader.
    fn extensions(&self) -> &[&str] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::io::{
        AssetSourceBuilder, AssetSourceId,
        memory::{Dir, MemoryAssetReader},
    };

    #[test]
    fn loose_source_loads_once_without_creating_a_render_image() {
        let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 80, 120, 160]));
        let mut png = std::io::Cursor::new(Vec::new());
        pixels
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode synthetic image");
        check_cpu_source("alice.png", png.into_inner());
    }

    #[test]
    fn uastc_source_transcodes_to_cpu_rgba_without_a_render_image() {
        // One synthetic 4x4 solid-color UASTC block in an uncompressed KTX2.
        // No encoder, native library or game asset is required by this test.
        let mut bytes = vec![0u8; 176];
        bytes[..12].copy_from_slice(b"\xabKTX 20\xbb\r\n\x1a\n");
        for (offset, value) in [
            (16, 1u32),
            (20, 4),
            (24, 4),
            (36, 1),
            (40, 1),
            (48, 104),
            (52, 44),
            (104, 44),
            (112, 2 | (40 << 16)),
            (116, 166 | (1 << 8) | (2 << 16)),
            (120, 3 | (3 << 8)),
            (124, 16),
            (132, (127 << 16) | (3 << 24)),
            (144, u32::MAX),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [(80, 160u64), (88, 16), (96, 16)] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let solid = 0x17u64 | (40 << 5) | (80 << 13) | (120 << 21) | (160 << 29);
        bytes[160..168].copy_from_slice(&solid.to_le_bytes());
        check_cpu_source("alice.uastc.ktx2", bytes);
    }

    fn check_cpu_source(path: &'static str, bytes: Vec<u8>) {
        let dir = Dir::default();
        dir.insert_asset(std::path::Path::new(path), bytes);
        let mut app = App::new();
        app.register_asset_source(
            AssetSourceId::Default,
            AssetSourceBuilder::new(move || Box::new(MemoryAssetReader { root: dir.clone() })),
        );
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .register_asset_loader(ImageLoader::new(CompressedImageFormats::NONE))
            .init_asset::<AtlasSource>()
            .init_asset_loader::<AtlasSourceLoader>();
        let server = app.world().resource::<AssetServer>();
        let prefetch = server.load::<AtlasSource>(path);
        let cpu = server.load::<AtlasSource>(path);
        assert_eq!(
            prefetch.id(),
            cpu.id(),
            "prefetch and scene share the CPU asset"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if app.world().resource::<Assets<AtlasSource>>().contains(&cpu) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "CPU image loader did not complete: {:?}",
                app.world().resource::<AssetServer>().load_state(cpu.id())
            );
            std::thread::yield_now();
        }
        assert!(
            app.world().resource::<Assets<Image>>().is_empty(),
            "loose sources must never upload a render image"
        );
        let sources = app.world().resource::<Assets<AtlasSource>>();
        let source = &sources.get(&cpu).expect("CPU source").0;
        assert_eq!(source.asset_usage, RenderAssetUsages::MAIN_WORLD);
        assert_eq!(
            &source.data.as_ref().expect("retained CPU pixels")[..4],
            &[40, 80, 120, 160]
        );
    }
}
