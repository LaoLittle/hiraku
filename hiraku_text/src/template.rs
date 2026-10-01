use crate::{Document, TextError};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range, sync::Arc};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TextValue {
    Opaque,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalizableText {
    pub key: Option<String>,
    pub source: String,
    pub context: Arc<TextSnapshot>,
}

impl LocalizableText {
    pub fn render_with(
        &self,
        translate: impl FnOnce(Option<&str>, &str) -> Result<String, TextError>,
    ) -> Result<Document, TextError> {
        Template::parse(&translate(self.key.as_deref(), &self.source)?)?.render(&self.context)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectorStep {
    Field(String),
    Index(usize),
    IndexSelector(Box<Selector>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    pub root: String,
    pub steps: Vec<SelectorStep>,
    pub range: Range<usize>,
}

impl Selector {
    pub fn roots(&self) -> Vec<&str> {
        let mut roots = vec![self.root.as_str()];
        for step in &self.steps {
            if let SelectorStep::IndexSelector(selector) = step {
                roots.extend(selector.roots());
            }
        }
        roots
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    source: String,
    selectors: Vec<Selector>,
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, TextError> {
        Ok(Self {
            source: source.to_owned(),
            selectors: crate::parser::selectors(source)?,
        })
    }

    pub fn selectors(&self) -> impl Iterator<Item = &Selector> {
        self.selectors.iter()
    }

    pub fn render(&self, snapshot: &TextSnapshot) -> Result<Document, TextError> {
        crate::parse_with_snapshot(&self.source, snapshot)
    }
}

pub(crate) enum Segment {
    Markup(String),
    Selector(Selector),
}

pub(crate) fn segments(source: &str, base: usize) -> Result<Vec<Segment>, TextError> {
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
                TextError::new("unclosed text selector", base + offset..base + source.len())
            })?;
            if !markup.is_empty() {
                segments.push(Segment::Markup(std::mem::take(&mut markup)));
            }
            segments.push(Segment::Selector(parse_selector(
                &rest[1..end],
                base + offset..base + offset + end + 1,
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
    Ok(segments)
}

pub(crate) fn scalar(value: &TextValue, range: Range<usize>) -> Result<String, TextError> {
    let text = match value {
        TextValue::Bool(v) => v.to_string(),
        TextValue::Int(v) => v.to_string(),
        TextValue::UInt(v) => v.to_string(),
        TextValue::Float(v) if v.is_finite() => v.to_string(),
        TextValue::String(v) => v.clone(),
        TextValue::Null => return Err(TextError::new("text selector is null", range)),
        _ => {
            return Err(TextError::new(
                "text selector must resolve to a finite scalar value",
                range,
            ));
        }
    };
    if text.len() > 64 * 1024 {
        return Err(TextError::new(
            "text selector exceeds the 64 KiB limit",
            range,
        ));
    }
    Ok(text)
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
                (TextValue::List(items), SelectorStep::IndexSelector(selector)) => {
                    let index = match self.resolve(selector)? {
                        TextValue::Int(index) => usize::try_from(*index).ok(),
                        TextValue::UInt(index) => usize::try_from(*index).ok(),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        TextError::new(
                            "text list index must be a nonnegative integer",
                            selector.range.clone(),
                        )
                    })?;
                    items.get(index).map(Arc::as_ref)
                }
                _ => None,
            }
            .ok_or_else(missing)?;
        }
        Ok(value)
    }
}

pub(crate) fn parse_selector(path: &str, range: Range<usize>) -> Result<Selector, TextError> {
    parse_selector_depth(path, range, 0)
}

fn parse_selector_depth(
    path: &str,
    range: Range<usize>,
    depth: usize,
) -> Result<Selector, TextError> {
    let error = || {
        TextError::new(
            "text templates allow only variables, stored fields and list indices read from the snapshot; calls and expressions are forbidden",
            range.clone(),
        )
    };
    if depth > 32 {
        return Err(error());
    }
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
            let mut nesting = 0;
            let end = after
                .char_indices()
                .find_map(|(index, ch)| match ch {
                    '[' => {
                        nesting += 1;
                        None
                    }
                    ']' if nesting == 0 => Some(index),
                    ']' => {
                        nesting -= 1;
                        None
                    }
                    _ => None,
                })
                .ok_or_else(error)?;
            let index = after[..end].trim();
            if !index.is_empty() && index.bytes().all(|c| c.is_ascii_digit()) {
                steps.push(SelectorStep::Index(index.parse().map_err(|_| error())?));
            } else {
                steps.push(SelectorStep::IndexSelector(Box::new(parse_selector_depth(
                    index,
                    range.clone(),
                    depth + 1,
                )?)));
            }
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
    fn variable_indices_read_the_same_immutable_snapshot() {
        let snapshot = TextSnapshot {
            values: BTreeMap::from([
                (
                    "a".into(),
                    Arc::new(TextValue::List(vec![
                        Arc::new(TextValue::String("Alice".into())),
                        Arc::new(TextValue::String("Bob".into())),
                    ])),
                ),
                ("i".into(), Arc::new(TextValue::Int(0))),
                (
                    "indices".into(),
                    Arc::new(TextValue::List(vec![Arc::new(TextValue::UInt(1))])),
                ),
            ]),
        };
        let template = Template::parse("#let name = {a[indices[i]]};#ruby(\"{a[i]}\")[#name]")
            .expect("snapshot indices");
        let document = template.render(&snapshot).expect("immutable values");
        assert_eq!(document.text, "Bob");
        assert_eq!(document.ruby[0].reading, "Alice");
        let roots: Vec<_> = template.selectors().flat_map(Selector::roots).collect();
        assert!(roots.contains(&"i") && roots.contains(&"indices") && roots.contains(&"a"));
    }

    #[test]
    fn variable_indices_reject_invalid_types_and_bounds() {
        let template = Template::parse("{a[i]}").expect("selector");
        for index in [
            TextValue::Int(-1),
            TextValue::Int(1),
            TextValue::Float(0.0),
            TextValue::Bool(false),
        ] {
            let snapshot = TextSnapshot {
                values: BTreeMap::from([
                    (
                        "a".into(),
                        Arc::new(TextValue::List(vec![Arc::new(TextValue::Int(7))])),
                    ),
                    ("i".into(), Arc::new(index)),
                ]),
            };
            assert!(template.render(&snapshot).is_err());
        }
    }

    #[test]
    fn typst_constants_receive_data_without_source_injection() {
        let payload = "\"); #include(\"evil\"); {missing} *bold*";
        let snapshot = TextSnapshot {
            values: BTreeMap::from([
                ("a".into(), Arc::new(TextValue::Int(1))),
                ("name".into(), Arc::new(TextValue::String(payload.into()))),
            ]),
        };
        let document = Template::parse("#let b = {a}; It's #b #let c = {name};#c")
            .expect("template")
            .render(&snapshot)
            .expect("data is not code");
        assert_eq!(document.text, format!(" It's 1 {payload}"));
        assert!(document.styles.iter().all(|style| !style.bold));
        assert_eq!(
            crate::parse(&document.to_markup()).expect("canonical markup"),
            document
        );
        assert_eq!(
            Template::parse(&document.to_markup())
                .expect("canonical template")
                .render(&TextSnapshot::default())
                .expect("literal braces must not become selectors again"),
            document
        );
    }

    #[test]
    fn ruby_arguments_and_content_interpolate_as_plain_data() {
        let snapshot = TextSnapshot {
            values: BTreeMap::from([("a".into(), Arc::new(TextValue::Int(2)))]),
        };
        let document = Template::parse("#ruby(\"{a}\")[Iter {a}]")
            .expect("template")
            .render(&snapshot)
            .expect("ruby data");
        assert_eq!(document.text, "Iter 2");
        assert_eq!(document.ruby[0].reading, "2");
        assert_eq!(
            crate::parse(&document.to_markup()).expect("canonical ruby"),
            document
        );
    }

    #[test]
    fn typst_code_blocks_cannot_execute_script() {
        for source in [
            "#let b = {time()};#b",
            "#let b = {a; panic()};#b",
            "#ruby(\"{a()}\")[Alice]",
        ] {
            assert!(Template::parse(source).is_err(), "{source}");
        }
    }

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
    fn calls_operators_and_effectful_indices_are_forbidden() {
        for source in [
            "{time()}",
            "{player.getName()}",
            "{arr[next()]}",
            "{arr[i + 1]}",
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
