//! Engine-owned HKS story prelude schemas.
//!
//! `hiraku_script` intentionally knows nothing about camera, scene, UI, or ECS
//! concepts. This module validates engine capabilities before they become ECS
//! effects.

use hiraku_script::hks::{Argument, Expr, ExprKind, NumberUnit, Span, Stmt, parse_program};
use thiserror::Error;

use crate::script::{IrCommand, IrInstruction, IrProgram, IrWaitKind};

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

/// Compiles the capability-approved HKS story subset into the existing
/// deterministic IR runtime during the migration away from Rhai.
pub fn compile_story_to_ir(path: &str, source: &str) -> Result<IrProgram, HksStoryCompileError> {
    let program = parse_program(source).map_err(|errors| HksStoryCompileError::Parse {
        path: path.to_string(),
        message: errors
            .into_iter()
            .map(|error| format!("{} at byte {}", error.message, error.span.start))
            .collect::<Vec<_>>()
            .join("; "),
    })?;
    let mut lowerer = HksStoryLowerer {
        path,
        instructions: Vec::new(),
    };
    for statement in &program.statements {
        lowerer.statement(statement)?;
    }
    lowerer.instructions.push(IrInstruction::Halt);
    Ok(IrProgram::new(source_hash(source), lowerer.instructions))
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HksStoryCompileError {
    #[error("failed to parse `{path}`: {message}")]
    Parse { path: String, message: String },
    #[error("unsupported HKS story statement in `{path}` at byte {offset}")]
    UnsupportedStatement { path: String, offset: usize },
    #[error("unsupported HKS story call `{name}` in `{path}` at byte {offset}")]
    UnsupportedCall {
        path: String,
        name: String,
        offset: usize,
    },
    #[error("invalid HKS story call `{name}` in `{path}` at byte {offset}: {message}")]
    InvalidCall {
        path: String,
        name: String,
        offset: usize,
        message: String,
    },
}

struct HksStoryLowerer<'a> {
    path: &'a str,
    instructions: Vec<IrInstruction>,
}

