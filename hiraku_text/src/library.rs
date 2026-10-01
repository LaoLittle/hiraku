use std::sync::LazyLock;

use typst_library::{
    diag::{StrResult, bail},
    foundations::{Content, NativeElement, Scope, Str, Value, elem, func},
    model::{EmphElem, StrongElem},
    text::{LinebreakElem, StrikeElem, TextElem, UnderlineElem},
};

#[elem]
pub(crate) struct RubyElem {
    #[required]
    pub reading: Str,
    #[required]
    pub body: Content,
}

#[elem]
pub(crate) struct ColorElem {
    #[required]
    pub rgba: Str,
    #[required]
    pub body: Content,
}

pub(crate) static SCOPE: LazyLock<Scope> = LazyLock::new(|| {
    let mut scope = Scope::deduplicating();
    scope.define_func::<text>();
    scope.define_func::<ruby>();
    scope.define_func::<color>();
    scope.define_func::<strong>();
    scope.define_func::<emph>();
    scope.define_func::<strike>();
    scope.define_func::<underline>();
    scope.define_func::<linebreak>();
    scope.define("br", LinebreakElem::new().pack());
    scope
});

pub(crate) fn contains(name: &str) -> bool {
    SCOPE.get(name).is_some()
}

fn content(value: Value) -> StrResult<Content> {
    let text = match value {
        Value::Content(content) => return Ok(content),
        Value::Str(text) => text.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => bail!("text requires content or a finite scalar value"),
    };
    if text.len() > 64 * 1024 {
        bail!("text exceeds the 64 KiB limit");
    }
    Ok(TextElem::new(text.into()).pack())
}

#[func]
fn text(value: Value) -> StrResult<Content> {
    content(value)
}

#[func]
fn ruby(reading: Str, body: Value) -> StrResult<Content> {
    if reading.is_empty() || reading.as_str().contains(['\n', '\r']) {
        bail!("ruby requires a nonempty single-line reading");
    }
    Ok(RubyElem::new(reading, content(body)?).pack())
}

#[func]
fn color(rgba: Str, body: Value) -> StrResult<Content> {
    let hex = rgba.strip_prefix('#').unwrap_or("");
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("color requires #RRGGBB or #RRGGBBAA");
    }
    Ok(ColorElem::new(rgba, content(body)?).pack())
}

#[func]
fn strong(body: Value) -> StrResult<Content> {
    Ok(StrongElem::new(content(body)?).pack())
}

#[func]
fn emph(body: Value) -> StrResult<Content> {
    Ok(EmphElem::new(content(body)?).pack())
}

#[func]
fn strike(body: Value) -> StrResult<Content> {
    Ok(StrikeElem::new(content(body)?).pack())
}

#[func]
fn underline(body: Value) -> StrResult<Content> {
    Ok(UnderlineElem::new(content(body)?).pack())
}

#[func]
fn linebreak() -> Content {
    LinebreakElem::new().pack()
}
