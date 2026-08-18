use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use hiraku_script::hks::{Argument, Block, Expr, ExprKind, NumberUnit, Stmt, parse_program};

use crate::{
    state::StoredValue,
    texture::TextureCatalog,
    ui::{
        BarNode, ButtonNode, ContainerNode, ScreenImageButtonNode, ScreenImageNode, ScreenLayout,
        ScreenNode, ScreenSpec, ScreenTexture, SpacerNode, TextNode,
    },
};

/// Immutable story data exposed while a declarative UI document is rendered.
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

    fn expand(&self, input: &str) -> Result<String, UiScriptError> {
        let mut output = String::new();
        let mut rest = input;
        while let Some(start) = rest.find("${") {
            output.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find('}').ok_or_else(|| {
                UiScriptError::InvalidScreen(format!("unterminated story binding in `{input}`"))
            })?;
            let key = &after[..end];
            let value = self.story_value(key).ok_or_else(|| {
                UiScriptError::InvalidScreen(format!("unknown story binding `{key}`"))
            })?;
            output.push_str(&display_value(value)?);
            rest = &after[end + 1..];
        }
        output.push_str(rest);
        Ok(output)
    }
}

fn display_value(value: &StoredValue) -> Result<String, UiScriptError> {
    match value {
        StoredValue::Bool(value) => Ok(value.to_string()),
        StoredValue::Int(value) => Ok(value.to_string()),
        StoredValue::Float(value) => Ok(value.to_string()),
        StoredValue::String(value) => Ok(value.clone()),
        StoredValue::Array(_) | StoredValue::Map(_) => Err(UiScriptError::InvalidScreen(
            "array and map story values cannot be interpolated into UI text".to_string(),
        )),
    }
}

/// A semantic UI result. The story runtime, not the UI renderer, applies it.
#[derive(Clone, Debug, PartialEq)]
pub struct UiIntent {
    pub screen: String,
    pub value: StoredValue,
}

#[derive(Debug, Error)]
pub enum UiScriptError {
    #[error("failed to evaluate UI script: {0}")]
    Evaluation(String),
    #[error("invalid UI screen: {0}")]
    InvalidScreen(String),
}

