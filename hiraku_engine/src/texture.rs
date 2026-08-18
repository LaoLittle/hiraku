use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    data::evaluate_hks_map,
    vfs::{HdpVfs, VfsError},
};

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

/// GPU-ready atlas entries keyed by the catalog's existing path/rectangle form.
/// Keeping this separate from `TextureCatalog` lets script evaluation stay
/// synchronous while image decoding and atlas construction happen on Bevy's
/// asset/render schedule.
#[derive(Clone)]
pub struct AtlasTexture {
    pub image: Handle<Image>,
    pub atlas: TextureAtlas,
}

#[derive(Default, Resource)]
pub struct TextureAtlasCatalog {
    entries: Vec<(String, Option<[f32; 4]>, AtlasTexture)>,
    pub ready: bool,
}

impl TextureAtlasCatalog {
    pub fn resolve(&self, path: &str, rect: Option<[f32; 4]>) -> Option<&AtlasTexture> {
        self.entries
            .iter()
            .find_map(|(entry_path, entry_rect, texture)| {
                (entry_path == path && *entry_rect == rect).then_some(texture)
            })
    }
}

#[derive(Resource)]
pub struct TextureAtlasBuildState {
    catalog: TextureCatalog,
    images: BTreeMap<String, Handle<Image>>,
    built: bool,
}

pub fn prepare_texture_atlases(commands: &mut Commands, asset_server: &AssetServer, vfs: &HdpVfs) {
    let catalog = match load_texture_catalog(vfs) {
        Ok(catalog) => catalog,
        Err(error) => {
            warn!("failed to load texture catalog: {error}");
            TextureCatalog::default()
        }
    };
    let images = catalog
        .textures
        .values()
        .map(|texture| texture.path.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|path| {
            let image = asset_server.load(path.clone());
            (path, image)
        })
        .collect();
    commands.insert_resource(TextureAtlasCatalog::default());
    commands.insert_resource(catalog.clone());
    commands.insert_resource(TextureAtlasBuildState {
        catalog,
        images,
        built: false,
    });
}

