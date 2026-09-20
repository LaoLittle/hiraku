//! Portable type contracts for independently compiled execution domains.
//!
//! This is compiler metadata, not permission to transfer values or heap IDs.
//! The host must supply stable logical module paths when compiling both domains.
use std::collections::BTreeMap;

use crate::{FunctionSignature, ScriptType, SymbolId, SymbolInterner, SymbolManifest};

/// A complete signature whose nominal identities do not depend on native
/// registration order or the source program's symbol numbering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionContract {
    signature: FunctionSignature,
    symbols: SymbolManifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    MissingModule(crate::ModuleId),
    MissingFunction {
        module: crate::ModuleId,
        symbol: SymbolId,
    },
    MissingSymbol(SymbolId),
    MissingOrigin(String),
    UnsupportedType(ScriptType),
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingModule(module) => {
                write!(f, "type contract refers to missing module {module:?}")
            }
            Self::MissingFunction { module, symbol } => write!(
                f,
                "type contract refers to missing function {symbol:?} in {module:?}"
            ),
            Self::MissingSymbol(id) => write!(f, "type contract refers to missing symbol {id:?}"),
            Self::MissingOrigin(name) => write!(
                f,
                "type `{name}` has no script declaration provenance; register a shared contract before crossing execution domains"
            ),
            Self::UnsupportedType(ty) => write!(
                f,
                "type contract contains unresolved or host-specific type {ty:?}"
            ),
        }
    }
}

impl std::error::Error for ContractError {}

impl FunctionContract {
    pub fn new(
        signature: &FunctionSignature,
        symbols: &SymbolManifest,
        origins: &BTreeMap<String, String>,
    ) -> Result<Self, ContractError> {
        let mut canonical = Canonicalizer {
            symbols,
            origins,
            output: SymbolInterner::new(),
        };
        let signature = FunctionSignature {
            receiver: signature
                .receiver
                .as_ref()
                .map(|ty| canonical.ty(ty))
                .transpose()?,
            parameters: signature
                .parameters
                .iter()
                .map(|ty| canonical.ty(ty))
                .collect::<Result<_, _>>()?,
            variadic: signature
                .variadic
                .as_ref()
                .map(|ty| canonical.ty(ty))
                .transpose()?,
            result: canonical.ty(&signature.result)?,
        };
        Ok(Self {
            signature,
            symbols: canonical.output.manifest(),
        })
    }
}

struct Canonicalizer<'a> {
    symbols: &'a SymbolManifest,
    origins: &'a BTreeMap<String, String>,
    output: SymbolInterner,
}

