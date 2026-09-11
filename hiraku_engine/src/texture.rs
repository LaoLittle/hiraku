use std::collections::BTreeMap;

use bevy::prelude::*;
use hiraku_script::hson;
use serde::Deserialize;
use thiserror::Error;

use crate::vfs::{HdpVfs, VfsError};

/// Immutable artwork keeps its metadata, but transfers pixel ownership to the
/// renderer. Use the same settings for speculative and demand loads.
pub(crate) fn load_static_image(server: &AssetServer, path: impl Into<String>) -> Handle<Image> {
    server
        .load_builder()
        .with_settings(|settings: &mut bevy::image::ImageLoaderSettings| {
            settings.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
        })
        .load(path.into())
}

#[derive(Clone, Debug, Default, Resource)]
pub struct TextureCatalog {
    textures: BTreeMap<String, TextureDefinition>,
}

#[derive(Clone, Debug)]
pub struct TextureDefinition {
    pub path: String,
    /// `[left, top, width, height]` source pixels, when this is a subtexture.
    pub rect: Option<[f32; 4]>,
}

impl TextureCatalog {
    pub fn resolve(&self, name: &str) -> Option<&TextureDefinition> {
        self.textures.get(name)
    }
}

/// Build only the descriptor catalog. Image bytes are requested by consumers
/// when a picture, character part or UI image is actually instantiated.
pub fn prepare_texture_catalog(commands: &mut Commands, vfs: &HdpVfs) {
    match load_texture_catalog(vfs) {
        Ok(catalog) => {
            commands.insert_resource(catalog);
        }
        Err(error) => {
            crate::script::emit_script_diagnostic(
                "failed to load texture catalog",
                &error.to_string(),
            );
            commands.insert_resource(TextureCatalog::default());
        }
    }
}