pub fn build_texture_atlases(
    state: Option<ResMut<TextureAtlasBuildState>>,
    mut image_assets: ParamSet<(Res<Assets<Image>>, ResMut<Assets<Image>>)>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut atlas_catalog: ResMut<TextureAtlasCatalog>,
) {
    let Some(mut state) = state else {
        return;
    };
    let generated_atlas = {
        let source_images = image_assets.p0();
        if state.built
            || !state
                .images
                .values()
                .all(|handle| source_images.contains(handle))
        {
            return;
        }

        let authored_source_paths = state
            .catalog
            .textures
            .values()
            .filter(|definition| definition.rect.is_some())
            .map(|definition| definition.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut authored_paths = BTreeMap::<String, Vec<(&String, &TextureDefinition)>>::new();
        let mut scattered_paths = BTreeMap::<String, Vec<(&String, &TextureDefinition)>>::new();
        for (name, definition) in &state.catalog.textures {
            let target = if authored_source_paths.contains(definition.path.as_str()) {
                &mut authored_paths
            } else {
                &mut scattered_paths
            };
            target
                .entry(definition.path.clone())
                .or_default()
                .push((name, definition));
        }

        // An authored source sheet becomes a Bevy layout, including full-image
        // names that share the sheet with explicit regions.
        for (path, entries) in &authored_paths {
            let source = state.images[path].clone();
            let Some(image) = source_images.get(&source) else {
                continue;
            };
            let mut layout = TextureAtlasLayout::new_empty(image.size());
            for (_, definition) in entries {
                let rect = definition.rect.unwrap_or([
                    0.0,
                    0.0,
                    image.width() as f32,
                    image.height() as f32,
                ]);
                let min = UVec2::new(
                    rect[0].round().max(0.0) as u32,
                    rect[1].round().max(0.0) as u32,
                );
                let max = UVec2::new(
                    (rect[0] + rect[2]).round().clamp(0.0, image.width() as f32) as u32,
                    (rect[1] + rect[3])
                        .round()
                        .clamp(0.0, image.height() as f32) as u32,
                );
                if max.x <= min.x || max.y <= min.y {
                    warn!("ignoring empty texture region in `{path}`");
                    continue;
                }
                let index = layout.add_texture(URect::from_corners(min, max));
                atlas_catalog.entries.push((
                    definition.path.clone(),
                    definition.rect,
                    AtlasTexture {
                        image: source.clone(),
                        atlas: TextureAtlas::default().with_index(index),
                    },
                ));
            }
            let layout = layouts.add(layout);
            for (_, _, texture) in atlas_catalog
                .entries
                .iter_mut()
                .filter(|(entry_path, _, _)| entry_path == path)
            {
                if texture.atlas.layout == Handle::default() {
                    texture.atlas.layout = layout.clone();
                }
            }
        }

        // A single image has no binding reduction to gain. Leave it as its source
        // asset instead of expanding it into a power-of-two atlas allocation.
        if scattered_paths.len() > 1 {
            let mut builder = TextureAtlasBuilder::default();
            builder.padding(UVec2::splat(1));
            builder.max_size(UVec2::splat(8192));
            let paths = scattered_paths.keys().cloned().collect::<Vec<_>>();
            for path in &paths {
                let image = source_images.get(&state.images[path]).unwrap();
                builder.add_texture(Some(state.images[path].id()), image);
            }
            match builder.build() {
                Ok((layout, _, image)) => Some((paths, layout, image)),
                Err(error) => {
                    warn!("failed to build texture atlas: {error}");
                    None
                }
            }
        } else {
            None
        }
    };

    if let Some((paths, layout, image)) = generated_atlas {
        let layout = layouts.add(layout);
        let image = image_assets.p1().add(image);
        for (index, path) in paths.iter().enumerate() {
            for definition in state
                .catalog
                .textures
                .values()
                .filter(|definition| definition.path == *path && definition.rect.is_none())
            {
                atlas_catalog.entries.push((
                    definition.path.clone(),
                    None,
                    AtlasTexture {
                        image: image.clone(),
                        atlas: TextureAtlas::from(layout.clone()).with_index(index),
                    },
                ));
            }
        }
    }

    state.built = true;
    atlas_catalog.ready = true;
    info!(
        "texture atlas catalog ready: {} atlas-backed textures",
        atlas_catalog.entries.len()
    );
}

pub fn texture_atlases_ready(catalog: Res<TextureAtlasCatalog>) -> bool {
    catalog.ready
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
    descriptor_paths.retain(|path| path.ends_with(".texture.data.hks"));
    descriptor_paths.sort();

    let mut textures = BTreeMap::new();
    for descriptor_path in descriptor_paths {
        let source = vfs.read_text(&descriptor_path)?;
        let data = evaluate_hks_map(&descriptor_path, &source).map_err(|error| {
            TextureCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
            }
        })?;
        let texture: TextureFile = serde_json::from_value(serde_json::Value::Object(data))
            .map_err(|error| TextureCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
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
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };

    fn test_image(color: [u8; 4]) -> Image {
        Image::new_fill(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &color,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
    }

    #[test]
    fn packs_multiple_standalone_textures() {
        let mut app = App::new();
        app.insert_resource(Assets::<Image>::default());
        app.insert_resource(Assets::<TextureAtlasLayout>::default());
        app.insert_resource(TextureAtlasCatalog::default());

        let (first, second) = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            (
                images.add(test_image([255, 0, 0, 255])),
                images.add(test_image([0, 255, 0, 255])),
            )
        };
        let catalog = TextureCatalog {
            textures: BTreeMap::from([
                (
                    "first".to_string(),
                    TextureDefinition {
                        path: "first.png".to_string(),
                        rect: None,
                    },
                ),
                (
                    "second".to_string(),
                    TextureDefinition {
                        path: "second.png".to_string(),
                        rect: None,
                    },
                ),
            ]),
        };
        app.insert_resource(TextureAtlasBuildState {
            catalog,
            images: BTreeMap::from([
                ("first.png".to_string(), first),
                ("second.png".to_string(), second),
            ]),
            built: false,
        });
        app.add_systems(Update, build_texture_atlases);
        app.update();

        let catalog = app.world().resource::<TextureAtlasCatalog>();
        let first = catalog.resolve("first.png", None).unwrap();
        let second = catalog.resolve("second.png", None).unwrap();
        assert!(catalog.ready);
        assert_eq!(first.image, second.image);
        assert_ne!(first.atlas.index, second.atlas.index);
    }

    #[test]
    fn ir_background_commands_resolve_catalog_names() {
        let catalog = TextureCatalog {
            textures: BTreeMap::from([(
                "bg/016/001".to_string(),
                TextureDefinition {
                    path: "hdp://main.hdp/textures/backgrounds/Background_016_001.png".to_string(),
                    rect: None,
                },
            )]),
        };

        let command = crate::script::script_command_from_ir(
            crate::script::IrCommand::SetBackground {
                texture: "bg/016/001".to_string(),
            },
            Some(&catalog),
        )
        .unwrap();
        assert!(matches!(
            command,
            crate::script::ScriptCommand::SetBackground { path, .. }
                if path == "hdp://main.hdp/textures/backgrounds/Background_016_001.png"
        ));
    }
}