impl Canonicalizer<'_> {
    fn nominal(&mut self, symbol: SymbolId) -> Result<SymbolId, ContractError> {
        let name = self
            .symbols
            .resolve(symbol)
            .ok_or(ContractError::MissingSymbol(symbol))?;
        let origin = self
            .origins
            .get(name)
            .ok_or_else(|| ContractError::MissingOrigin(name.into()))?;
        // Length-prefix the path so punctuation in module paths cannot alias a
        // different (module, declaration) pair.
        Ok(self
            .output
            .intern(format!("{}:{origin}{name}", origin.len())))
    }

    fn types(&mut self, types: &[ScriptType]) -> Result<Vec<ScriptType>, ContractError> {
        types.iter().map(|ty| self.ty(ty)).collect()
    }

    fn ty(&mut self, ty: &ScriptType) -> Result<ScriptType, ContractError> {
        use ScriptType as T;
        Ok(match ty {
            T::Struct {
                name,
                arguments,
                fields,
            } => T::Struct {
                name: self.nominal(*name)?,
                arguments: self.types(arguments)?,
                fields: fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.ty(value)?)))
                    .collect::<Result<_, ContractError>>()?,
            },
            T::Enum {
                name,
                arguments,
                variants,
            } => T::Enum {
                name: self.nominal(*name)?,
                arguments: self.types(arguments)?,
                variants: variants
                    .iter()
                    .map(|(key, values)| Ok((key.clone(), self.types(values)?)))
                    .collect::<Result<_, ContractError>>()?,
            },
            T::Callable { parameters, result } => T::Callable {
                parameters: self.types(parameters)?,
                result: Box::new(self.ty(result)?),
            },
            T::TupleOf(types) => T::TupleOf(self.types(types)?),
            T::Union(types) => T::Union(self.types(types)?),
            T::Optional(inner) => T::Optional(Box::new(self.ty(inner)?)),
            T::List(inner) => T::List(Box::new(self.ty(inner)?)),
            T::Map(key, value) => T::Map(Box::new(self.ty(key)?), Box::new(self.ty(value)?)),
            T::Record(fields) => T::Record(
                fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.ty(value)?)))
                    .collect::<Result<_, ContractError>>()?,
            ),
            T::Named(_)
            | T::TypeParameter(_)
            | T::Binding(_)
            | T::Function
            | T::Task
            | T::Selector => {
                return Err(ContractError::UnsupportedType(ty.clone()));
            }
            T::Never
            | T::Any
            | T::Unit
            | T::Ellipsis
            | T::Bool
            | T::Int
            | T::UInt
            | T::Float
            | T::Percent
            | T::String
            | T::TextTemplate
            | T::Symbol
            | T::Tuple => ty.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(prefix: bool, origin: &str, payload: ScriptType) -> FunctionContract {
        let mut symbols = SymbolInterner::new();
        if prefix {
            symbols.intern("unrelated");
        }
        let name = symbols.intern("Reply");
        let reply = ScriptType::Enum {
            name,
            arguments: vec![payload.clone()],
            variants: BTreeMap::from([
                ("accepted".into(), vec![payload]),
                ("cancelled".into(), vec![]),
            ]),
        };
        let signature = FunctionSignature {
            receiver: Some(reply.clone()),
            parameters: vec![ScriptType::Callable {
                parameters: vec![ScriptType::List(Box::new(ScriptType::Optional(Box::new(
                    reply.clone(),
                ))))],
                result: Box::new(ScriptType::TupleOf(vec![ScriptType::String, reply.clone()])),
            }],
            variadic: Some(reply.clone()),
            result: reply,
        };
        FunctionContract::new(
            &signature,
            &symbols.manifest(),
            &BTreeMap::from([("Reply".into(), origin.into())]),
        )
        .expect("complete nominal contract")
    }

    #[test]
    fn nested_generic_enum_signatures_are_independent_of_symbol_numbering() {
        let left = contract(false, "contracts.hks", ScriptType::String);
        assert_eq!(left, contract(true, "contracts.hks", ScriptType::String));
        assert_ne!(left, contract(false, "other.hks", ScriptType::String));
        assert_ne!(left, contract(false, "contracts.hks", ScriptType::Int));
    }

    #[test]
    fn incomplete_contracts_fail_closed() {
        let mut symbols = SymbolInterner::new();
        let name = symbols.intern("Input");
        let struct_type = ScriptType::Struct {
            name,
            arguments: vec![],
            fields: BTreeMap::new(),
        };
        let signature = |ty| FunctionSignature {
            receiver: None,
            parameters: vec![ty],
            variadic: None,
            result: ScriptType::Unit,
        };
        assert!(matches!(
            FunctionContract::new(
                &signature(struct_type.clone()),
                &symbols.manifest(),
                &BTreeMap::new()
            ),
            Err(ContractError::MissingOrigin(_))
        ));
        assert!(matches!(
            FunctionContract::new(
                &signature(struct_type),
                &SymbolManifest::default(),
                &BTreeMap::new()
            ),
            Err(ContractError::MissingSymbol(_))
        ));
        for ty in [
            ScriptType::Named(name),
            ScriptType::TypeParameter(name),
            ScriptType::Function,
            ScriptType::Task,
        ] {
            assert!(matches!(
                FunctionContract::new(&signature(ty), &symbols.manifest(), &BTreeMap::new()),
                Err(ContractError::UnsupportedType(_))
            ));
        }
    }
}
