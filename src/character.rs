use std::collections::BTreeMap;

use bevy::{math::Vec2, prelude::Resource};
use serde::Deserialize;
use thiserror::Error;

use crate::vfs::{HdpVfs, VfsError};

#[derive(Clone, Debug, Default, Resource)]
pub struct CharacterCatalog {
    pub directory: Option<String>,
    pub characters: BTreeMap<String, CharacterDefinition>,
}

#[derive(Clone, Debug)]
pub struct CharacterDefinition {
    pub name: String,
    pub directory: String,
    pub config_path: String,
    pub parts: Vec<CharacterPartDefinition>,
}

#[derive(Clone, Debug)]
pub struct CharacterPartDefinition {
    pub id: String,
    pub path: String,
    pub offset: Vec2,
    pub layer: f32,
}

#[derive(Debug, Error)]
pub enum CharacterCatalogError {
    #[error("failed to read character catalog setting: {0}")]
    Settings(#[from] VfsError),
    #[error("failed to parse character catalog `{path}`: {source}")]
    CatalogToml {
        path: String,
        source: toml::de::Error,
    },
    #[error("failed to parse character config `{path}`: {source}")]
    CharacterToml {
        path: String,
        source: toml::de::Error,
    },
}

#[derive(Debug, Deserialize, Default)]
struct CharacterCatalogFile {
    #[serde(default, alias = "characters")]
    character: Vec<CharacterCatalogEntryFile>,
}

#[derive(Debug, Deserialize)]
struct CharacterCatalogEntryFile {
    name: String,
    #[serde(alias = "directory", alias = "dir")]
    dir: String,
    #[serde(default, alias = "config_path", alias = "path")]
    config: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CharacterConfigFile {
    #[serde(default)]
    parts: BTreeMap<String, CharacterPartFile>,
}

#[derive(Debug, Deserialize)]
struct CharacterPartFile {
    path: String,
    #[serde(default)]
    offset: Option<[f32; 2]>,
    #[serde(default)]
    layer: Option<f32>,
}

pub fn load_character_catalog(vfs: &HdpVfs) -> Result<CharacterCatalog, CharacterCatalogError> {
    let Some(directory) = vfs.load_characters_dir_path()? else {
        return Ok(CharacterCatalog::default());
    };

    let catalog_path = vfs.resolve_path(Some(vfs.settings_path()), &format!("{directory}/characters.toml"));
    let catalog_text = match vfs.read_text(&catalog_path) {
        Ok(catalog_text) => catalog_text,
        Err(VfsError::NotFound(_)) => {
            return Ok(CharacterCatalog {
                directory: Some(directory),
                characters: BTreeMap::new(),
            });
        }
        Err(err) => return Err(CharacterCatalogError::Settings(err)),
    };

    let file = toml::from_str::<CharacterCatalogFile>(&catalog_text).map_err(|source| {
        CharacterCatalogError::CatalogToml {
            path: catalog_path.clone(),
            source,
        }
    })?;

    let mut characters = BTreeMap::new();
    for entry in file.character {
        let character_directory = vfs.resolve_path(Some(&catalog_path), &entry.dir);
        let config_relative = entry.config.unwrap_or_else(|| "character.toml".to_string());
        let config_path = vfs.resolve_path(
            Some(&format!("{character_directory}/__dir__")),
            &config_relative,
        );
        let config_text = vfs.read_text(&config_path).map_err(CharacterCatalogError::Settings)?;
        let config = toml::from_str::<CharacterConfigFile>(&config_text).map_err(|source| {
            CharacterCatalogError::CharacterToml {
                path: config_path.clone(),
                source,
            }
        })?;

        let mut parts = config
            .parts
            .into_iter()
            .map(|(id, part)| CharacterPartDefinition {
                id,
                path: vfs.resolve_path(Some(&config_path), &part.path),
                offset: part
                    .offset
                    .map(|offset| Vec2::new(offset[0], offset[1]))
                    .unwrap_or(Vec2::ZERO),
                layer: part.layer.unwrap_or(0.0),
            })
            .collect::<Vec<_>>();
        parts.sort_by(|left, right| {
            left.layer
                .partial_cmp(&right.layer)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });

        characters.insert(
            entry.name.clone(),
            CharacterDefinition {
                name: entry.name,
                directory: character_directory,
                config_path,
                parts,
            },
        );
    }

    Ok(CharacterCatalog {
        directory: Some(directory),
        characters,
    })
}