impl HksStoryLowerer<'_> {
    fn statement(&mut self, statement: &Stmt) -> Result<(), HksStoryCompileError> {
        let Stmt::Expr(expression) = statement else {
            return Err(HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: statement_span(statement).start,
            });
        };
        let ExprKind::Call {
            callee,
            arguments,
            trailing_block,
        } = &expression.kind
        else {
            return Err(HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: expression.span.start,
            });
        };
        if let Some(command) = self.character_call(expression)? {
            self.instructions.push(IrInstruction::Emit(command));
            return Ok(());
        }
        if trailing_block.is_some() {
            return Err(HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: expression.span.start,
            });
        }
        let name =
            flatten_callee(callee).ok_or_else(|| HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: callee.span.start,
            })?;
        if arguments.iter().any(|argument| argument.label.is_some()) {
            return Err(self.invalid_call(
                &name,
                expression,
                "named arguments are not supported by this command",
            ));
        }
        match name.as_str() {
            "clear_text" => {
                self.no_arguments(&name, expression, arguments, IrCommand::ClearDialogue)
            }
            "stop_bgm" => self.no_arguments(&name, expression, arguments, IrCommand::StopBgm),
            "return_to_title" => {
                self.no_arguments(&name, expression, arguments, IrCommand::ReturnToTitle)
            }
            "log" => self.one_string(
                &name,
                expression,
                arguments,
                |message| IrCommand::Log(message),
                None,
            ),
            "bg" => self.one_string(
                &name,
                expression,
                arguments,
                |texture| IrCommand::SetBackground { texture },
                None,
            ),
            "load_script" => self.one_string(
                &name,
                expression,
                arguments,
                |path| IrCommand::LoadScript { path },
                None,
            ),
            "narrate" => self.one_string(
                &name,
                expression,
                arguments,
                |text| IrCommand::Say {
                    speaker: String::new(),
                    text,
                },
                Some(IrWaitKind::DialogueAdvance),
            ),
            _ => Err(HksStoryCompileError::UnsupportedCall {
                path: self.path.to_string(),
                name,
                offset: expression.span.start,
            }),
        }
    }

    fn character_call(&self, expression: &Expr) -> Result<Option<IrCommand>, HksStoryCompileError> {
        let mut methods = Vec::new();
        let Some(arguments) = collect_character_chain(expression, &mut methods) else {
            return Ok(None);
        };
        let actor_id = one_string_literal(arguments).ok_or_else(|| {
            self.invalid_call("char", expression, "expected exactly one string argument")
        })?;
        let mut position = [0.0, 0.0];
        let mut scale = 1.0;
        let mut expressions = Vec::new();
        for (method, arguments) in methods {
            match method {
                "at" => {
                    let position_name = one_string_literal(arguments).ok_or_else(|| {
                        self.invalid_call(
                            "char.at",
                            expression,
                            "expected exactly one string argument",
                        )
                    })?;
                    position = match position_name {
                        "left" => [-600.0, 0.0],
                        "center" => [0.0, 0.0],
                        "right" => [600.0, 0.0],
                        _ => {
                            return Err(self.invalid_call(
                                "char.at",
                                expression,
                                "position must be left, center, or right",
                            ));
                        }
                    };
                }
                "scale" => {
                    let value = one_scalar_number(arguments).ok_or_else(|| {
                        self.invalid_call(
                            "char.scale",
                            expression,
                            "expected exactly one numeric argument",
                        )
                    })?;
                    if value <= 0.0 {
                        return Err(self.invalid_call(
                            "char.scale",
                            expression,
                            "scale must be positive",
                        ));
                    }
                    scale = value as f32;
                }
                "e" => expressions.push(
                    one_string_literal(arguments)
                        .ok_or_else(|| {
                            self.invalid_call(
                                "char.e",
                                expression,
                                "expected exactly one string argument",
                            )
                        })?
                        .to_string(),
                ),
                _ => {
                    return Err(HksStoryCompileError::UnsupportedCall {
                        path: self.path.to_string(),
                        name: format!("char.{method}"),
                        offset: expression.span.start,
                    });
                }
            }
        }
        Ok(Some(IrCommand::ShowCharacter {
            actor_id: actor_id.to_string(),
            character_name: actor_id.to_string(),
            expressions,
            position,
            scale,
        }))
    }

    fn no_arguments(
        &mut self,
        name: &str,
        expression: &Expr,
        arguments: &[Argument],
        command: IrCommand,
    ) -> Result<(), HksStoryCompileError> {
        if !arguments.is_empty() {
            return Err(self.invalid_call(name, expression, "expected no arguments"));
        }
        self.instructions.push(IrInstruction::Emit(command));
        Ok(())
    }

    fn one_string(
        &mut self,
        name: &str,
        expression: &Expr,
        arguments: &[Argument],
        build: impl FnOnce(String) -> IrCommand,
        wait: Option<IrWaitKind>,
    ) -> Result<(), HksStoryCompileError> {
        let [argument] = arguments else {
            return Err(self.invalid_call(
                name,
                expression,
                "expected exactly one string argument",
            ));
        };
        let ExprKind::String(value) = &argument.value.kind else {
            return Err(self.invalid_call(name, expression, "expected a string argument"));
        };
        self.instructions
            .push(IrInstruction::Emit(build(value.clone())));
        if let Some(wait) = wait {
            self.instructions.push(IrInstruction::Wait(wait));
        }
        Ok(())
    }

    fn invalid_call(&self, name: &str, expression: &Expr, message: &str) -> HksStoryCompileError {
        HksStoryCompileError::InvalidCall {
            path: self.path.to_string(),
            name: name.to_string(),
            offset: expression.span.start,
            message: message.to_string(),
        }
    }
}

fn flatten_callee(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => Some(format!("{}.{}", flatten_callee(object)?, name)),
        _ => None,
    }
}

