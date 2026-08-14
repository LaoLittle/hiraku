use std::collections::BTreeMap;

use bevy::{math::Vec2, prelude::Resource};
use rhai::Dynamic;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    data::evaluate_rhai_map,
    vfs::{HdpVfs, VfsError},
};

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
    pub expressions: BTreeMap<String, Vec<String>>,
    pub default_expression: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CharacterPartDefinition {
    pub id: String,
    pub path: String,
    pub offset: Vec2,
    pub layer: f32,
}

impl CharacterDefinition {
    pub fn parts_for_expression(
        &self,
        expression: Option<&str>,
    ) -> Result<Vec<CharacterPartDefinition>, String> {
        let expression = expression.or(self.default_expression.as_deref());
        let Some(expression) = expression else {
            return Ok(self.parts.clone());
        };
        let part_ids = self.expressions.get(expression).ok_or_else(|| {
            format!(
                "character `{}` has no expression named `{expression}`",
                self.name
            )
        })?;

        Ok(self
            .parts
            .iter()
            .filter(|part| part_ids.contains(&part.id))
            .cloned()
            .collect())
    }
}

#[derive(Debug, Error)]
pub enum CharacterCatalogError {
    #[error("failed to read character data: {0}")]
    Read(#[from] VfsError),
    #[error("failed to load character data `{path}`: {message}")]
    Data { path: String, message: String },
}

#[derive(Debug, Deserialize, Default)]
struct CharacterCatalogFile {
    #[serde(default)]
    characters: Vec<CharacterCatalogEntryFile>,
}

#[derive(Debug, Deserialize)]
struct CharacterCatalogEntryFile {
    name: String,
    dir: String,
    #[serde(default)]
    config: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CharacterConfigFile {
    #[serde(default)]
    parts: BTreeMap<String, CharacterPartFile>,
    #[serde(default)]
    expressions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    default_expression: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CharacterPartFile {
    path: String,
    #[serde(default)]
    offset: Option<[f64; 2]>,
    #[serde(default)]
    layer: Option<f64>,
}

pub fn load_character_catalog(vfs: &HdpVfs) -> Result<CharacterCatalog, CharacterCatalogError> {
    let Some(directory) = vfs.load_characters_dir_path()? else {
        return Ok(CharacterCatalog::default());
    };

    let catalog_path = vfs.resolve_path(
        Some(vfs.settings_path()),
        &format!("{directory}/characters.rhai"),
    );
    let catalog_text = match vfs.read_text(&catalog_path) {
        Ok(catalog_text) => catalog_text,
        Err(VfsError::NotFound(_)) => {
            return Ok(CharacterCatalog {
                directory: Some(directory),
                characters: BTreeMap::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    let file: CharacterCatalogFile = parse_rhai_data(&catalog_path, &catalog_text)?;

    let mut characters = BTreeMap::new();
    for entry in file.characters {
        let character_directory = vfs.resolve_path(Some(&catalog_path), &entry.dir);
        let config_relative = entry.config.unwrap_or_else(|| "character.rhai".to_string());
        let config_path = vfs.resolve_path(
            Some(&format!("{character_directory}/__dir__")),
            &config_relative,
        );
        let config_text = vfs.read_text(&config_path)?;
        let config: CharacterConfigFile = parse_rhai_data(&config_path, &config_text)?;

        let mut parts = config
            .parts
            .into_iter()
            .map(|(id, part)| CharacterPartDefinition {
                id,
                path: vfs.resolve_path(Some(&config_path), &part.path),
                offset: part
                    .offset
                    .map(|offset| Vec2::new(offset[0] as f32, offset[1] as f32))
                    .unwrap_or(Vec2::ZERO),
                layer: part.layer.unwrap_or(0.0) as f32,
            })
            .collect::<Vec<_>>();
        parts.sort_by(|left, right| {
            left.layer
                .partial_cmp(&right.layer)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        validate_expressions(
            &config.expressions,
            config.default_expression.as_deref(),
            &parts,
            &config_path,
        )?;

        characters.insert(
            entry.name.clone(),
            CharacterDefinition {
                name: entry.name,
                directory: character_directory,
                config_path,
                parts,
                expressions: config.expressions,
                default_expression: config.default_expression,
            },
        );
    }

    Ok(CharacterCatalog {
        directory: Some(directory),
        characters,
    })
}

fn validate_expressions(
    expressions: &BTreeMap<String, Vec<String>>,
    default_expression: Option<&str>,
    parts: &[CharacterPartDefinition],
    path: &str,
) -> Result<(), CharacterCatalogError> {
    if let Some(default_expression) = default_expression
        && !expressions.contains_key(default_expression)
    {
        return Err(CharacterCatalogError::Data {
            path: path.to_string(),
            message: format!("default_expression `{default_expression}` is not defined"),
        });
    }

    for (expression, part_ids) in expressions {
        for part_id in part_ids {
            if !parts.iter().any(|part| &part.id == part_id) {
                return Err(CharacterCatalogError::Data {
                    path: path.to_string(),
                    message: format!(
                        "expression `{expression}` references missing part `{part_id}`"
                    ),
                });
            }
        }
    }

    Ok(())
}

fn parse_rhai_data<T>(path: &str, source: &str) -> Result<T, CharacterCatalogError>
where
    T: DeserializeOwned,
{
    let data = evaluate_rhai_map(path, source).map_err(|error| CharacterCatalogError::Data {
        path: path.to_string(),
        message: error.to_string(),
    })?;
    rhai::serde::from_dynamic(&Dynamic::from_map(data)).map_err(|error| {
        CharacterCatalogError::Data {
            path: path.to_string(),
            message: error.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_rhai_character_catalog_and_parts() {
        let root =
            std::env::temp_dir().join(format!("hiraku-character-test-{}", std::process::id()));
        let characters = root.join("characters/alice");
        std::fs::create_dir_all(&characters).unwrap();
        std::fs::write(
            root.join("settings.rhai"),
            "#{ characters_dir: \"characters\" }",
        )
        .unwrap();
        std::fs::write(
            root.join("characters/characters.rhai"),
            "#{ characters: [#{ name: \"alice\", dir: \"alice\" }] }",
        )
        .unwrap();
        std::fs::write(
            characters.join("character.rhai"),
            "#{ parts: #{ body: #{ path: \"body.png\", offset: [12.5, -3.0], layer: -1.0 }, face: #{ path: \"face.png\", layer: 2.0 } }, expressions: #{ happy: [\"body\", \"face\"] }, default_expression: \"happy\" }",
        )
        .unwrap();

        let vfs = HdpVfs::new_with_config(&root, "settings.rhai", "startup.rhai");
        let catalog = load_character_catalog(&vfs).unwrap();
        let alice = &catalog.characters["alice"];

        assert_eq!(alice.parts.len(), 2);
        assert_eq!(alice.parts[0].id, "body");
        assert_eq!(alice.parts[0].offset, Vec2::new(12.5, -3.0));
        assert_eq!(alice.parts[1].id, "face");
        assert_eq!(alice.parts_for_expression(Some("happy")).unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(root);
    }
}
