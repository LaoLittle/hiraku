use std::{collections::BTreeMap, path::Path};

use bevy::{math::Vec2, prelude::Resource};
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    data::evaluate_hks_map,
    texture::{TextureCatalog, TextureCatalogError, load_texture_catalog},
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
    /// Named slots are assigned in declaration order so authored state changes
    /// have stable internal identities instead of relying on part names.
    pub slots: BTreeMap<String, usize>,
    pub parts: Vec<CharacterPartDefinition>,
    pub expressions: BTreeMap<String, CharacterExpressionDefinition>,
    pub basis: Vec<String>,
    pub default_expression: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CharacterPartDefinition {
    pub id: String,
    pub slot: Option<usize>,
    pub path: String,
    /// Catalog rectangle in `[left, top, width, height]` form, retained so
    /// rendering can select the generated/declared atlas section.
    pub atlas_rect: Option<[f32; 4]>,
    pub offset: Vec2,
    pub layer: f32,
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct CharacterExpressionDefinition {
    pub slot: Option<usize>,
    pub parts: Vec<String>,
    pub expressions: Vec<String>,
}

impl CharacterDefinition {
    pub fn parts_for_expressions(
        &self,
        expressions: &[String],
    ) -> Result<Vec<CharacterPartDefinition>, String> {
        let mut selected = BTreeMap::<SelectionKey, Vec<String>>::new();
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
            .into_iter()
            .flat_map(|(slot, ids)| ids.into_iter().map(move |id| (slot.clone(), id)))
            .collect::<Vec<_>>();
        Ok(self
            .parts
            .iter()
            .filter_map(|part| {
                let slot = selected_ids
                    .iter()
                    .find_map(|(slot, id)| (id == &part.id).then_some(slot))?;
                let mut part = part.clone();
                if let SelectionKey::Slot(index) = slot {
                    part.slot = Some(*index);
                }
                Some(part)
            })
            .collect())
    }

    fn apply_expression(
        &self,
        name: &str,
        selected: &mut BTreeMap<SelectionKey, Vec<String>>,
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
                .map(SelectionKey::Slot)
                .unwrap_or_else(|| SelectionKey::Expression(name.to_string()));
            selected.insert(key, expression.parts.clone());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SelectionKey {
    Slot(usize),
    Expression(String),
}

#[derive(Debug, Error)]
pub enum CharacterCatalogError {
    #[error("failed to read character data: {0}")]
    Read(#[from] VfsError),
    #[error("failed to load texture data: {0}")]
    Texture(#[from] TextureCatalogError),
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
    slots: Vec<String>,
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
struct CharacterDataFile {
    name: String,
    #[serde(flatten)]
    config: CharacterConfigFile,
}

#[derive(Debug, Deserialize)]
struct CharacterPartFile {
    #[serde(default)]
    slot: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    texture: Option<String>,
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

    let textures = load_texture_catalog(vfs)?;
    let catalog_path = vfs.resolve_path(
        Some(vfs.settings_path()),
        &format!("{directory}/characters.data.hks"),
    );
    let catalog_text = match vfs.read_text(&catalog_path) {
        Ok(catalog_text) => catalog_text,
        Err(VfsError::NotFound(_)) => {
            return load_character_data_files(vfs, &textures, directory);
        }
        Err(error) => return Err(error.into()),
    };
    let file: CharacterCatalogFile = parse_hks_data(&catalog_path, &catalog_text)?;

    let mut characters = BTreeMap::new();
    for entry in file.characters {
        let character_directory = vfs.resolve_path(Some(&catalog_path), &entry.dir);
        let config_relative = entry
            .config
            .unwrap_or_else(|| "character.data.hks".to_string());
        let config_path = vfs.resolve_path(
            Some(&format!("{character_directory}/__dir__")),
            &config_relative,
        );
        let config_text = vfs.read_text(&config_path)?;
        let config: CharacterConfigFile = parse_hks_data(&config_path, &config_text)?;

        let definition = character_definition_from_config(
            vfs,
            &textures,
            entry.name.clone(),
            character_directory,
            config_path,
            config,
        )?;
        characters.insert(entry.name, definition);
    }

    Ok(CharacterCatalog {
        directory: Some(directory),
        characters,
    })
}

fn load_character_data_files(
    vfs: &HdpVfs,
    textures: &TextureCatalog,
    directory: String,
) -> Result<CharacterCatalog, CharacterCatalogError> {
    let mut paths = match vfs.list_files_recursive(&directory) {
        Ok(paths) => paths,
        Err(VfsError::NotFound(_)) => {
            return Ok(CharacterCatalog {
                directory: Some(directory),
                characters: BTreeMap::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    paths.retain(|path| path.ends_with(".char.data.hks"));
    paths.sort();

    let mut characters = BTreeMap::new();
    for path in paths {
        let source = vfs.read_text(&path)?;
        let data: CharacterDataFile = parse_hks_data(&path, &source)?;
        let directory_path = Path::new(&path)
            .parent()
            .and_then(|path| path.to_str())
            .unwrap_or_default();
        let definition = character_definition_from_config(
            vfs,
            textures,
            data.name.clone(),
            directory_path.to_string(),
            path,
            data.config,
        )?;
        if characters.insert(data.name.clone(), definition).is_some() {
            return Err(CharacterCatalogError::Data {
                path: data.name,
                message: "character is defined more than once".to_string(),
            });
        }
    }

    Ok(CharacterCatalog {
        directory: Some(directory),
        characters,
    })
}

fn character_definition_from_config(
    vfs: &HdpVfs,
    textures: &TextureCatalog,
    name: String,
    directory: String,
    config_path: String,
    config: CharacterConfigFile,
) -> Result<CharacterDefinition, CharacterCatalogError> {
    let slots = build_slot_indices(&config, &config_path)?;
    let mut parts = config
        .parts
        .into_iter()
        .map(|(id, part)| {
            let (path, texture_rect) = if let Some(texture_name) = part.texture.as_deref() {
                let texture =
                    textures
                        .resolve(texture_name)
                        .ok_or_else(|| CharacterCatalogError::Data {
                            path: config_path.clone(),
                            message: format!(
                                "part `{id}` references undefined texture `{texture_name}`"
                            ),
                        })?;
                (texture.path.clone(), texture.rect)
            } else {
                let path = part
                    .path
                    .as_deref()
                    .ok_or_else(|| CharacterCatalogError::Data {
                        path: config_path.clone(),
                        message: format!("part `{id}` requires `texture` or `path`"),
                    })?;
                (vfs.resolve_path(Some(&config_path), path), None)
            };
            let rect = part
                .rect
                .map(|rect| {
                    let left = rect[0] as f32;
                    let top = rect[1] as f32;
                    [left, top, left + rect[2] as f32, top + rect[3] as f32]
                })
                .or_else(|| {
                    texture_rect
                        .map(|rect| [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]])
                });
            Ok(CharacterPartDefinition {
                id,
                slot: part
                    .slot
                    .as_deref()
                    .and_then(|name| slots.get(name).copied()),
                path,
                atlas_rect: texture_rect,
                offset: part
                    .offset
                    .map(|offset| Vec2::new(offset[0] as f32, offset[1] as f32))
                    .unwrap_or(Vec2::ZERO),
                layer: part.layer.unwrap_or(0.0) as f32,
                rect,
            })
        })
        .collect::<Result<Vec<_>, CharacterCatalogError>>()?;
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
                    slot: slot.as_deref().and_then(|name| slots.get(name).copied()),
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

    Ok(CharacterDefinition {
        name,
        directory,
        config_path,
        slots,
        parts,
        expressions,
        basis: config.basis,
        default_expression: config.default_expression,
    })
}

fn build_slot_indices(
    config: &CharacterConfigFile,
    path: &str,
) -> Result<BTreeMap<String, usize>, CharacterCatalogError> {
    let mut slots = BTreeMap::new();
    for name in &config.slots {
        let index = slots.len();
        if slots.insert(name.clone(), index).is_some() {
            return Err(CharacterCatalogError::Data {
                path: path.to_string(),
                message: format!("slot `{name}` is declared more than once"),
            });
        }
    }

    // Older files only name slots inside parts/expressions. Keep accepting
    // those files while assigning their extra slots deterministically.
    let mut implicit = config
        .parts
        .values()
        .filter_map(|part| part.slot.clone())
        .chain(
            config
                .expressions
                .values()
                .filter_map(|expression| match expression {
                    CharacterExpressionFile::Parts(_) => None,
                    CharacterExpressionFile::Definition { slot, .. } => slot.clone(),
                }),
        )
        .collect::<Vec<_>>();
    implicit.sort();
    implicit.dedup();
    for name in implicit {
        if !slots.contains_key(&name) {
            let index = slots.len();
            slots.insert(name, index);
        }
    }
    Ok(slots)
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

fn parse_hks_data<T>(path: &str, source: &str) -> Result<T, CharacterCatalogError>
where
    T: DeserializeOwned,
{
    let data = evaluate_hks_map(path, source).map_err(|error| CharacterCatalogError::Data {
        path: path.to_string(),
        message: error.to_string(),
    })?;
    serde_json::from_value(serde_json::Value::Object(data)).map_err(|error| {
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
    fn loads_hks_character_catalog_and_parts() {
        let root =
            std::env::temp_dir().join(format!("hiraku-character-test-{}", std::process::id()));
        let characters = root.join("characters/alice");
        std::fs::create_dir_all(&characters).unwrap();
        std::fs::write(
            root.join("settings.data.hks"),
            ".{ charactersDir: \"characters\" }",
        )
        .unwrap();
        std::fs::write(
            root.join("characters/characters.data.hks"),
            ".{ characters: (.{ name: \"alice\", dir: \"alice\" }) }",
        )
        .unwrap();
        std::fs::write(
            characters.join("character.data.hks"),
            ".{ slots: (\"body\", \"face\"), parts: .{ body: .{ path: \"body.png\", slot: \"body\", offset: (12.5, -3.0), layer: -1.0 }, face: .{ path: \"face.png\", slot: \"face\", layer: 2.0 } }, expressions: .{ happy: (\"body\", \"face\") }, default_expression: \"happy\" }",
        )
        .unwrap();

        let vfs = HdpVfs::new_with_config(&root, "settings.data.hks", "startup.story.hks");
        let catalog = load_character_catalog(&vfs).unwrap();
        let alice = &catalog.characters["alice"];

        assert_eq!(alice.parts.len(), 2);
        assert_eq!(alice.parts[0].id, "body");
        assert_eq!(alice.parts[0].offset, Vec2::new(12.5, -3.0));
        assert_eq!(alice.parts[1].id, "face");
        assert_eq!(alice.slots["body"], 0);
        assert_eq!(alice.slots["face"], 1);
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
            slots: BTreeMap::from([("mouth".to_string(), 0), ("face".to_string(), 1)]),
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
                slot: None,
                path: format!("{id}.png"),
                atlas_rect: None,
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
                        slot: Some(0),
                        parts: vec!["mouth_closed".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "mouth_open".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some(0),
                        parts: vec!["mouth_open".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "face_neutral".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some(1),
                        parts: vec!["face_neutral".to_string()],
                        expressions: Vec::new(),
                    },
                ),
                (
                    "face_happy".to_string(),
                    CharacterExpressionDefinition {
                        slot: Some(1),
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
