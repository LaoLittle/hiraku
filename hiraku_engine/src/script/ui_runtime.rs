use std::collections::BTreeMap;

use thiserror::Error;

use crate::state::StoredValue;

/// Immutable story data exposed while a declarative UI component is evaluated.
#[derive(Clone, Debug, Default)]
pub struct UiContext {
    story: BTreeMap<String, StoredValue>,
}

impl UiContext {
    pub fn new(story: BTreeMap<String, StoredValue>) -> Self {
        Self { story }
    }

    pub fn story_value(&self, key: &str) -> Option<&StoredValue> {
        self.story.get(key)
    }

    pub(crate) fn expand(&self, input: &str) -> Result<String, UiContextError> {
        let mut output = String::new();
        let mut rest = input;
        while let Some(start) = rest.find("${") {
            output.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find('}').ok_or_else(|| {
                UiContextError::Template(format!("unterminated story binding in `{input}`"))
            })?;
            let key = &after[..end];
            let value = self.story_value(key).ok_or_else(|| {
                UiContextError::Template(format!("unknown story binding `{key}`"))
            })?;
            output.push_str(&display_value(value)?);
            rest = &after[end + 1..];
        }
        output.push_str(rest);
        Ok(output)
    }

    /// Expands values captured from the story while preserving unknown paths
    /// for the ECS-owned live signal resolver.
    pub(crate) fn expand_binding(&self, input: &str) -> Result<String, UiContextError> {
        let mut output = String::new();
        let mut rest = input;
        while let Some(start) = rest.find("${") {
            output.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find('}').ok_or_else(|| {
                UiContextError::Template(format!("unterminated UI binding in `{input}`"))
            })?;
            let key = &after[..end];
            if let Some(value) = self.story_value(key) {
                output.push_str(&display_value(value)?);
            } else {
                output.push_str("${");
                output.push_str(key);
                output.push('}');
            }
            rest = &after[end + 1..];
        }
        output.push_str(rest);
        Ok(output)
    }
}

fn display_value(value: &StoredValue) -> Result<String, UiContextError> {
    match value {
        StoredValue::Bool(value) => Ok(value.to_string()),
        StoredValue::Int(value) => Ok(value.to_string()),
        StoredValue::Float(value) => Ok(value.to_string()),
        StoredValue::String(value) => Ok(value.clone()),
        StoredValue::Array(_) | StoredValue::Map(_) => Err(UiContextError::Template(
            "array and map story values cannot be interpolated into UI text".to_string(),
        )),
    }
}

#[derive(Debug, Error)]
pub(crate) enum UiContextError {
    #[error("invalid UI template: {0}")]
    Template(String),
}

/// A semantic UI result. The story runtime, not the renderer, applies it.
#[derive(Clone, Debug, PartialEq)]
pub struct UiIntent {
    pub screen: String,
    pub value: StoredValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_read_only_story_values() {
        let context = UiContext::new(BTreeMap::from([(
            "playerName".to_string(),
            StoredValue::String("alice".to_string()),
        )]));

        assert_eq!(
            context.expand("Hello ${playerName}").expect("value exists"),
            "Hello alice"
        );
    }
}
