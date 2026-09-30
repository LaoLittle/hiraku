use crate::{Document, TextError};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range, sync::Arc};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TextValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    List(Vec<Arc<TextValue>>),
    Object(BTreeMap<String, Arc<TextValue>>),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextSnapshot {
    pub values: BTreeMap<String, Arc<TextValue>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectorStep {
    Field(String),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    pub root: String,
    pub steps: Vec<SelectorStep>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Segment {
    Markup(String),
    Selector(Selector),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    segments: Vec<Segment>,
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, TextError> {
        let mut segments = Vec::new();
        let mut markup = String::new();
        let mut offset = 0;
        while offset < source.len() {
            let rest = &source[offset..];
            if rest.starts_with("{{") || rest.starts_with("}}") {
                markup.push(rest.as_bytes()[0] as char);
                offset += 2;
            } else if rest.starts_with('{') {
                let end = rest.find('}').ok_or_else(|| {
                    TextError::new("unclosed text selector", offset..source.len())
                })?;
                if !markup.is_empty() {
                    segments.push(Segment::Markup(std::mem::take(&mut markup)));
                }
                segments.push(Segment::Selector(parse_selector(
                    &rest[1..end],
                    offset..offset + end + 1,
                )?));
                offset += end + 1;
            } else {
                let ch = rest.chars().next().expect("nonempty UTF-8 suffix");
                markup.push(ch);
                offset += ch.len_utf8();
            }
        }
        if !markup.is_empty() {
            segments.push(Segment::Markup(markup));
        }
        Ok(Self { segments })
    }

    pub fn selectors(&self) -> impl Iterator<Item = &Selector> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Selector(selector) => Some(selector),
            _ => None,
        })
    }

    pub fn render(&self, snapshot: &TextSnapshot) -> Result<Document, TextError> {
        let mut source = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Markup(markup) => source.push_str(markup),
                Segment::Selector(selector) => {
                    let value = snapshot.resolve(selector)?;
                    let text = match value {
                        TextValue::Null => {
                            return Err(TextError::new(
                                "text selector is null",
                                selector.range.clone(),
                            ));
                        }
                        TextValue::Bool(v) => v.to_string(),
                        TextValue::Int(v) => v.to_string(),
                        TextValue::UInt(v) => v.to_string(),
                        TextValue::Float(v) if v.is_finite() => v.to_string(),
                        TextValue::String(v) => v.clone(),
                        _ => {
                            return Err(TextError::new(
                                "text selector must resolve to a finite scalar value",
                                selector.range.clone(),
                            ));
                        }
                    };
                    source.push_str("#text(\"");
                    for ch in text.chars() {
                        match ch {
                            '\\' => source.push_str("\\\\"),
                            '"' => source.push_str("\\\""),
                            '\n' => source.push_str("\\n"),
                            '\r' => source.push_str("\\r"),
                            '\t' => source.push_str("\\t"),
                            _ => source.push(ch),
                        }
                    }
                    source.push_str("\")");
                }
            }
        }
        crate::parse(&source)
    }
}

impl TextSnapshot {
    pub fn resolve(&self, selector: &Selector) -> Result<&TextValue, TextError> {
        let missing = || {
            TextError::new(
                format!("unknown text selector rooted at `{}`", selector.root),
                selector.range.clone(),
            )
        };
        let mut value = self
            .values
            .get(&selector.root)
            .map(Arc::as_ref)
            .ok_or_else(missing)?;
        for step in &selector.steps {
            value = match (value, step) {
                (TextValue::Object(fields), SelectorStep::Field(name)) => {
                    fields.get(name).map(Arc::as_ref)
                }
                (TextValue::List(items), SelectorStep::Index(index)) => {
                    items.get(*index).map(Arc::as_ref)
                }
                _ => None,
            }
            .ok_or_else(missing)?;
        }
        Ok(value)
    }
}

fn parse_selector(path: &str, range: Range<usize>) -> Result<Selector, TextError> {
    let error = || {
        TextError::new(
            "text templates allow only variables, stored fields and literal list indices; calls and expressions are forbidden",
            range.clone(),
        )
    };
    let mut rest = path.trim();
    let root = take_ident(&mut rest).ok_or_else(error)?.to_string();
    let mut steps = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            rest = after;
            steps.push(SelectorStep::Field(
                take_ident(&mut rest).ok_or_else(error)?.to_string(),
            ));
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').ok_or_else(error)?;
            let digits = &after[..end];
            if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return Err(error());
            }
            steps.push(SelectorStep::Index(digits.parse().map_err(|_| error())?));
            rest = &after[end + 1..];
        } else {
            return Err(error());
        }
    }
    Ok(Selector { root, steps, range })
}

fn take_ident<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let end = rest
        .char_indices()
        .take_while(|(_, ch)| typst_syntax::is_id_continue(*ch))
        .map(|(i, ch)| i + ch.len_utf8())
        .last()?;
    let name = &rest[..end];
    if !typst_syntax::is_ident(name) {
        return None;
    }
    *rest = &rest[end..];
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_selectors_render_literals_and_preserve_style() {
        let scope = TextSnapshot {
            values: BTreeMap::from([
                (
                    "player".into(),
                    Arc::new(TextValue::Object(BTreeMap::from([(
                        "name".into(),
                        Arc::new(TextValue::String("#include evil~".into())),
                    )]))),
                ),
                (
                    "arr".into(),
                    Arc::new(TextValue::List(vec![Arc::new(TextValue::Int(7))])),
                ),
            ]),
        };
        let document = Template::parse("*{player.name}*: {arr[0]} {{literal}}")
            .expect("safe syntax")
            .render(&scope)
            .expect("safe data");
        assert_eq!(document.text, "#include evil~: 7 {literal}");
        assert!(document.styles[0].bold);
    }

    #[test]
    fn calls_operators_and_dynamic_indices_are_forbidden() {
        for source in [
            "{time()}",
            "{player.getName()}",
            "{arr[i]}",
            "{a + b}",
            "{name ?: fallback}",
        ] {
            assert!(Template::parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn copy_on_write_freezes_old_nested_values() {
        let old = Arc::new(TextSnapshot {
            values: BTreeMap::from([("name".into(), Arc::new(TextValue::String("Alice".into())))]),
        });
        let mut current = old.clone();
        let unchanged = current.values["name"].clone();
        let value = Arc::make_mut(&mut current)
            .values
            .get_mut("name")
            .expect("name");
        *Arc::make_mut(value) = TextValue::String("Bob".into());
        assert!(Arc::ptr_eq(&old.values["name"], &unchanged));
        let template = Template::parse("{name}").expect("selector");
        assert_eq!(template.render(&old).expect("old").text, "Alice");
        assert_eq!(template.render(&current).expect("new").text, "Bob");
    }
}
