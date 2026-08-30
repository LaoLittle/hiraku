use bevy::prelude::Resource;
use hiraku_script::{SymbolId, SymbolInterner, hson::HsonValue};
use thiserror::Error;

use crate::{
    data::evaluate_hson_map,
    vfs::{HdpVfs, VfsError},
};

pub const DEFAULT_GLOSSARY_PATH: &str = "glossary.hson";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermId(SymbolId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, Resource)]
pub struct TermCatalog {
    interner: SymbolInterner,
    terms: Vec<TermDefinition>,
}

impl TermCatalog {
    pub fn resolve(&self, id: &str) -> Option<TermId> {
        self.interner.get(id).map(TermId)
    }

    pub fn get(&self, id: TermId) -> Option<&TermDefinition> {
        self.terms.get(id.0.0 as usize)
    }

    fn insert(&mut self, term: TermDefinition) -> Result<TermId, GlossaryError> {
        if self.interner.get(&term.id).is_some() {
            return Err(GlossaryError::DuplicateTerm(term.id));
        }
        let id = TermId(self.interner.intern(term.id.clone()));
        debug_assert_eq!(id.0.0 as usize, self.terms.len());
        self.terms.push(term);
        Ok(id)
    }
}

#[derive(Debug, Error)]
pub enum GlossaryError {
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error("invalid glossary `{path}`: {message}")]
    Invalid { path: String, message: String },
    #[error("term `{0}` is defined more than once")]
    DuplicateTerm(String),
}

pub fn load_term_catalog(vfs: &HdpVfs) -> Result<TermCatalog, GlossaryError> {
    let path = vfs.resolve_path(Some(vfs.settings_path()), DEFAULT_GLOSSARY_PATH);
    if !vfs.exists(&path) {
        return Ok(TermCatalog::default());
    }
    let source = vfs.read_text(&path)?;
    parse_term_catalog(&path, &source)
}

pub(crate) fn parse_term_catalog(path: &str, source: &str) -> Result<TermCatalog, GlossaryError> {
    let mut root = evaluate_hson_map(path, source).map_err(|error| GlossaryError::Invalid {
        path: path.to_string(),
        message: error.to_string(),
    })?;
    let terms = root
        .remove("terms")
        .ok_or_else(|| invalid(path, "missing `terms` list"))?;
    if !root.is_empty() {
        return Err(invalid(
            path,
            format!(
                "unknown field(s): {}",
                root.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let HsonValue::Array(terms) = terms else {
        return Err(invalid(path, "`terms` must be a list"));
    };
    let mut catalog = TermCatalog::default();
    for (index, value) in terms.into_iter().enumerate() {
        let HsonValue::Map(mut term) = value else {
            return Err(invalid(
                path,
                format!("term at index {index} must be a map"),
            ));
        };
        let id = take_string(&mut term, "id", path, index)?;
        let name = take_string(&mut term, "name", path, index)?;
        let description = take_string(&mut term, "description", path, index)?;
        if !term.is_empty() {
            return Err(invalid(
                path,
                format!(
                    "term `{id}` has unknown field(s): {}",
                    term.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        catalog.insert(TermDefinition {
            id,
            name,
            description,
        })?;
    }
    Ok(catalog)
}

fn take_string(
    map: &mut hiraku_script::hson::HsonMap,
    key: &str,
    path: &str,
    index: usize,
) -> Result<String, GlossaryError> {
    map.remove(key)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| {
            invalid(
                path,
                format!("term at index {index} requires string `{key}`"),
            )
        })
}

fn invalid(path: &str, message: impl Into<String>) -> GlossaryError {
    GlossaryError::Invalid {
        path: path.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_user_defined_string_term_ids() {
        let catalog = parse_term_catalog(
            "memory://glossary.hson",
            r#".{ terms: [
                .{ id: "ether", name: "Ether", description: "A fictional substance." },
                .{ id: "academy", name: "Academy", description: "Alice's school." },
            ] }"#,
        )
        .expect("glossary should parse");

        let ether = catalog.resolve("ether").expect("term should be interned");
        assert_eq!(catalog.get(ether).expect("term exists").name, "Ether");
        assert!(catalog.resolve("unknown").is_none());
    }
}
