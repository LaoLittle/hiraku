//! Stable script/embedding ABI shared by the compiler, linker and register VM.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ScriptType, SymbolId, SymbolManifest};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BuiltinId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub receiver: Option<ScriptType>,
    pub parameters: Vec<ScriptType>,
    pub result: ScriptType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StaticMemberKind {
    Method,
    Getter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticMember {
    pub owner: SymbolId,
    pub name: SymbolId,
    pub builtin: BuiltinId,
    pub kind: StaticMemberKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinManifest {
    hash: u64,
    names: BTreeMap<String, BuiltinId>,
    selectors: BTreeMap<(String, String), BuiltinId>,
    operators: BTreeMap<String, BuiltinId>,
    #[serde(default)]
    symbols: SymbolManifest,
    #[serde(default)]
    signatures: BTreeMap<BuiltinId, FunctionSignature>,
    #[serde(default)]
    static_members: Vec<StaticMember>,
    #[serde(default)]
    globals: BTreeMap<String, ScriptType>,
}

impl BuiltinManifest {
    pub fn new(entries: impl IntoIterator<Item = (impl Into<String>, BuiltinId)>) -> Self {
        Self::with_selectors(
            entries
                .into_iter()
                .map(|(name, id)| (name.into(), id))
                .collect(),
            BTreeMap::new(),
        )
    }

    pub fn with_selectors(
        names: BTreeMap<String, BuiltinId>,
        selectors: BTreeMap<(String, String), BuiltinId>,
    ) -> Self {
        Self::with_operators(names, selectors, BTreeMap::new())
    }

    pub fn with_operators(
        names: BTreeMap<String, BuiltinId>,
        selectors: BTreeMap<(String, String), BuiltinId>,
        operators: BTreeMap<String, BuiltinId>,
    ) -> Self {
        let mut value = Self {
            hash: 0,
            names,
            selectors,
            operators,
            symbols: SymbolManifest::default(),
            signatures: BTreeMap::new(),
            static_members: Vec::new(),
            globals: BTreeMap::new(),
        };
        value.rehash();
        value
    }

    pub fn with_type_metadata(
        mut self,
        symbols: SymbolManifest,
        signatures: BTreeMap<BuiltinId, FunctionSignature>,
        static_members: Vec<StaticMember>,
    ) -> Self {
        self.symbols = symbols;
        self.signatures = signatures;
        self.static_members = static_members;
        self.rehash();
        self
    }

    pub fn with_globals(mut self, globals: BTreeMap<String, ScriptType>) -> Self {
        self.globals = globals;
        self.rehash();
        self
    }

    pub fn globals(&self) -> &BTreeMap<String, ScriptType> {
        &self.globals
    }
    pub fn hash(&self) -> u64 {
        self.hash
    }
    pub fn resolve(&self, name: &str) -> Option<BuiltinId> {
        self.names.get(name).copied()
    }
    pub fn resolve_selector(&self, selector: &str, method: &str) -> Option<BuiltinId> {
        self.selectors
            .get(&(selector.to_string(), method.to_string()))
            .copied()
    }
    pub fn has_selector(&self, selector: &str) -> bool {
        self.selectors
            .keys()
            .any(|(candidate, _)| candidate == selector)
    }
    pub fn resolve_operator(&self, operator: &str) -> Option<BuiltinId> {
        self.operators.get(operator).copied()
    }
    pub fn symbols(&self) -> &SymbolManifest {
        &self.symbols
    }
    pub fn signature(&self, builtin: BuiltinId) -> Option<&FunctionSignature> {
        self.signatures.get(&builtin)
    }
    pub fn resolve_static_method(&self, name: &str) -> Result<&StaticMember, &'static str> {
        self.resolve_static(name, StaticMemberKind::Method)
    }
    pub fn resolve_getter(&self, name: &str) -> Result<&StaticMember, &'static str> {
        self.resolve_static(name, StaticMemberKind::Getter)
    }
    fn resolve_static(
        &self,
        name: &str,
        kind: StaticMemberKind,
    ) -> Result<&StaticMember, &'static str> {
        let symbol = self.symbols.find(name).ok_or("unknown")?;
        let mut found = self
            .static_members
            .iter()
            .filter(|member| member.name == symbol && member.kind == kind);
        let value = found.next().ok_or("unknown")?;
        if found.next().is_some() {
            Err("ambiguous")
        } else {
            Ok(value)
        }
    }
    pub fn callable_name(&self, builtin: BuiltinId) -> Option<String> {
        self.callable_name_candidates()
            .find_map(|(name, id)| (id == builtin).then_some(name))
    }
    pub fn callable_name_candidates(&self) -> impl Iterator<Item = (String, BuiltinId)> + '_ {
        self.names
            .iter()
            .map(|(name, id)| (name.clone(), *id))
            .chain(
                self.selectors
                    .iter()
                    .map(|((owner, name), id)| (format!("{owner}.{name}"), *id)),
            )
            .chain(
                self.operators
                    .iter()
                    .map(|(name, id)| (format!("operator {name}"), *id)),
            )
            .chain(self.static_members.iter().map(|member| {
                (
                    format!(
                        "{}.{}",
                        self.symbols.resolve(member.owner).unwrap_or("<unknown>"),
                        self.symbols.resolve(member.name).unwrap_or("<unknown>")
                    ),
                    member.builtin,
                )
            }))
    }
    fn rehash(&mut self) {
        let mut hash = 0xcbf2_9ce4_8422_2325;
        for (name, id) in self.callable_name_candidates() {
            for byte in name.bytes().chain(id.0.to_le_bytes()) {
                hash = hash_byte(hash, byte);
            }
        }
        for symbol in self.symbols.symbols() {
            for byte in symbol.bytes() {
                hash = hash_byte(hash, byte);
            }
        }
        for (id, signature) in &self.signatures {
            for byte in
                id.0.to_le_bytes()
                    .into_iter()
                    .chain(format!("{signature:?}").bytes())
            {
                hash = hash_byte(hash, byte);
            }
        }
        for (name, ty) in &self.globals {
            for byte in name.bytes().chain(format!("{ty:?}").bytes()) {
                hash = hash_byte(hash, byte);
            }
        }
        self.hash = hash;
    }
}

fn hash_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Uninitialized,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(String),
    Selector(String),
    RegisterClosure {
        region: u32,
        statements: Vec<u32>,
        captures: Vec<Value>,
    },
    Typed {
        type_id: SymbolId,
        value: Box<Value>,
    },
    Handle {
        type_id: u32,
        id: u64,
    },
    Task(u64),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinCall {
    pub builtin: BuiltinId,
    pub receiver: Option<Value>,
    pub arguments: Vec<CallArgument>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallArgument {
    pub label: Option<String>,
    pub value: Value,
}
