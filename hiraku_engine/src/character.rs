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
    pub expressions: BTreeMap<String, CharacterExpressionDefinition>,
    pub basis: Vec<String>,
    pub default_expression: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CharacterPartDefinition {
    pub id: String,
    pub path: String,
    pub offset: Vec2,
    pub layer: f32,
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct CharacterExpressionDefinition {
    pub slot: Option<String>,
    pub parts: Vec<String>,
    pub expressions: Vec<String>,
}

impl CharacterDefinition {
    pub fn parts_for_expressions(
        &self,
        expressions: &[String],
    ) -> Result<Vec<CharacterPartDefinition>, String> {
        let mut selected = BTreeMap::<String, Vec<String>>::new();
        let basis = if self.basis.is_empty() {
            self.default_expression.iter().cloned().collect::<Vec<_>>()
        } else {
            self.basis.clone()
        };
        if basis.is_empty() && expressions.is_empty() {
            return Ok(self.parts.clone());
        }

        for expression in basis.iter().chain(expressions) {
            self.apply_expression(expression, &mut selected, &mut Vec::new())?;
        }

        let selected_ids = selected
            .into_values()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>();
        Ok(self
            .parts
            .iter()
            .filter(|part| selected_ids.contains(&part.id))
            .cloned()
            .collect())
    }

    fn apply_expression(
        &self,
        name: &str,
        selected: &mut BTreeMap<String, Vec<String>>,
        resolving: &mut Vec<String>,
    ) -> Result<(), String> {
        if resolving.iter().any(|expression| expression == name) {
            return Err(format!(
                "character `{}` has a circular expression reference at `{name}`",
                self.name
            ));
        }
        let expression = self
            .expressions
            .get(name)
            .ok_or_else(|| format!("character `{}` has no expression named `{name}`", self.name))?;
        resolving.push(name.to_string());
        for nested in &expression.expressions {
            self.apply_expression(nested, selected, resolving)?;
        }
        resolving.pop();

        if !expression.parts.is_empty() {
            let key = expression
                .slot
                .clone()
                .unwrap_or_else(|| format!("expression:{name}"));
            selected.insert(key, expression.parts.clone());
        }
        Ok(())
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
    expressions: BTreeMap<String, CharacterExpressionFile>,
    #[serde(default)]
    basis: Vec<String>,
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
    #[serde(default)]
    rect: Option<[f64; 4]>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CharacterExpressionFile {
    Parts(Vec<String>),
    Definition {
        #[serde(default)]
        slot: Option<String>,
        #[serde(default)]
        parts: Vec<String>,
        #[serde(default)]
        expressions: Vec<String>,
    },
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
                rect: part.rect.map(|rect| {
                    let left = rect[0] as f32;
                    let top = rect[1] as f32;
                    [left, top, left + rect[2] as f32, top + rect[3] as f32]
                }),
            })
            .collect::<Vec<_>>();
        parts.sort_by(|left, right| {
            left.layer
                .partial_cmp(&right.layer)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        let expressions = config
            .expressions
            .into_iter()
            .map(|(name, expression)| {
                let expression = match expression {
                    CharacterExpressionFile::Parts(parts) => CharacterExpressionDefinition {
                        slot: None,
                        parts,
                        expressions: Vec::new(),
                    },
                    CharacterExpressionFile::Definition {
                        slot,
                        parts,
                        expressions,
                    } => CharacterExpressionDefinition {
                        slot,
                        parts,
                        expressions,
                    },
                };
                (name, expression)
            })
            .collect::<BTreeMap<_, _>>();
        validate_expressions(
            &expressions,
            &config.basis,
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
                expressions,
                basis: config.basis,
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
    expressions: &BTreeMap<String, CharacterExpressionDefinition>,
    basis: &[String],
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

    for expression in basis {
        if !expressions.contains_key(expression) {
            return Err(CharacterCatalogError::Data {
                path: path.to_string(),
                message: format!("basis references undefined expression `{expression}`"),
            });
        }
    }

    for (expression, definition) in expressions {
        for part_id in &definition.parts {
            if !parts.iter().any(|part| &part.id == part_id) {
                return Err(CharacterCatalogError::Data {
                    path: path.to_string(),
                    message: format!(
                        "expression `{expression}` references missing part `{part_id}`"
                    ),
                });
            }
        }
        for nested in &definition.expressions {
            if !expressions.contains_key(nested) {
                return Err(CharacterCatalogError::Data {
                    path: path.to_string(),
                    message: format!(
                        "expression `{expression}` references undefined expression `{nested}`"
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
        assert_eq!(
            alice
                .parts_for_expressions(&["happy".to_string()])
                .unwrap()
                .len(),
            2
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn slot_expressions_preserve_the_other_basis_slots() {
        let definition = CharacterDefinition {
            name: "alice".to_string(),
            directory: String::new(),
            config_path: String::new(),
            parts: [
                "body",
                "mouth_closed",
                "mouth_open",
                "face_neutral",
                "face_happy",
            ]
            .into_iter()
            .map(|id| CharacterPartDefinition {
                id: id.to_string(),
                path: format!("{id}.png"),
                offset: Vec2::ZERO,
                layer: 0.0,
                rect: None,
            })
            .collect(),
            expressions: BTreeMap::from([
                (
                    "basis".to_string(),
                    CharacterExpressionDefinition {
                        slot: None,
                        parts: vec!["body".to_string()],
                        expressions: vec!["mouth_closed".to_string(), "face_neutral".to_string()],
                    },
                ),
                (
                    "mouth_closed".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some("mouth".to_string()),
                        parts: vec!["mouth_closed".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "mouth_open".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some("mouth".to_string()),
                        parts: vec!["mouth_open".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "face_neutral".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some("face".to_string()),
                        parts: vec!["face_neutral".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "face_happy".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some("face".to_string()),
                        parts: vec!["face_happy".to_string()],
                        expressions: Vec::new(),
                    },
                ),
            ]),
            basis: vec!["basis".to_string()],
            default_expression: None,
        };

        let parts = definition
            .parts_for_expressions(&["mouth_open".to_string(), "face_happy".to_string()])
            .unwrap();
        let ids = parts.into_iter().map(|part| part.id).collect::<Vec<_>>();
        assert_eq!(ids, ["body", "mouth_open", "face_happy"]);
    }
}
