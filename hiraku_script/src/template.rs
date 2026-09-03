//! Host-evaluated string templates.
//!
//! Templates deliberately resolve values and pure calls through an embedding context instead of
//! giving the VM direct access to engine state.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::{BinaryOp, Expr, ExprKind, Stmt, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateCallArgument {
    pub label: Option<String>,
    pub value: Value,
}

pub trait TemplateContext {
    fn resolve_template_path(&mut self, path: &[&str]) -> Result<Value, TemplateError>;

    /// Invokes an embedding-approved pure function. Effectful story functions
    /// should not be exposed here because template evaluation is synchronous.
    fn call_template_function(
        &mut self,
        name: &str,
        receiver: Option<Value>,
        arguments: &[TemplateCallArgument],
    ) -> Result<Value, TemplateError> {
        let _ = (receiver, arguments);
        Err(TemplateError::UnsupportedCall(name.to_string()))
    }
}

impl TemplateContext for BTreeMap<String, Value> {
    fn resolve_template_path(&mut self, path: &[&str]) -> Result<Value, TemplateError> {
        let full_path = path.join(".");
        let mut value = self
            .get(path[0])
            .ok_or_else(|| TemplateError::UnknownPath(full_path.clone()))?;
        for member in &path[1..] {
            value = match value {
                Value::Map(fields) => fields
                    .get(*member)
                    .ok_or_else(|| TemplateError::UnknownPath(full_path.clone()))?,
                Value::Typed { value, .. } => match value.as_ref() {
                    Value::Map(fields) => fields
                        .get(*member)
                        .ok_or_else(|| TemplateError::UnknownPath(full_path.clone()))?,
                    _ => return Err(TemplateError::UnknownPath(full_path)),
                },
                _ => return Err(TemplateError::UnknownPath(full_path)),
            };
        }
        Ok(value.clone())
    }
}

pub fn eval_template(
    template: &str,
    context: &mut impl TemplateContext,
) -> Result<String, TemplateError> {
    let mut output = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let expression = &remaining[start + 2..];
        let end = expression_end(expression).ok_or(TemplateError::UnterminatedExpression)?;
        let value = evaluate_expression(expression[..end].trim(), context)?;
        output.push_str(&display_value(value)?);
        remaining = &expression[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn expression_end(source: &str) -> Option<usize> {
    let mut braces = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in source.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '{' => braces += 1,
            '}' if braces == 0 => return Some(index),
            '}' => braces -= 1,
            _ => {}
        }
    }
    None
}

