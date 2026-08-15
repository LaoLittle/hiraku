use std::collections::BTreeMap;

use bevy::prelude::Resource;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    data::evaluate_rhai_map,
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
    descriptor_paths.retain(|path| path.ends_with(".texture.rhai"));
    descriptor_paths.sort();

    let mut textures = BTreeMap::new();
    for descriptor_path in descriptor_paths {
        let source = vfs.read_text(&descriptor_path)?;
        let data = evaluate_rhai_map(&descriptor_path, &source).map_err(|error| {
            TextureCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
            }
        })?;
        let value = rhai::Dynamic::from_map(data);
        let texture: TextureFile =
            rhai::serde::from_dynamic(&value).map_err(|error| TextureCatalogError::Data {
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
