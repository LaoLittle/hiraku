//! The compiler-only operations available to the script standard library.
//! Public conveniences are ordinary HKS functions, not compiler name checks.
use crate::ScriptType;

/// Operator spelling is a language protocol, not an embedding capability.
pub fn binary_protocol(op: crate::BinaryOp) -> Option<(&'static str, &'static str)> {
    use crate::BinaryOp::*;
    Some(match op {
        Add => ("Add", "add"),
        Subtract => ("Subtract", "subtract"),
        Multiply => ("Multiply", "multiply"),
        Divide => ("Divide", "divide"),
        Equal => ("Equal", "equal"),
        NotEqual => ("NotEqual", "notEqual"),
        Less => ("Less", "less"),
        LessEqual => ("LessEqual", "lessEqual"),
        Greater => ("Greater", "greater"),
        GreaterEqual => ("GreaterEqual", "greaterEqual"),
        Colon => ("Colon", "colon"),
        And | Or => return None,
    })
}

pub fn binary(name: &str) -> Option<(crate::BinaryOp, ScriptType, ScriptType)> {
    let suffix = name.strip_prefix("intrinsics.")?;
    let (owner, operation) = suffix.split_once('.')?;
    let ty = match owner {
        "int" => ScriptType::Int,
        "float" => ScriptType::Float,
        "string" => ScriptType::String,
        "bool" => ScriptType::Bool,
        _ => return None,
    };
    use crate::BinaryOp::*;
    let op = match operation {
        "add" => Add,
        "subtract" => Subtract,
        "multiply" => Multiply,
        "divide" => Divide,
        "equal" => Equal,
        "notEqual" => NotEqual,
        "less" => Less,
        "lessEqual" => LessEqual,
        "greater" => Greater,
        "greaterEqual" => GreaterEqual,
        _ => return None,
    };
    let comparison = matches!(
        op,
        Equal | NotEqual | Less | LessEqual | Greater | GreaterEqual
    );
    if ty == ScriptType::Bool && !matches!(op, Equal | NotEqual) {
        return None;
    }
    if ty == ScriptType::String && !matches!(op, Add | Equal | NotEqual) {
        return None;
    }
    let result = if comparison {
        ScriptType::Bool
    } else {
        ty.clone()
    };
    Some((op, ty, result))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intrinsic {
    Negate,
    Not,
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
        name: "intrinsics.int.negate",
        operation: Intrinsic::Negate,
        parameter: ScriptType::Int,
        result: ScriptType::Int,
    },
    IntrinsicDefinition {
        name: "intrinsics.float.negate",
        operation: Intrinsic::Negate,
        parameter: ScriptType::Float,
        result: ScriptType::Float,
    },
    IntrinsicDefinition {
        name: "intrinsics.bool.not",
        operation: Intrinsic::Not,
        parameter: ScriptType::Bool,
        result: ScriptType::Bool,
    },
    IntrinsicDefinition {
        name: "intrinsics.toString",
        operation: Intrinsic::ValueToString,
        parameter: ScriptType::Any,
        result: ScriptType::String,
    },
    IntrinsicDefinition {
        name: "intrinsics.intToFloat",
        operation: Intrinsic::IntToFloat,
        parameter: ScriptType::Int,
        result: ScriptType::Float,
    },
    IntrinsicDefinition {
        name: "intrinsics.panic",
        operation: Intrinsic::Panic,
        parameter: ScriptType::String,
        result: ScriptType::Never,
    },
    IntrinsicDefinition {
        name: "intrinsics.floatToInt",
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