fn collect_character_chain<'a>(
    expression: &'a Expr,
    methods: &mut Vec<(&'a str, &'a [Argument])>,
) -> Option<&'a [Argument]> {
    let ExprKind::Call {
        callee,
        arguments,
        trailing_block,
    } = &expression.kind
    else {
        return None;
    };
    if trailing_block.is_some() || arguments.iter().any(|argument| argument.label.is_some()) {
        return None;
    }
    match &callee.kind {
        ExprKind::Ident(name) if name == "char" => Some(arguments),
        ExprKind::Member { object, name } => {
            let base = collect_character_chain(object, methods)?;
            methods.push((name, arguments));
            Some(base)
        }
        _ => None,
    }
}

fn one_string_literal(arguments: &[Argument]) -> Option<&str> {
    let [argument] = arguments else {
        return None;
    };
    let ExprKind::String(value) = &argument.value.kind else {
        return None;
    };
    Some(value)
}

fn one_scalar_number(arguments: &[Argument]) -> Option<f64> {
    let [argument] = arguments else {
        return None;
    };
    match &argument.value.kind {
        ExprKind::Number {
            value,
            unit: NumberUnit::Scalar,
        } => Some(*value),
        ExprKind::UnaryMinus(value) => match &value.kind {
            ExprKind::Number {
                value,
                unit: NumberUnit::Scalar,
            } => Some(-value),
            _ => None,
        },
        _ => None,
    }
}

fn statement_span(statement: &Stmt) -> &Span {
    match statement {
        Stmt::Let { span, .. } => span,
        Stmt::Expr(expression) => &expression.span,
    }
}

fn source_hash(source: &str) -> u64 {
    source.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
    })
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

    #[test]
    fn lowers_migrated_gallery_story_to_ir() {
        let program = compile_story_to_ir(
            "scripts/gallery.story.hks",
            "clear_text()\nnarrate(\"Gallery\")\nreturn_to_title()",
        )
        .unwrap();
        assert_eq!(
            program.instructions,
            vec![
                IrInstruction::Emit(IrCommand::ClearDialogue),
                IrInstruction::Emit(IrCommand::Say {
                    speaker: String::new(),
                    text: "Gallery".to_string(),
                }),
                IrInstruction::Wait(IrWaitKind::DialogueAdvance),
                IrInstruction::Emit(IrCommand::ReturnToTitle),
                IrInstruction::Halt,
            ]
        );
    }

    #[test]
    fn lowers_fluent_character_setup() {
        let program = compile_story_to_ir(
            "scripts/new_game.story.hks",
            "char(\"ema\").at(\"center\").e(\"happy\").scale(0.14)",
        )
        .unwrap();
        assert!(matches!(
            program.instructions.first(),
            Some(IrInstruction::Emit(IrCommand::ShowCharacter {
                actor_id,
                character_name,
                expressions,
                position,
                scale,
            })) if actor_id == "ema"
                && character_name == "ema"
                && expressions == &vec!["happy".to_string()]
                && position == &[0.0, 0.0]
                && (*scale - 0.14).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn migrated_new_game_story_is_ready_for_ir_handoff() {
        let source =
            include_str!("../../../manosabars/assets/main_hdp_contents/scripts/new_game.story.hks");
        let program = compile_story_to_ir("scripts/new_game.story.hks", source).unwrap();
        assert!(program.validate().is_ok());
        assert_eq!(
            program
                .instructions
                .iter()
                .filter(|instruction| matches!(
                    instruction,
                    IrInstruction::Wait(IrWaitKind::DialogueAdvance)
                ))
                .count(),
            24
        );
    }

    #[test]
    fn lowers_hks_startup_handoff() {
        let source = include_str!("../../../manosabars/assets/main_hdp_contents/startup.story.hks");
        let program = compile_story_to_ir("startup.story.hks", source).unwrap();
        assert_eq!(
            program.instructions,
            vec![
                IrInstruction::Emit(IrCommand::Log("manosaba bootstrap startup".to_string())),
                IrInstruction::Emit(IrCommand::LoadScript {
                    path: "system.rhai".to_string(),
                }),
                IrInstruction::Halt,
            ]
        );
    }
}