fn evaluate_expression(
    source: &str,
    context: &mut impl TemplateContext,
) -> Result<Value, TemplateError> {
    let program = crate::parse_program(source).map_err(|errors| {
        TemplateError::InvalidExpression(
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    let [Stmt::Expr(expression)] = program.statements.as_slice() else {
        return Err(TemplateError::InvalidExpression(
            "expected exactly one expression".to_string(),
        ));
    };
    evaluate_ast(expression, context)
}

fn evaluate_ast(
    expression: &Expr,
    context: &mut impl TemplateContext,
) -> Result<Value, TemplateError> {
    match &expression.kind {
        ExprKind::Null => Ok(Value::Optional(None)),
        ExprKind::Bool(value) => Ok(Value::Bool(*value)),
        ExprKind::Number { value, .. } => Ok(Value::Number(*value)),
        ExprKind::String(value) => Ok(Value::String(eval_template(value, context)?)),
        ExprKind::Ident(name) => context.resolve_template_path(&[name]),
        ExprKind::Member { object, name } => {
            member_value(evaluate_ast(object, context)?, name, false)
        }
        ExprKind::SafeMember { object, name } => {
            member_value(evaluate_ast(object, context)?, name, true)
        }
        ExprKind::Elvis { value, fallback } => {
            let value = evaluate_ast(value, context)?;
            match value {
                Value::Optional(None) | Value::Null => evaluate_ast(fallback, context),
                Value::Optional(Some(value)) => Ok(*value),
                value => Ok(value),
            }
        }
        ExprKind::NonNull(value) => {
            let value = evaluate_ast(value, context)?;
            match value {
                Value::Optional(None) | Value::Null => Err(TemplateError::NullAssertion),
                Value::Optional(Some(value)) => Ok(*value),
                value => Ok(value),
            }
        }
        ExprKind::UnaryMinus(value) => match evaluate_ast(value, context)? {
            Value::Number(value) => Ok(Value::Number(-value)),
            _ => Err(TemplateError::UnsupportedExpression(
                "unary minus requires a number".to_string(),
            )),
        },
        ExprKind::Binary { left, op, right } => {
            let left = evaluate_ast(left, context)?;
            let right = evaluate_ast(right, context)?;
            match op {
                BinaryOp::Equal => Ok(Value::Bool(left == right)),
                BinaryOp::NotEqual => Ok(Value::Bool(left != right)),
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => {
                    let (Value::Number(left), Value::Number(right)) = (left, right) else {
                        return Err(TemplateError::UnsupportedExpression(
                            "arithmetic and comparison operands must be numbers".to_string(),
                        ));
                    };
                    match op {
                        BinaryOp::Add => Ok(Value::Number(left + right)),
                        BinaryOp::Subtract => Ok(Value::Number(left - right)),
                        BinaryOp::Multiply => Ok(Value::Number(left * right)),
                        BinaryOp::Divide if right == 0.0 => Err(
                            TemplateError::UnsupportedExpression("division by zero".to_string()),
                        ),
                        BinaryOp::Divide => Ok(Value::Number(left / right)),
                        BinaryOp::Less => Ok(Value::Bool(left < right)),
                        BinaryOp::LessEqual => Ok(Value::Bool(left <= right)),
                        BinaryOp::Greater => Ok(Value::Bool(left > right)),
                        BinaryOp::GreaterEqual => Ok(Value::Bool(left >= right)),
                        _ => unreachable!("matched numeric template operator"),
                    }
                }
                BinaryOp::Colon => Err(TemplateError::UnsupportedExpression(
                    "operator `:` is host-defined and cannot run in a template".to_string(),
                )),
            }
        }
        ExprKind::Call {
            callee,
            type_arguments,
            arguments,
            trailing_block: None,
        } => {
            if !type_arguments.is_empty() {
                return Err(TemplateError::UnsupportedExpression(
                    "generic calls are not available in templates".to_string(),
                ));
            }
            let (name, receiver) = match &callee.kind {
                ExprKind::Ident(name) => (name.as_str(), None),
                ExprKind::Member { object, name } => {
                    (name.as_str(), Some(evaluate_ast(object, context)?))
                }
                _ => {
                    return Err(TemplateError::UnsupportedExpression(
                        "template call target must be a function or method name".to_string(),
                    ));
                }
            };
            let arguments = arguments
                .iter()
                .map(|argument| {
                    Ok(TemplateCallArgument {
                        label: argument.label.clone(),
                        value: evaluate_ast(&argument.value, context)?,
                    })
                })
                .collect::<Result<Vec<_>, TemplateError>>()?;
            context.call_template_function(name, receiver, &arguments)
        }
        ExprKind::Tuple(values) => values
            .iter()
            .map(|value| evaluate_ast(value, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Tuple),
        ExprKind::List(values) => values
            .iter()
            .map(|value| evaluate_ast(value, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => fields
            .iter()
            .map(|field| Ok((field.name.clone(), evaluate_ast(&field.value, context)?)))
            .collect::<Result<BTreeMap<_, _>, TemplateError>>()
            .map(Value::Map),
        _ => Err(TemplateError::UnsupportedExpression(format!(
            "{:?}",
            expression.kind
        ))),
    }
}

fn member_value(value: Value, name: &str, safe: bool) -> Result<Value, TemplateError> {
    let value = match value {
        Value::Optional(None) | Value::Null if safe => return Ok(Value::Optional(None)),
        Value::Optional(Some(value)) => *value,
        value => value,
    };
    let fields = match value {
        Value::Map(fields) => fields,
        Value::Typed { value, .. } => match *value {
            Value::Map(fields) => fields,
            _ => return Err(TemplateError::UnknownPath(name.to_string())),
        },
        _ => return Err(TemplateError::UnknownPath(name.to_string())),
    };
    fields
        .get(name)
        .cloned()
        .ok_or_else(|| TemplateError::UnknownPath(name.to_string()))
}

fn display_value(value: Value) -> Result<String, TemplateError> {
    match value {
        Value::String(value) | Value::Symbol(value) => Ok(value),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok("null".to_string()),
        Value::Optional(None) => Ok("null".to_string()),
        Value::Optional(Some(value)) => display_value(*value),
        Value::Uninitialized => Err(TemplateError::UninitializedValue),
        value => Err(TemplateError::UnsupportedValue(format!("{value:?}"))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    UnterminatedExpression,
    InvalidExpression(String),
    UnknownPath(String),
    UninitializedValue,
    NullAssertion,
    UnsupportedCall(String),
    UnsupportedExpression(String),
    UnsupportedValue(String),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedExpression => formatter.write_str("unterminated template expression"),
            Self::InvalidExpression(message) => {
                write!(formatter, "invalid template expression: {message}")
            }
            Self::UnknownPath(path) => write!(formatter, "unknown template path `{path}`"),
            Self::UninitializedValue => formatter.write_str("template value is uninitialized"),
            Self::NullAssertion => formatter.write_str("template non-null assertion failed"),
            Self::UnsupportedCall(name) => {
                write!(
                    formatter,
                    "template function `{name}` is not registered as pure"
                )
            }
            Self::UnsupportedExpression(expression) => {
                write!(formatter, "unsupported template expression: {expression}")
            }
            Self::UnsupportedValue(value) => {
                write!(formatter, "template value cannot be displayed: {value}")
            }
        }
    }
}

impl Error for TemplateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_identifier_paths_through_the_host_context() {
        let mut context = BTreeMap::from([(
            "player".to_string(),
            Value::Map(BTreeMap::from([(
                "name".to_string(),
                Value::String("alice".to_string()),
            )])),
        )]);
        assert_eq!(
            eval_template("Hi, ${player.name}", &mut context).expect("template must evaluate"),
            "Hi, alice"
        );
    }

    #[test]
    fn evaluates_elvis_with_a_string_literal_inside_the_template() {
        let mut context = BTreeMap::from([(
            "player".to_string(),
            Value::Map(BTreeMap::from([("name".to_string(), Value::Null)])),
        )]);
        assert_eq!(
            eval_template("Test: ${player.name ?: \"Player\"}", &mut context)
                .expect("template expression must evaluate"),
            "Test: Player"
        );
    }

    struct PureCallContext(BTreeMap<String, Value>);

    impl TemplateContext for PureCallContext {
        fn resolve_template_path(&mut self, path: &[&str]) -> Result<Value, TemplateError> {
            self.0.resolve_template_path(path)
        }

        fn call_template_function(
            &mut self,
            name: &str,
            _receiver: Option<Value>,
            arguments: &[TemplateCallArgument],
        ) -> Result<Value, TemplateError> {
            if name == "fallback" {
                Ok(arguments
                    .iter()
                    .find_map(|argument| {
                        (argument.value != Value::Null).then(|| argument.value.clone())
                    })
                    .unwrap_or(Value::Null))
            } else {
                Err(TemplateError::UnsupportedCall(name.to_string()))
            }
        }
    }

    #[test]
    fn delegates_calls_with_string_arguments_to_the_pure_context() {
        let mut context = PureCallContext(BTreeMap::from([("name".to_string(), Value::Null)]));
        assert_eq!(
            eval_template("${fallback(name, \"Player\")}", &mut context)
                .expect("pure template call must evaluate"),
            "Player"
        );
    }
}
