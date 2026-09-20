//! Deterministic symbols shared by API manifests, bytecode metadata and snapshots.

use crate::SharedStrings;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum InternedSymbol {
    Shared(lasso::Spur),
    Owned(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolManifest {
    symbols: Vec<String>,
}

impl SymbolManifest {
    pub fn symbols(&self) -> &[String] {
        &self.symbols
    }

    pub fn resolve(&self, id: SymbolId) -> Option<&str> {
        self.symbols.get(id.0 as usize).map(String::as_str)
    }

    pub fn find(&self, symbol: &str) -> Option<SymbolId> {
        self.symbols
            .iter()
            .position(|candidate| candidate == symbol)
            .and_then(|index| u32::try_from(index).ok())
            .map(SymbolId)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SymbolInterner {
    symbols: IndexMap<InternedSymbol, SymbolId>,
    strings: SharedStrings,
}

impl SymbolInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, symbol: impl Into<String>) -> SymbolId {
        let symbol = symbol.into();
        if let Some(id) = self.get(&symbol) {
            return id;
        }
        let symbol = match self.strings.intern(&symbol) {
            Some(key) => InternedSymbol::Shared(key),
            None => InternedSymbol::Owned(symbol),
        };
        if let Some(id) = self.symbols.get(&symbol) {
            return *id;
        }
        let id = SymbolId(
            u32::try_from(self.symbols.len()).expect("symbol manifest exceeds u32 capacity"),
        );
        self.symbols.insert(symbol, id);
        id
    }

    pub fn get(&self, symbol: &str) -> Option<SymbolId> {
        let key = match self.strings.get(symbol) {
            Some(key) => InternedSymbol::Shared(key),
            None => InternedSymbol::Owned(symbol.to_owned()),
        };
        self.symbols.get(&key).copied().or_else(|| {
            self.symbols
                .get(&InternedSymbol::Owned(symbol.to_owned()))
                .copied()
        })
    }

    pub fn resolve(&self, id: SymbolId) -> Option<&str> {
        self.symbols
            .get_index(id.0 as usize)
            .map(|(symbol, _)| match symbol {
                InternedSymbol::Shared(key) => self.strings.resolve(*key),
                InternedSymbol::Owned(value) => value.as_str(),
            })
    }

    pub fn manifest(&self) -> SymbolManifest {
        SymbolManifest {
            symbols: (0..self.symbols.len())
                .map(|i| {
                    self.resolve(SymbolId(i as u32))
                        .expect("manifest index exists")
                        .to_owned()
                })
                .collect(),
        }
    }

    pub fn from_manifest(manifest: SymbolManifest) -> Result<Self, SymbolManifestError> {
        let mut interner = Self::new();
        for symbol in manifest.symbols {
            if interner.get(&symbol).is_some() {
                return Err(SymbolManifestError::Duplicate(symbol));
            }
            interner.intern(symbol);
        }
        Ok(interner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolManifestError {
    Duplicate(String),
}

impl std::fmt::Display for SymbolManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(symbol) => write!(formatter, "duplicate manifest symbol `{symbol}`"),
        }
    }
}

impl std::error::Error for SymbolManifestError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_pool_order_never_changes_manifest_ids() {
        let first_pool = SharedStrings::with_budget(4096);
        let second_pool = SharedStrings::with_budget(4096);
        first_pool.intern("bob");
        second_pool.intern("alice");
        let mut first = SymbolInterner {
            strings: first_pool,
            ..Default::default()
        };
        let mut second = SymbolInterner {
            strings: second_pool,
            ..Default::default()
        };
        for name in ["alice", "bob", "Position", "relative"] {
            assert_eq!(first.intern(name), second.intern(name));
        }
        assert_eq!(first.manifest(), second.manifest());
    }

    #[test]
    fn full_pool_falls_back_without_losing_symbols() {
        let mut symbols = SymbolInterner {
            strings: SharedStrings::with_budget(1),
            ..Default::default()
        };
        let id = symbols.intern("alice");
        assert_eq!(symbols.get("alice"), Some(id));
        assert_eq!(symbols.intern("alice"), id);
        assert_eq!(symbols.resolve(id), Some("alice"));
    }

    #[test]
    fn manifest_preserves_ids_across_restore() {
        let mut interner = SymbolInterner::new();
        let position = interner.intern("Position");
        let relative = interner.intern("relative");
        let manifest = interner.manifest();
        let restored = SymbolInterner::from_manifest(manifest).expect("manifest must restore");
        assert_eq!(restored.get("Position"), Some(position));
        assert_eq!(restored.get("relative"), Some(relative));
        assert_eq!(restored.resolve(relative), Some("relative"));
    }
}