/// Lowers a small Compose-style `.ui.hks` call tree into an engine-owned screen tree.
///
/// This is deliberately an embedding compiler, not a generic HKS VM feature. UI documents can
/// only call the builders declared below and their arguments remain declarative literals. Strings
/// may read immutable context values with `${camelCaseName}`.
pub fn evaluate_ui_script(
    source: &str,
    context: &UiContext,
    textures: &TextureCatalog,
) -> Result<ScreenSpec, UiScriptError> {
    let program = parse_program(source).map_err(|errors| {
        UiScriptError::Evaluation(
            errors
                .into_iter()
                .map(|error| {
                    format!(
                        "{} at {}..{}",
                        error.message, error.span.start, error.span.end
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    if program.statements.len() != 1 {
        return Err(invalid(
            "a UI document must contain exactly one `screen { ... }` root",
        ));
    }
    let Stmt::Expr(root) = &program.statements[0] else {
        return Err(invalid("a UI document root must be `screen { ... }`"));
    };
    let screen = lower_builder(root, true)?;
    let screen = screen
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("`screen` did not produce a screen object"))?;
    parse_screen(screen, context, textures)
}

fn lower_builder(expression: &Expr, root: bool) -> Result<Value, UiScriptError> {
    let ExprKind::Call {
        callee,
        arguments,
        trailing_block,
    } = &expression.kind
    else {
        return Err(invalid("UI children must be declarative builder calls"));
    };
    let ExprKind::Ident(name) = &callee.kind else {
        return Err(invalid("UI builders must be unqualified names"));
    };
    if root && name != "screen" {
        return Err(invalid("the UI document root must be `screen { ... }`"));
    }
    if !root && name == "screen" {
        return Err(invalid("`screen` can only appear at the document root"));
    }

    let positional_key = match name.as_str() {
        "screen" | "row" | "column" | "frame" | "bar" | "spacer" => None,
        "text" | "button" => Some("text"),
        "image" | "imageButton" => Some("texture"),
        _ => return Err(invalid(format!("unknown UI builder `{name}`"))),
    };
    let mut object = Map::new();
    if name != "screen" {
        object.insert("type".to_string(), Value::String(name.clone()));
    }
    lower_arguments(name, arguments, positional_key, &mut object)?;

    match (name.as_str(), trailing_block) {
        ("screen" | "row" | "column" | "frame", Some(block)) => {
            object.insert("children".to_string(), lower_children(block)?);
        }
        ("screen" | "row" | "column" | "frame", None) => {
            object.insert("children".to_string(), Value::Array(Vec::new()));
        }
        (_, Some(_)) => {
            return Err(invalid(format!("`{name}` does not accept a child block")));
        }
        (_, None) => {}
    }
    Ok(Value::Object(object))
}

fn lower_arguments(
    builder: &str,
    arguments: &[Argument],
    positional_key: Option<&str>,
    output: &mut Map<String, Value>,
) -> Result<(), UiScriptError> {
    let mut used_positional = false;
    for argument in arguments {
        let key = match argument.label.as_deref() {
            Some(label) => label,
            None if !used_positional => {
                used_positional = true;
                positional_key
                    .ok_or_else(|| invalid(format!("`{builder}` accepts only named arguments")))?
            }
            None => {
                return Err(invalid(format!(
                    "`{builder}` accepts at most one positional argument"
                )));
            }
        };
        if output.contains_key(key) {
            return Err(invalid(format!(
                "duplicate `{key}` argument on `{builder}`"
            )));
        }
        output.insert(key.to_string(), lower_literal(&argument.value)?);
    }
    Ok(())
}

fn lower_children(block: &Block) -> Result<Value, UiScriptError> {
    block
        .statements
        .iter()
        .map(|statement| match statement {
            Stmt::Expr(expression) => lower_builder(expression, false),
            _ => Err(invalid("UI blocks may contain only builder calls")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn lower_literal(expression: &Expr) -> Result<Value, UiScriptError> {
    match &expression.kind {
        ExprKind::Bool(value) => Ok(Value::Bool(*value)),
        ExprKind::Number { value, unit } => {
            if *unit == NumberUnit::Percent {
                return Err(invalid(
                    "percent literals are not accepted here; use a camelCase `*Percent` argument",
                ));
            }
            serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .ok_or_else(|| invalid("UI numbers must be finite"))
        }
        ExprKind::String(value) | ExprKind::Symbol(value) => Ok(Value::String(value.clone())),
        ExprKind::UnaryMinus(value) => {
            let Value::Number(value) = lower_literal(value)? else {
                return Err(invalid("unary minus requires a numeric UI literal"));
            };
            let value = value
                .as_f64()
                .and_then(|value| serde_json::Number::from_f64(-value))
                .ok_or_else(|| invalid("UI numbers must be finite"))?;
            Ok(Value::Number(value))
        }
        ExprKind::Tuple(values) => values
            .iter()
            .map(lower_literal)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        ExprKind::Map(fields) => fields
            .iter()
            .map(|field| Ok((field.name.clone(), lower_literal(&field.value)?)))
            .collect::<Result<Map<_, _>, UiScriptError>>()
            .map(Value::Object),
        _ => Err(invalid(
            "UI arguments may contain only literals, symbols, tuples, and maps",
        )),
    }
}

fn parse_screen(
    mut screen: Map<String, Value>,
    context: &UiContext,
    textures: &TextureCatalog,
) -> Result<ScreenSpec, UiScriptError> {
    let title = optional_string(&mut screen, "title", context)?;
    let panel = optional_bool(&mut screen, "panel")?.unwrap_or(true);
    let width = optional_number(&mut screen, "width")?;
    let background_texture = optional_string(&mut screen, "backgroundTexture", context)?
        .map(|name| resolve_texture(textures, &name))
        .transpose()?;
    let xalign = optional_number(&mut screen, "xalign")?.unwrap_or(0.5);
    let yalign = optional_number(&mut screen, "yalign")?.unwrap_or(0.5);
    let padding = optional_number(&mut screen, "padding")?.unwrap_or(24.0);
    let gap = optional_number(&mut screen, "gap")?.unwrap_or(16.0);
    let overlay = optional_rgba(&mut screen, "overlay")?;
    let background = optional_rgba(&mut screen, "background")?;
    let border = optional_rgba(&mut screen, "border")?;
    let children = required_tuple(&mut screen, "children")?
        .into_iter()
        .map(|value| parse_node(value, context, textures))
        .collect::<Result<Vec<_>, _>>()?;
    no_unknown("screen", &screen)?;
    Ok(ScreenSpec {
        title,
        panel,
        width,
        background_texture,
        xalign,
        yalign,
        padding,
        gap,
        overlay,
        background,
        border,
        children,
    })
}

fn parse_node(
    value: Value,
    context: &UiContext,
    textures: &TextureCatalog,
) -> Result<ScreenNode, UiScriptError> {
    let mut node = value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("screen nodes must be maps"))?;
    let kind = required_string(&mut node, "type", context)?;
    match kind.as_str() {
        "text" => {
            let text = required_string(&mut node, "text", context)?;
            let size = optional_number(&mut node, "size")?.unwrap_or(26.0);
            let color = optional_rgba(&mut node, "color")?;
            let align = optional_number(&mut node, "align")?;
            let layout = take_layout(&mut node)?;
            no_unknown("text", &node)?;
            Ok(ScreenNode::Text(TextNode {
                text,
                size,
                color,
                align,
                layout,
            }))
        }
        "button" => {
            let text = required_string(&mut node, "text", context)?;
            let value = node.remove("value").map(stored_value).transpose()?;
            let action = optional_string(&mut node, "action", context)?;
            if value.is_none() && action.is_none() {
                return Err(invalid("button requires `value` or `action`"));
            }
            let enabled = optional_bool(&mut node, "enabled")?.unwrap_or(true);
            let size = optional_number(&mut node, "size")?.unwrap_or(28.0);
            let color = optional_rgba(&mut node, "color")?;
            let hovered_color = optional_rgba(&mut node, "hoveredColor")?;
            let pressed_color = optional_rgba(&mut node, "pressedColor")?;
            let insensitive_color = optional_rgba(&mut node, "insensitiveColor")?;
            let background = optional_rgba(&mut node, "background")?;
            let border = optional_rgba(&mut node, "border")?;
            let hovered_background = optional_rgba(&mut node, "hoveredBackground")?;
            let pressed_background = optional_rgba(&mut node, "pressedBackground")?;
            let align = optional_number(&mut node, "align")?;
            let uniform_padding = optional_number(&mut node, "padding")?;
            let padding_x = optional_number(&mut node, "paddingX")?.or(uniform_padding);
            let padding_y = optional_number(&mut node, "paddingY")?.or(uniform_padding);
            let border_width = optional_number(&mut node, "borderWidth")?;
            let radius = optional_number(&mut node, "radius")?;
            let layout = take_layout(&mut node)?;
            no_unknown("button", &node)?;
            Ok(ScreenNode::Button(ButtonNode {
                text,
                value,
                action,
                enabled,
                size,
                color,
                hovered_color,
                pressed_color,
                insensitive_color,
                background,
                border,
                hovered_background,
                pressed_background,
                align,
                padding_x,
                padding_y,
                border_width,
                radius,
                layout,
            }))
        }
        "image" => {
            let texture = required_string(&mut node, "texture", context)?;
            let layout = take_layout(&mut node)?;
            no_unknown("image", &node)?;
            Ok(ScreenNode::Image(ScreenImageNode {
                texture: resolve_texture(textures, &texture)?,
                layout,
            }))
        }
        "imageButton" => {
            let texture = required_string(&mut node, "texture", context)?;
            let hovered_texture = optional_string(&mut node, "hoveredTexture", context)?
                .map(|name| resolve_texture(textures, &name))
                .transpose()?;
            let hovered_layout = optional_layout(&mut node, "hoveredLayout")?;
            let value = node
                .remove("value")
                .map(stored_value)
                .transpose()?
                .ok_or_else(|| invalid("imageButton requires `value`"))?;
            let enabled = optional_bool(&mut node, "enabled")?.unwrap_or(true);
            let hovered_when_disabled =
                optional_bool(&mut node, "hoveredWhenDisabled")?.unwrap_or(false);
            let layout = take_layout(&mut node)?;
            no_unknown("imageButton", &node)?;
            Ok(ScreenNode::ImageButton(ScreenImageButtonNode {
                texture: resolve_texture(textures, &texture)?,
                hovered_texture,
                hovered_layout,
                value,
                enabled,
                hovered_when_disabled,
                layout,
            }))
        }
        "bar" => {
            let value = optional_number(&mut node, "value")?.unwrap_or(0.0);
            let min = optional_number(&mut node, "min")?.unwrap_or(0.0);
            let max = optional_number(&mut node, "max")?.unwrap_or(1.0);
            let width = optional_number(&mut node, "width")?.unwrap_or(320.0);
            let height = optional_number(&mut node, "height")?.unwrap_or(18.0);
            let background = optional_rgba(&mut node, "background")?;
            let fill = optional_rgba(&mut node, "fill")?;
            let border = optional_rgba(&mut node, "border")?;
            no_unknown("bar", &node)?;
            Ok(ScreenNode::Bar(BarNode {
                value,
                min,
                max,
                width,
                height,
                background,
                fill,
                border,
            }))
        }
        "column" | "row" | "frame" => {
            let gap = optional_number(&mut node, "gap")?.unwrap_or(12.0);
            let padding = optional_number(&mut node, "padding")?.unwrap_or(0.0);
            let background = optional_rgba(&mut node, "background")?;
            let border = optional_rgba(&mut node, "border")?;
            let justify = optional_string(&mut node, "justify", context)?;
            let align_items = optional_string(&mut node, "alignItems", context)?;
            let layout = take_layout(&mut node)?;
            let children = required_tuple(&mut node, "children")?
                .into_iter()
                .map(|child| parse_node(child, context, textures))
                .collect::<Result<Vec<_>, _>>()?;
            no_unknown(&kind, &node)?;
            let container = ContainerNode {
                gap,
                padding,
                background,
                border,
                justify,
                align_items,
                layout,
                children,
            };
            if kind == "row" {
                Ok(ScreenNode::Row(container))
            } else {
                Ok(ScreenNode::Column(container))
            }
        }
        "spacer" => {
            let width = optional_number(&mut node, "width")?.unwrap_or(0.0);
            let height = optional_number(&mut node, "height")?.unwrap_or(0.0);
            no_unknown("spacer", &node)?;
            Ok(ScreenNode::Spacer(SpacerNode { width, height }))
        }
        _ => Err(invalid(format!("unknown screen node type `{kind}`"))),
    }
}

fn resolve_texture(catalog: &TextureCatalog, name: &str) -> Result<ScreenTexture, UiScriptError> {
    let texture = catalog
        .resolve(name)
        .ok_or_else(|| invalid(format!("texture `{name}` is not defined")))?;
    Ok(ScreenTexture {
        path: texture.path.clone(),
        rect: texture.rect,
    })
}

fn stored_value(value: Value) -> Result<StoredValue, UiScriptError> {
    match value {
        Value::Bool(value) => Ok(StoredValue::Bool(value)),
        Value::Number(value) if value.is_i64() => Ok(StoredValue::Int(value.as_i64().unwrap())),
        Value::Number(value) => value
            .as_f64()
            .map(StoredValue::Float)
            .ok_or_else(|| invalid("invalid numeric UI value")),
        Value::String(value) => Ok(StoredValue::String(value)),
        Value::Array(values) => values
            .into_iter()
            .map(stored_value)
            .collect::<Result<Vec<_>, _>>()
            .map(StoredValue::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key, stored_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, UiScriptError>>()
            .map(StoredValue::Map),
        Value::Null => Err(invalid("null is not a storable UI value")),
    }
}

fn take_layout(map: &mut Map<String, Value>) -> Result<ScreenLayout, UiScriptError> {
    Ok(ScreenLayout {
        width: optional_number(map, "width")?,
        height: optional_number(map, "height")?,
        width_percent: optional_number(map, "widthPercent")?,
        height_percent: optional_number(map, "heightPercent")?,
        min_width: optional_number(map, "minWidth")?,
        left: optional_number(map, "left")?,
        left_percent: optional_number(map, "leftPercent")?,
        right: optional_number(map, "right")?,
        right_percent: optional_number(map, "rightPercent")?,
        top: optional_number(map, "top")?,
        top_percent: optional_number(map, "topPercent")?,
        bottom: optional_number(map, "bottom")?,
        bottom_percent: optional_number(map, "bottomPercent")?,
    })
}

fn optional_layout(
    map: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<ScreenLayout>, UiScriptError> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    let mut value = value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid(format!("`{key}` must be a map")))?;
    let layout = take_layout(&mut value)?;
    no_unknown(key, &value)?;
    Ok(Some(layout))
}

fn required_tuple(map: &mut Map<String, Value>, key: &str) -> Result<Vec<Value>, UiScriptError> {
    map.remove(key)
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| invalid(format!("`{key}` must be a tuple")))
}

fn required_string(
    map: &mut Map<String, Value>,
    key: &str,
    context: &UiContext,
) -> Result<String, UiScriptError> {
    optional_string(map, key, context)?.ok_or_else(|| invalid(format!("`{key}` must be a string")))
}

fn optional_string(
    map: &mut Map<String, Value>,
    key: &str,
    context: &UiContext,
) -> Result<Option<String>, UiScriptError> {
    map.remove(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid(format!("`{key}` must be a string")))
                .and_then(|value| context.expand(&value))
        })
        .transpose()
}

fn optional_number(map: &mut Map<String, Value>, key: &str) -> Result<Option<f32>, UiScriptError> {
    map.remove(key)
        .map(|value| {
            value
                .as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| invalid(format!("`{key}` must be numeric")))
        })
        .transpose()
}

fn optional_bool(map: &mut Map<String, Value>, key: &str) -> Result<Option<bool>, UiScriptError> {
    map.remove(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| invalid(format!("`{key}` must be boolean")))
        })
        .transpose()
}

fn optional_rgba(
    map: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<[f32; 4]>, UiScriptError> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("`{key}` must be a four-number tuple")))?;
    if values.len() != 4 {
        return Err(invalid(format!("`{key}` must contain four numbers")));
    }
    let mut rgba = [0.0; 4];
    for (index, value) in values.iter().enumerate() {
        rgba[index] = value
            .as_f64()
            .ok_or_else(|| invalid(format!("`{key}` must contain only numbers")))?
            as f32;
    }
    Ok(Some(rgba))
}

