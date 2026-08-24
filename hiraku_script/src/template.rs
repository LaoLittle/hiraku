//! Host-evaluated string templates.
//!
//! Templates deliberately resolve values through an embedding context instead of giving the VM
//! access to engine state. The first version accepts identifier paths such as `${player.name}`.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::vm::Value;

pub trait TemplateContext {
    fn resolve_template_path(&mut self, path: &[&str]) -> Result<Value, TemplateError>;
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
        let end = expression
            .find('}')
            .ok_or(TemplateError::UnterminatedExpression)?;
        let path = expression[..end].trim();
        let segments = parse_path(path)?;
        let value = context.resolve_template_path(&segments)?;
        output.push_str(&display_value(value)?);
        remaining = &expression[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn parse_path(path: &str) -> Result<Vec<&str>, TemplateError> {
    if path.is_empty() {
        return Err(TemplateError::InvalidPath(path.to_string()));
    }
    let segments = path.split('.').collect::<Vec<_>>();
    if segments.iter().any(|segment| {
        let mut chars = segment.chars();
        !chars.next().is_some_and(|character| {
            character == '_' || unicode_xid::UnicodeXID::is_xid_start(character)
        }) || !chars.all(|character| unicode_xid::UnicodeXID::is_xid_continue(character))
    }) {
        return Err(TemplateError::InvalidPath(path.to_string()));
    }
    Ok(segments)
}

fn display_value(value: Value) -> Result<String, TemplateError> {
    match value {
        Value::String(value) | Value::Symbol(value) => Ok(value),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok("null".to_string()),
        Value::Uninitialized => Err(TemplateError::UninitializedValue),
        value => Err(TemplateError::UnsupportedValue(format!("{value:?}"))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    UnterminatedExpression,
    InvalidPath(String),
    UnknownPath(String),
    UninitializedValue,
    UnsupportedValue(String),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedExpression => formatter.write_str("unterminated template expression"),
            Self::InvalidPath(path) => write!(formatter, "invalid template path `{path}`"),
            Self::UnknownPath(path) => write!(formatter, "unknown template path `{path}`"),
            Self::UninitializedValue => formatter.write_str("template value is uninitialized"),
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
                Value::String("Alice".to_string()),
            )])),
        )]);
        assert_eq!(
            eval_template("Hi, ${player.name}", &mut context).expect("template must evaluate"),
            "Hi, Alice"
        );
    }
}
