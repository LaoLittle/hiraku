use hiraku_script::hks::{Expr, ExprKind, NumberUnit, Stmt, parse_program};
use serde_json::{Map, Value as JsonValue};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HksDataError {
    #[error("failed to parse HKS data `{path}`: {message}")]
    Parse { path: String, message: String },
    #[error("HKS data `{path}` must contain exactly one map expression")]
    ExpectedMap { path: String },
    #[error("invalid HKS data `{path}` at byte {offset}: {message}")]
    InvalidValue {
        path: String,
        offset: usize,
        message: String,
    },
}

/// Parses a declarative HKS data document and converts it to a JSON-shaped
/// value for typed engine deserialization. Calls, variables, and control flow
/// are rejected by `literal_value`.
pub fn evaluate_hks_map(path: &str, source: &str) -> Result<Map<String, JsonValue>, HksDataError> {
    let program = parse_program(source).map_err(|errors| HksDataError::Parse {
        path: path.to_string(),
        message: errors
            .into_iter()
            .map(|error| format!("{} at byte {}", error.message, error.span.start))
            .collect::<Vec<_>>()
            .join("; "),
    })?;
    let [Stmt::Expr(expression)] = program.statements.as_slice() else {
        return Err(HksDataError::ExpectedMap {
            path: path.to_string(),
        });
    };
    let ExprKind::Map(fields) = &expression.kind else {
        return Err(HksDataError::ExpectedMap {
            path: path.to_string(),
        });
    };
    let mut map = Map::new();
    for field in fields {
        if map
            .insert(field.name.clone(), literal_value(path, &field.value)?)
            .is_some()
        {
            return Err(HksDataError::InvalidValue {
                path: path.to_string(),
                offset: field.span.start,
                message: format!("duplicate map key `{}`", field.name),
            });
        }
    }
    Ok(map)
}

fn literal_value(path: &str, expression: &Expr) -> Result<JsonValue, HksDataError> {
    let invalid = |message: &str| HksDataError::InvalidValue {
        path: path.to_string(),
        offset: expression.span.start,
        message: message.to_string(),
    };
    match &expression.kind {
        ExprKind::String(value) => Ok(JsonValue::String(value.clone())),
        ExprKind::Bool(value) => Ok(JsonValue::Bool(*value)),
        ExprKind::Number { value, unit } => match unit {
            NumberUnit::Scalar | NumberUnit::Percent => serde_json::Number::from_f64(*value)
                .map(JsonValue::Number)
                .ok_or_else(|| invalid("number is not finite")),
        },
        ExprKind::Symbol(value) => Ok(JsonValue::String(value.clone())),
        ExprKind::UnaryMinus(value) => {
            let ExprKind::Number { value, .. } = value.kind else {
                return Err(invalid("unary minus is only valid for numbers"));
            };
            serde_json::Number::from_f64(-value)
                .map(JsonValue::Number)
                .ok_or_else(|| invalid("number is not finite"))
        }
        ExprKind::Tuple(values) => values
            .iter()
            .map(|value| literal_value(path, value))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        ExprKind::Map(fields) => {
            let mut map = Map::new();
            for field in fields {
                if map
                    .insert(field.name.clone(), literal_value(path, &field.value)?)
                    .is_some()
                {
                    return Err(HksDataError::InvalidValue {
                        path: path.to_string(),
                        offset: field.span.start,
                        message: format!("duplicate map key `{}`", field.name),
                    });
                }
            }
            Ok(JsonValue::Object(map))
        }
        ExprKind::Ident(_)
        | ExprKind::Member { .. }
        | ExprKind::Call { .. }
        | ExprKind::Block(_)
        | ExprKind::Binary { .. } => Err(invalid(
            "data documents may contain only literal, tuple, and map values",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_declarative_hks_map_expression() {
        let data = evaluate_hks_map(
            "settings.data.hks",
            ".{ startup: \"startup.story.hks\", fonts: .{ path: \"fonts\" } }",
        )
        .unwrap();
        assert_eq!(data["startup"], "startup.story.hks");
        assert_eq!(data["fonts"]["path"], "fonts");
    }

    #[test]
    fn accepts_nested_tuples_and_maps() {
        let data = evaluate_hks_map(
            "character.data.hks",
            ".{ slots: (\"body\", \"face\"), offset: (12.5, -3.0) }",
        )
        .unwrap();
        assert_eq!(data["slots"][0], "body");
        assert_eq!(data["offset"][1], -3.0);
    }

    #[test]
    fn rejects_procedural_documents() {
        let error = evaluate_hks_map("settings.data.hks", "let startup = \"startup\"").unwrap_err();
        assert!(matches!(error, HksDataError::ExpectedMap { .. }));
    }
}