#[derive(Debug, Error)]
pub enum TextureCatalogError {
    #[error("failed to read texture data: {0}")]
    Read(#[from] VfsError),
    #[error("failed to load texture data `{path}`: {message}")]
    Data { path: String, message: String },
}

#[derive(Debug, Deserialize)]
struct TextureFile {
    #[serde(default)]
    name: Option<String>,
    image: String,
    #[serde(default)]
    regions: BTreeMap<String, TextureRegionFile>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TextureRegionFile {
    Rect([f64; 4]),
    Definition { rect: [f64; 4] },
}

pub fn load_texture_catalog(vfs: &HdpVfs) -> Result<TextureCatalog, TextureCatalogError> {
    let directory = vfs.load_textures_dir_path()?;
    let mut descriptor_paths = match vfs.list_files_recursive(&directory) {
        Ok(paths) => paths,
        Err(VfsError::NotFound(_)) => return Ok(TextureCatalog::default()),
        Err(error) => return Err(error.into()),
    };
    descriptor_paths.retain(|path| path.ends_with(".texture.hson"));
    descriptor_paths.sort();

    let mut textures = BTreeMap::new();
    for descriptor_path in descriptor_paths {
        let source = vfs.read_text(&descriptor_path)?;
        let texture: TextureFile =
            hson::from_str(&source).map_err(|error| TextureCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.render_with_options(
                    &descriptor_path,
                    &source,
                    hiraku_script::RenderOptions::terminal(),
                ),
            })?;
        let path = vfs.resolve_path(Some(&descriptor_path), &texture.image);

        if let Some(name) = texture.name {
            insert_texture(
                &mut textures,
                name,
                TextureDefinition {
                    path: path.clone(),
                    rect: None,
                },
                &descriptor_path,
            )?;
        }
        for (name, region) in texture.regions {
            let rect = match region {
                TextureRegionFile::Rect(rect) => rect,
                TextureRegionFile::Definition { rect } => rect,
            };
            insert_texture(
                &mut textures,
                name,
                TextureDefinition {
                    path: path.clone(),
                    rect: Some(rect.map(|value| value as f32)),
                },
                &descriptor_path,
            )?;
        }
    }
    Ok(TextureCatalog { textures })
}

fn insert_texture(
    textures: &mut BTreeMap<String, TextureDefinition>,
    name: String,
    definition: TextureDefinition,
    descriptor_path: &str,
) -> Result<(), TextureCatalogError> {
    if textures.insert(name.clone(), definition).is_some() {
        return Err(TextureCatalogError::Data {
            path: descriptor_path.to_string(),
            message: format!("texture `{name}` is defined more than once"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_artwork_extraction_preserves_layout_metadata_without_cpu_pixels() {
        use bevy::{
            asset::RenderAssetUsages,
            render::{render_asset::RenderAsset, texture::GpuImage},
        };
        let mut image = Image::from_dynamic(
            image::DynamicImage::new_rgba8(16, 8),
            true,
            RenderAssetUsages::RENDER_WORLD,
        );
        let upload = GpuImage::take_gpu_data(&mut image, None).expect("extract static artwork");
        assert_eq!(
            upload.data.as_ref().expect("upload pixels").len(),
            16 * 8 * 4
        );
        assert!(image.data.is_none());
        assert_eq!(image.size(), UVec2::new(16, 8));
        assert_eq!(image.texture_descriptor, upload.texture_descriptor);
    }

    #[test]
    fn startup_reads_descriptors_without_an_asset_server_or_image_payload() {
        use hiraku_hdp::{Archive, PackOptions, PackageBuilder};
        use std::{path::PathBuf, sync::Arc};
        let mut package = PackageBuilder::new();
        package
            .add_file("settings.hson", br#".{ texturesDir: "textures" }"#)
            .expect("settings fixture");
        package
            .add_file(
                "textures/alice.texture.hson",
                br#".{
            image: "alice.png", regions: .{ "alice/face": (8, 16, 32, 48) }
        }"#,
            )
            .expect("descriptor fixture");
        // Intentionally omit alice.png. Merely indexing it must not request,
        // decode, or require the image to be present.
        let bytes = package
            .build(PackOptions::default())
            .expect("fixture package");
        let archive = Archive::from_bytes(Arc::<[u8]>::from(bytes.volumes[0].clone()))
            .expect("fixture archive");
        let store = crate::vfs::HdpArchiveStore::default();
        store
            .publish(Arc::new(archive), PathBuf::from("fixture.hdp"))
            .expect("publish fixture once");
        let vfs = HdpVfs::new_with_config_and_store(
            "unused",
            "hdp://fixture.hdp/settings.hson",
            "startup.hks",
            store,
        );
        let mut app = App::new();
        app.add_systems(Update, move |mut commands: Commands| {
            prepare_texture_catalog(&mut commands, &vfs)
        });
        app.update();
        assert!(!app.world().contains_resource::<AssetServer>());
        assert!(!app.world().contains_resource::<Assets<Image>>());
        let entry = app
            .world()
            .resource::<TextureCatalog>()
            .resolve("alice/face")
            .expect("descriptor registered");
        assert_eq!(entry.rect, Some([8.0, 16.0, 32.0, 48.0]));
        assert!(entry.path.ends_with("textures/alice.png"));
    }

    #[test]
    fn slider_skin_resolves_catalog_regions_without_image_loading() {
        let textures = TextureCatalog {
            textures: ["track", "fill", "thumb"]
                .into_iter()
                .map(|name| {
                    (
                        name.to_string(),
                        TextureDefinition {
                            path: "memory://controls.png".into(),
                            rect: Some([0.0, 0.0, 32.0, 16.0]),
                        },
                    )
                })
                .collect(),
        };
        let screen = crate::script::evaluate_ui_component_named_with_args(
            "memory://controls.ui.hks",
            "import ui.widgets.*\nscreen { slider(0.5, 0.0, 1.0).skin(\"track\", \"fill\", \"thumb\").onChange { value: Float -> () } }",
            crate::script::UiContext::default(), &textures, &crate::glossary::TermCatalog::default(), &[],
        ).expect("skinned slider builds");
        let crate::ui::ScreenNode::Input(input) = &screen.children[0] else {
            panic!("expected slider")
        };
        let skin = input.slider_skin.as_ref().expect("skin retained");
        assert!(
            skin.iter()
                .all(|texture| texture.rect == Some([0.0, 0.0, 32.0, 16.0]))
        );
        assert!(input.on_change.is_some());
    }

    #[test]
    fn story_background_effects_resolve_catalog_names() {
        let catalog = TextureCatalog {
            textures: BTreeMap::from([(
                "bg/016/001".to_string(),
                TextureDefinition {
                    path: "hdp://main.hdp/textures/backgrounds/Background_016_001.png".to_string(),
                    rect: None,
                },
            )]),
        };

        let command = crate::script::script_command_from_effect(
            crate::script::capabilities::StoryEffect::SetBackground {
                texture: "bg/016/001".to_string(),
                fade_in_ms: None,
            },
            Some(&catalog),
        )
        .unwrap();
        assert!(matches!(
            command,
            crate::script::ScriptCommand::Stage(crate::script::StageCommand::SetBackground {
                path,
                ..
            })
                if path == "hdp://main.hdp/textures/backgrounds/Background_016_001.png"
        ));
    }
}
