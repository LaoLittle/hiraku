//! Engine-owned HKS story prelude schemas.
//!
//! `hiraku_script` intentionally knows nothing about camera, scene, UI, or ECS
//! concepts. This module validates engine capabilities before they become ECS
//! effects.

use hiraku_script::hks::{Argument, Expr, ExprKind, NumberUnit, Span};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionSpec {
    Preset(PresetPosition),
    Absolute { x: f32, y: f32 },
    RelativePercent { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetPosition {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraZoom {
    pub scale: f32,
    pub at: PositionSpec,
    pub duration: f32,
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    pub message: String,
    pub span: Span,
}

/// Resolves engine defaults for the story-prelude `camera.zoom` capability.
///
/// `scale` is positional. Optional arguments are named-only so additions do
/// not silently alter existing content.
pub fn normalize_camera_zoom(expression: &Expr) -> Result<CameraZoom, SchemaError> {
    let ExprKind::Call {
        callee, arguments, ..
    } = &expression.kind
    else {
        return Err(error(expression, "expected a camera.zoom call"));
    };
    let ExprKind::Member { object, name } = &callee.kind else {
        return Err(error(callee.as_ref(), "expected a camera.zoom call"));
    };
    if name != "zoom" || !matches!(&object.kind, ExprKind::Ident(name) if name == "camera") {
        return Err(error(callee.as_ref(), "expected a camera.zoom call"));
    }

    let mut scale = None;
    let mut at = None;
    let mut duration = None;
    let mut ease = None;
    for argument in arguments {
        match argument.label.as_deref() {
            None if scale.is_none() => scale = Some(number(argument, "scale")?),
            None => {
                return Err(error(
                    argument,
                    "camera.zoom accepts only one positional argument",
                ));
            }
            Some("at") if at.is_none() => at = Some(position(argument)?),
            Some("duration") if duration.is_none() => {
                duration = Some(number(argument, "duration")?)
            }
            Some("ease") if ease.is_none() => ease = Some(ease_value(argument)?),
            Some("at" | "duration" | "ease") => {
                return Err(error(
                    argument,
                    "camera.zoom argument was specified more than once",
                ));
            }
            Some(name) => {
                return Err(error(
                    argument,
                    format!("unknown camera.zoom argument `{name}`"),
                ));
            }
        }
    }

    let scale = scale.ok_or_else(|| error(expression, "camera.zoom requires a scale"))?;
    if scale <= 0.0 {
        return Err(error(expression, "camera.zoom scale must be positive"));
    }
    let duration = duration.unwrap_or(0.0);
    if duration < 0.0 {
        return Err(error(
            expression,
            "camera.zoom duration must not be negative",
        ));
    }
    Ok(CameraZoom {
        scale,
        at: at.unwrap_or(PositionSpec::Preset(PresetPosition::Center)),
        duration,
        ease: ease.unwrap_or(Ease::Linear),
    })
}

fn number(argument: &Argument, name: &str) -> Result<f32, SchemaError> {
    match &argument.value.kind {
        ExprKind::Number {
            value,
            unit: NumberUnit::Scalar,
        } => Ok(*value as f32),
        ExprKind::Number {
            unit: NumberUnit::Percent,
            ..
        } => Err(error(argument, format!("{name} cannot use percent units"))),
        _ => Err(error(argument, format!("{name} must be numeric"))),
    }
}

fn position(argument: &Argument) -> Result<PositionSpec, SchemaError> {
    match &argument.value.kind {
        ExprKind::Symbol(symbol) => match symbol.as_str() {
            "left" => Ok(PositionSpec::Preset(PresetPosition::Left)),
            "center" => Ok(PositionSpec::Preset(PresetPosition::Center)),
            "right" => Ok(PositionSpec::Preset(PresetPosition::Right)),
            _ => Err(error(
                argument,
                "position must be .left, .center, .right, or a tuple",
            )),
        },
        ExprKind::Tuple(values) if values.len() == 2 => {
            let (x, x_unit) = tuple_number(&values[0])?;
            let (y, y_unit) = tuple_number(&values[1])?;
            match (x_unit, y_unit) {
                (NumberUnit::Scalar, NumberUnit::Scalar) => Ok(PositionSpec::Absolute { x, y }),
                (NumberUnit::Percent, NumberUnit::Percent) => {
                    Ok(PositionSpec::RelativePercent { x, y })
                }
                _ => Err(error(
                    argument,
                    "position tuple cannot mix pixels and percent units",
                )),
            }
        }
        _ => Err(error(
            argument,
            "position must be .left, .center, .right, or a tuple",
        )),
    }
}

fn tuple_number(expression: &Expr) -> Result<(f32, NumberUnit), SchemaError> {
    match &expression.kind {
        ExprKind::Number { value, unit } => Ok((*value as f32, *unit)),
        ExprKind::UnaryMinus(expression) => match &expression.kind {
            ExprKind::Number { value, unit } => Ok((-(*value as f32), *unit)),
            _ => Err(error(
                expression.as_ref(),
                "position tuple members must be numeric",
            )),
        },
        _ => Err(error(expression, "position tuple members must be numeric")),
    }
}

fn ease_value(argument: &Argument) -> Result<Ease, SchemaError> {
    match &argument.value.kind {
        ExprKind::Symbol(symbol) => match symbol.as_str() {
            "linear" => Ok(Ease::Linear),
            "easeIn" => Ok(Ease::EaseIn),
            "easeOut" => Ok(Ease::EaseOut),
            "easeInOut" => Ok(Ease::EaseInOut),
            _ => Err(error(argument, "unsupported camera easing")),
        },
        _ => Err(error(argument, "ease must be a symbol literal")),
    }
}

trait Spanned {
    fn span(&self) -> &Span;
}

impl Spanned for Expr {
    fn span(&self) -> &Span {
        &self.span
    }
}

impl Spanned for Argument {
    fn span(&self) -> &Span {
        &self.span
    }
}

fn error(value: &impl Spanned, message: impl Into<String>) -> SchemaError {
    SchemaError {
        message: message.into(),
        span: value.span().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiraku_script::hks::{Stmt, parse_program};

    fn zoom(source: &str) -> Expr {
        let program = parse_program(source).unwrap();
        let [Stmt::Expr(expression)] = program.statements.as_slice() else {
            panic!("expected one expression");
        };
        expression.clone()
    }

    #[test]
    fn applies_camera_zoom_defaults() {
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, at: .center)")).unwrap(),
            CameraZoom {
                scale: 1.2,
                at: PositionSpec::Preset(PresetPosition::Center),
                duration: 0.0,
                ease: Ease::Linear,
            }
        );
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, duration: 1)")).unwrap(),
            CameraZoom {
                scale: 1.2,
                at: PositionSpec::Preset(PresetPosition::Center),
                duration: 1.0,
                ease: Ease::Linear,
            }
        );
    }

    #[test]
    fn normalizes_absolute_and_percent_positions() {
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, at: (12, 33))"))
                .unwrap()
                .at,
            PositionSpec::Absolute { x: 12.0, y: 33.0 }
        );
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, at: (20%, 30%))"))
                .unwrap()
                .at,
            PositionSpec::RelativePercent { x: 20.0, y: 30.0 }
        );
    }
}
