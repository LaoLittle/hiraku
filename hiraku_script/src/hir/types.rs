use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::SymbolId;

/// Compiler-visible HKS type. Runtime values do not carry this structure except
/// for embedding-defined nominal type IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptType {
    Never,
    Any,
    Unit,
    Ellipsis,
    Bool,
    Int,
    UInt,
    Float,
    Percent,
    String,
    TextTemplate,
    Symbol,
    Selector,
    Function,
    Callable {
        parameters: Vec<ScriptType>,
        result: Box<ScriptType>,
    },
    TupleOf(Vec<ScriptType>),
    /// An explicitly captured expression whose scheduling is owned by the embedding.
    Binding(Box<ScriptType>),
    Task,
    TypeParameter(SymbolId),
    Named(SymbolId),
    Enum {
        name: SymbolId,
        arguments: Vec<ScriptType>,
        variants: BTreeMap<String, Vec<ScriptType>>,
    },
    Struct {
        name: SymbolId,
        arguments: Vec<ScriptType>,
        fields: BTreeMap<String, ScriptType>,
    },
    Union(Vec<ScriptType>),
    /// `Optional<T>`. The source spelling `T?` is sugar for this type.
    Optional(Box<ScriptType>),
    Tuple,
    List(Box<ScriptType>),
    Record(BTreeMap<String, ScriptType>),
    Map(Box<ScriptType>, Box<ScriptType>),
}

impl ScriptType {
    /// Dynamic native arguments are validated by the host's typed conversion.
    /// This does not make Any assignable to a concrete script binding.
    pub(crate) fn accepts_native_argument(&self, actual: &Self) -> bool {
        self.accepts(actual)
            || actual == &Self::Any
            || matches!(self, Self::Union(types) if types.iter().any(|ty| ty.accepts_native_argument(actual)))
            || matches!((self, actual), (Self::Binding(expected), Self::Binding(actual)) if expected.accepts_native_argument(actual))
    }

    pub(crate) fn accepts(&self, actual: &Self) -> bool {
        self == &Self::Any
            || actual == &Self::Never
            || self == actual
            || matches!((self, actual), (Self::Function, Self::Callable { .. }))
            || matches!((self, actual), (Self::Tuple, Self::TupleOf(_)))
            || matches!((self, actual), (Self::TupleOf(expected), Self::TupleOf(actual))
                if expected.len() == actual.len() && expected.iter().zip(actual).all(|(expected, actual)| expected.accepts(actual)))
            || matches!((self, actual), (Self::Callable { parameters: expected, result }, Self::Callable { parameters: actual, result: actual_result })
                if expected.len() == actual.len() && expected.iter().zip(actual).all(|(expected, actual)| actual.accepts(expected)) && result.accepts(actual_result))
            || matches!(self, Self::Union(types) if types.iter().any(|expected| expected.accepts(actual)))
            || matches!((self, actual),
                (Self::Optional(_), Self::Optional(actual)) if actual.as_ref() == &Self::Any)
            || matches!((self, actual),
                (Self::Optional(expected), Self::Optional(actual)) if expected.accepts(actual))
            || matches!(self, Self::Optional(inner) if inner.accepts(actual))
            || matches!((self, actual), (Self::List(expected), Self::List(actual)) if expected.accepts(actual))
            || matches!((self, actual), (Self::Binding(expected), Self::Binding(actual)) if expected.accepts(actual))
            || matches!((self, actual), (Self::Record(expected), Self::Record(actual))
                if expected.len() == actual.len()
                    && expected.iter().all(|(name, expected)|
                        actual.get(name).is_some_and(|actual| expected.accepts(actual))))
            || matches!((self, actual),
                (Self::Map(key, value), Self::Record(fields))
                    if key.accepts(&Self::String)
                        && fields.values().all(|actual| value.accepts(actual)))
            || matches!((self, actual), (Self::TextTemplate, Self::String))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TypeId(pub u32);

/// Canonical type storage used by typed HIR nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeTable {
    types: Vec<ScriptType>,
}

impl TypeTable {
    pub fn intern(&mut self, ty: ScriptType) -> TypeId {
        if let Some(index) = self.types.iter().position(|candidate| candidate == &ty) {
            return TypeId(index as u32);
        }
        let id =
            TypeId(u32::try_from(self.types.len()).expect("HIR type table exceeds u32 capacity"));
        self.types.push(ty);
        id
    }

    pub fn get(&self, id: TypeId) -> Option<&ScriptType> {
        self.types.get(id.0 as usize)
    }

    pub fn types(&self) -> &[ScriptType] {
        &self.types
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structurally_equal_types_share_an_id() {
        let mut types = TypeTable::default();
        let first = types.intern(ScriptType::List(Box::new(ScriptType::String)));
        let second = types.intern(ScriptType::List(Box::new(ScriptType::String)));
        assert_eq!(first, second);
        assert_eq!(types.types().len(), 1);
    }
}