fn no_unknown(kind: &str, map: &Map<String, Value>) -> Result<(), UiScriptError> {
    if map.is_empty() {
        return Ok(());
    }
    Err(invalid(format!(
        "unknown {kind} option(s): {}",
        map.keys().cloned().collect::<Vec<_>>().join(", ")
    )))
}

fn invalid(message: impl Into<String>) -> UiScriptError {
    UiScriptError::InvalidScreen(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_read_only_story_values() {
        let context = UiContext::new(BTreeMap::from([(
            "bgmVolume".to_string(),
            StoredValue::Float(0.8),
        )]));
        let screen = evaluate_ui_script(
            r#"screen(title: "Settings (BGM: ${bgmVolume})") {
                button("Back", value: "back")
                spacer()
            }"#,
            &context,
            &TextureCatalog::default(),
        )
        .unwrap();
        assert_eq!(screen.title.as_deref(), Some("Settings (BGM: 0.8)"));
        assert_eq!(
            context.story_value("bgmVolume"),
            Some(&StoredValue::Float(0.8))
        );
    }

    #[test]
    fn rejects_unknown_calls_in_ui_documents() {
        let error = evaluate_ui_script(
            r#"screen { launchMissiles() }"#,
            &UiContext::default(),
            &TextureCatalog::default(),
        )
        .unwrap_err();
        assert!(matches!(error, UiScriptError::InvalidScreen(_)));
    }

    #[test]
    fn lowers_nested_compose_builders() {
        let screen = evaluate_ui_script(
            r#"screen(panel: true) {
                column(gap: 8) {
                    text("Title")
                    row { button("OK", value: .confirm) }
                }
            }"#,
            &UiContext::default(),
            &TextureCatalog::default(),
        )
        .unwrap();
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("expected a column");
        };
        assert_eq!(column.children.len(), 2);
        assert!(matches!(column.children[1], ScreenNode::Row(_)));
    }

    #[test]
    fn parses_the_settings_ui() {
        let context = UiContext::new(BTreeMap::from([(
            "bgmVolume".to_string(),
            StoredValue::Float(0.8),
        )]));
        let source =
            include_str!("../../../../manosabars/assets/main_hdp_contents/ui/settings.ui.hks");
        let screen = evaluate_ui_script(source, &context, &TextureCatalog::default()).unwrap();
        assert_eq!(screen.title.as_deref(), Some("Settings (BGM: 0.8)"));
    }
}
