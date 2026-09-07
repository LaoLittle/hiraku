//! The compiler-only operations available to the script standard library.
//! Public conveniences are ordinary HKS functions, not compiler name checks.
use crate::ScriptType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intrinsic {
    Panic,
    FloatToInt,
    IntToFloat,
    ValueToString,
}

pub struct IntrinsicDefinition {
    pub name: &'static str,
    pub operation: Intrinsic,
    pub parameter: ScriptType,
    pub result: ScriptType,
}

pub static DEFINITIONS: &[IntrinsicDefinition] = &[
    IntrinsicDefinition {
        name: "__builtin_to_string",
        operation: Intrinsic::ValueToString,
        parameter: ScriptType::Any,
        result: ScriptType::String,
    },
    IntrinsicDefinition {
        name: "__builtin_i2f",
        operation: Intrinsic::IntToFloat,
        parameter: ScriptType::Int,
        result: ScriptType::Float,
    },
    IntrinsicDefinition {
        name: "__builtin_panic",
        operation: Intrinsic::Panic,
        parameter: ScriptType::String,
        result: ScriptType::Never,
    },
    IntrinsicDefinition {
        name: "__builtin_f2i",
        operation: Intrinsic::FloatToInt,
        parameter: ScriptType::Float,
        result: ScriptType::Int,
    },
];

pub fn resolve(name: &str) -> Option<&'static IntrinsicDefinition> {
    DEFINITIONS
        .iter()
        .find(|definition| definition.name == name)
}
