//! Engine-owned HKS story prelude schemas.
//!
//! `hiraku_script` intentionally knows nothing about camera, scene, UI, or ECS
//! concepts. This module validates engine capabilities before they become ECS
//! effects.

use hiraku_script::hks::{
    Argument, BinaryOp, Expr, ExprKind, NumberUnit, Span, Stmt, parse_program,
};
use thiserror::Error;

use crate::script::{
    CameraEffectScope, IrCommand, IrExpression, IrExpressionId, IrInstruction, IrProgram,
    IrWaitKind,
};

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
    pub scope: CameraEffectScope,
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
    let mut scope = None;
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
            Some("scope") if scope.is_none() => scope = Some(scope_value(argument)?),
            Some("at" | "duration" | "ease" | "scope") => {
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
        scope: scope.unwrap_or_default(),
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

fn scope_value(argument: &Argument) -> Result<CameraEffectScope, SchemaError> {
    match &argument.value.kind {
        ExprKind::Symbol(symbol) if symbol == "world" => Ok(CameraEffectScope::World),
        ExprKind::Symbol(symbol) if symbol == "canvas" => Ok(CameraEffectScope::Canvas),
        ExprKind::Symbol(_) => Err(error(argument, "camera scope must be .world or .canvas")),
        _ => Err(error(argument, "camera scope must be a symbol literal")),
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
/// deterministic IR runtime used by the engine embedding.
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
        source_hash: source_hash(source),
        expressions: Vec::new(),
        instructions: Vec::new(),
        functions: program
            .statements
            .iter()
            .filter(|statement| matches!(statement, Stmt::Function { .. }))
            .cloned()
            .collect(),
    };
    for statement in &program.statements {
        lowerer.statement(statement)?;
    }
    lowerer.instructions.push(IrInstruction::Halt);
    Ok(IrProgram::with_expressions(
        source_hash(source),
        lowerer.expressions,
        lowerer.instructions,
    ))
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
    source_hash: u64,
    expressions: Vec<IrExpression>,
    instructions: Vec<IrInstruction>,
    functions: Vec<Stmt>,
}

impl HksStoryLowerer<'_> {
    fn statement(&mut self, statement: &Stmt) -> Result<(), HksStoryCompileError> {
        match statement {
            Stmt::Function { .. } => Ok(()),
            Stmt::Let {
                name, value, span, ..
            } => self.let_statement(name, value, span),
            Stmt::If {
                condition,
                then_block,
                else_block,
                span,
            } => self.if_statement(condition, then_block, else_block.as_ref(), span),
            Stmt::While {
                condition,
                body,
                span,
            } => self.while_statement(condition, body, span),
            Stmt::Expr(expression) => self.expression_statement(expression),
        }
    }

    fn let_statement(
        &mut self,
        variable: &str,
        value: &Expr,
        span: &Span,
    ) -> Result<(), HksStoryCompileError> {
        let ExprKind::Call {
            callee,
            arguments,
            trailing_block,
        } = &value.kind
        else {
            return Err(HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: span.start,
            });
        };
        let name =
            flatten_callee(callee).ok_or_else(|| HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: value.span.start,
            })?;
        if trailing_block.is_some()
            && arguments.is_empty()
            && matches!(name.as_str(), "seq" | "par")
        {
            let bytecode = crate::hks_capabilities::compile_expression(
                value,
                &self.functions,
                self.source_hash ^ value.span.start as u64,
            )
            .ok_or_else(|| HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: span.start,
            })?;
            self.instructions
                .push(IrInstruction::Emit(IrCommand::HksStatement {
                    bytecode,
                    task_result: Some(variable.to_string()),
                }));
            return Ok(());
        }
        if trailing_block.is_some() {
            return Err(HksStoryCompileError::UnsupportedStatement {
                path: self.path.to_string(),
                offset: span.start,
            });
        }
        if name != "openUi" || arguments.len() != 1 || arguments[0].label.is_some() {
            return Err(HksStoryCompileError::UnsupportedCall {
                path: self.path.to_string(),
                name,
                offset: value.span.start,
            });
        }
        let ExprKind::String(path) = &arguments[0].value.kind else {
            return Err(self.invalid_call("openUi", value, "expected one string path"));
        };
        self.instructions
            .push(IrInstruction::Emit(IrCommand::OpenUi {
                path: path.clone(),
                result: variable.to_string(),
            }));
        self.instructions
            .push(IrInstruction::Wait(IrWaitKind::UiIntent));
        Ok(())
    }

    fn expression_statement(&mut self, expression: &Expr) -> Result<(), HksStoryCompileError> {
        if let Some(bytecode) = crate::hks_capabilities::compile_expression(
            expression,
            &self.functions,
            self.source_hash ^ expression.span.start as u64,
        ) {
            self.instructions
                .push(IrInstruction::Emit(IrCommand::HksStatement {
                    bytecode,
                    task_result: None,
                }));
            return Ok(());
        }
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
        match name.as_str() {
            "camera.blur" => return self.camera_blur(expression, arguments),
            "camera.zoom" => return self.camera_zoom(expression),
            "wait" => {
                if arguments.len() != 1 || arguments[0].label.is_some() {
                    return Err(self.invalid_call("wait", expression, "expected one task handle"));
                }
                let ExprKind::Ident(handle) = &arguments[0].value.kind else {
                    return Err(self.invalid_call(
                        "wait",
                        expression,
                        "expected a task handle variable",
                    ));
                };
                self.instructions
                    .push(IrInstruction::Emit(IrCommand::WaitHksTask {
                        handle: handle.clone(),
                    }));
                return Ok(());
            }
            _ => {}
        }
        if arguments.iter().any(|argument| argument.label.is_some()) {
            return Err(self.invalid_call(
                &name,
                expression,
                "named arguments are not supported by this command",
            ));
        }
        match name.as_str() {
            "clearText" => {
                self.no_arguments(&name, expression, arguments, IrCommand::ClearDialogue)
            }
            "stopBgm" => self.no_arguments(&name, expression, arguments, IrCommand::StopBgm),
            "exit" => self.no_arguments(&name, expression, arguments, IrCommand::Exit),
            "returnToTitle" => {
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
            "loadScript" => self.one_string(
                &name,
                expression,
                arguments,
                |path| IrCommand::LoadScript { path },
                None,
            ),
            "adjustSetting" => self.adjust_setting(expression, arguments),
            "playBgm" => self.play_bgm(expression, arguments),
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

    fn if_statement(
        &mut self,
        condition: &Expr,
        then_block: &hiraku_script::hks::Block,
        else_block: Option<&hiraku_script::hks::Block>,
        span: &Span,
    ) -> Result<(), HksStoryCompileError> {
        let expression = self.condition(condition, span)?;
        let branch_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Branch {
            expression,
            then_pc: 0,
            else_pc: 0,
        });
        for statement in &then_block.statements {
            self.statement(statement)?;
        }
        let end_jump_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Jump(0));
        let else_pc = self.instructions.len() as u32;
        if let Some(else_block) = else_block {
            for statement in &else_block.statements {
                self.statement(statement)?;
            }
        }
        let end_pc = self.instructions.len() as u32;
        self.instructions[branch_pc] = IrInstruction::Branch {
            expression,
            then_pc: (branch_pc + 1) as u32,
            else_pc,
        };
        self.instructions[end_jump_pc] = IrInstruction::Jump(end_pc);
        Ok(())
    }

    fn while_statement(
        &mut self,
        condition: &Expr,
        body: &hiraku_script::hks::Block,
        span: &Span,
    ) -> Result<(), HksStoryCompileError> {
        let condition_pc = self.instructions.len() as u32;
        let expression = self.condition(condition, span)?;
        let branch_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Branch {
            expression,
            then_pc: 0,
            else_pc: 0,
        });
        for statement in &body.statements {
            self.statement(statement)?;
        }
        self.instructions.push(IrInstruction::Jump(condition_pc));
        let end_pc = self.instructions.len() as u32;
        self.instructions[branch_pc] = IrInstruction::Branch {
            expression,
            then_pc: (branch_pc + 1) as u32,
            else_pc: end_pc,
        };
        Ok(())
    }

    fn condition(
        &mut self,
        expression: &Expr,
        span: &Span,
    ) -> Result<IrExpressionId, HksStoryCompileError> {
        let value = match &expression.kind {
            ExprKind::Bool(value) => IrExpression::BoolLiteral(*value),
            ExprKind::Ident(name) => IrExpression::BoolVariable(name.clone()),
            ExprKind::Binary {
                left,
                op: BinaryOp::Equal,
                right,
            } => match (&left.kind, &right.kind) {
                (ExprKind::Ident(variable), ExprKind::String(value)) => {
                    IrExpression::StringEquals {
                        variable: variable.clone(),
                        value: value.clone(),
                    }
                }
                _ => {
                    return Err(HksStoryCompileError::UnsupportedStatement {
                        path: self.path.to_string(),
                        offset: span.start,
                    });
                }
            },
            _ => {
                return Err(HksStoryCompileError::UnsupportedStatement {
                    path: self.path.to_string(),
                    offset: span.start,
                });
            }
        };
        let id = IrExpressionId(self.expressions.len() as u32);
        self.expressions.push(value);
        Ok(id)
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

    fn adjust_setting(
        &mut self,
        expression: &Expr,
        arguments: &[Argument],
    ) -> Result<(), HksStoryCompileError> {
        let [name, delta] = arguments else {
            return Err(self.invalid_call(
                "adjustSetting",
                expression,
                "expected a string name and numeric delta",
            ));
        };
        let Some(name) = one_string_literal(std::slice::from_ref(name)) else {
            return Err(self.invalid_call(
                "adjustSetting",
                expression,
                "expected a string setting name",
            ));
        };
        let Some(delta) = one_scalar_number(std::slice::from_ref(delta)) else {
            return Err(self.invalid_call("adjustSetting", expression, "expected a numeric delta"));
        };
        self.instructions
            .push(IrInstruction::Emit(IrCommand::AdjustSetting {
                name: name.to_string(),
                delta: delta as f32,
            }));
        Ok(())
    }

    fn play_bgm(
        &mut self,
        expression: &Expr,
        arguments: &[Argument],
    ) -> Result<(), HksStoryCompileError> {
        let [name, volume, fade] = arguments else {
            return Err(self.invalid_call(
                "playBgm",
                expression,
                "expected a music name, volume, and fade duration in milliseconds",
            ));
        };
        let Some(name) = one_string_literal(std::slice::from_ref(name)) else {
            return Err(self.invalid_call("playBgm", expression, "music name must be a string"));
        };
        let Some(volume) = one_scalar_number(std::slice::from_ref(volume)) else {
            return Err(self.invalid_call("playBgm", expression, "volume must be numeric"));
        };
        let Some(fade) = one_scalar_number(std::slice::from_ref(fade)) else {
            return Err(self.invalid_call("playBgm", expression, "fade duration must be numeric"));
        };
        if !(0.0..=1.0).contains(&volume) || fade < 0.0 {
            return Err(self.invalid_call(
                "playBgm",
                expression,
                "volume must be between 0 and 1 and fade duration must not be negative",
            ));
        }
        self.instructions
            .push(IrInstruction::Emit(IrCommand::PlayBgm {
                path: name.to_string(),
                volume: volume as f32,
                fade_in_ms: Some(fade.round() as u64),
            }));
        Ok(())
    }

    fn camera_blur(
        &mut self,
        expression: &Expr,
        arguments: &[Argument],
    ) -> Result<(), HksStoryCompileError> {
        let mut intensity = None;
        let mut duration = 0.0;
        let mut ease = Ease::Linear;
        let mut scope = CameraEffectScope::World;
        for argument in arguments {
            match argument.label.as_deref() {
                None if intensity.is_none() => {
                    intensity = Some(number(argument, "intensity").map_err(|error| {
                        self.invalid_call("camera.blur", expression, &error.message)
                    })?)
                }
                Some("duration") => {
                    duration = number(argument, "duration").map_err(|error| {
                        self.invalid_call("camera.blur", expression, &error.message)
                    })?
                }
                Some("ease") => {
                    ease = ease_value(argument).map_err(|error| {
                        self.invalid_call("camera.blur", expression, &error.message)
                    })?
                }
                Some("scope") => {
                    scope = scope_value(argument).map_err(|error| {
                        self.invalid_call("camera.blur", expression, &error.message)
                    })?
                }
                _ => {
                    return Err(self.invalid_call(
                        "camera.blur",
                        expression,
                        "expected intensity with optional duration and ease",
                    ));
                }
            }
        }
        let intensity = intensity
            .ok_or_else(|| self.invalid_call("camera.blur", expression, "intensity is required"))?;
        if intensity < 0.0 || duration < 0.0 {
            return Err(self.invalid_call(
                "camera.blur",
                expression,
                "intensity and duration must not be negative",
            ));
        }
        self.instructions
            .push(IrInstruction::Emit(IrCommand::SetCamera {
                blur: Some(intensity),
                zoom: None,
                scope,
                duration_ms: (duration * 1000.0).round() as u64,
                ease: ease_name(ease).to_string(),
            }));
        Ok(())
    }

    fn camera_zoom(&mut self, expression: &Expr) -> Result<(), HksStoryCompileError> {
        let zoom = normalize_camera_zoom(expression)
            .map_err(|error| self.invalid_call("camera.zoom", expression, &error.message))?;
        if !matches!(zoom.at, PositionSpec::Preset(PresetPosition::Center)) {
            return Err(self.invalid_call(
                "camera.zoom",
                expression,
                "the transitional IR runtime currently supports only .center",
            ));
        }
        self.instructions
            .push(IrInstruction::Emit(IrCommand::SetCamera {
                blur: None,
                zoom: Some(zoom.scale),
                scope: zoom.scope,
                duration_ms: (zoom.duration * 1000.0).round() as u64,
                ease: ease_name(zoom.ease).to_string(),
            }));
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

fn ease_name(ease: Ease) -> &'static str {
    match ease {
        Ease::Linear => "linear",
        Ease::EaseIn => "ease_in",
        Ease::EaseOut => "ease_out",
        Ease::EaseInOut => "ease_in_out",
    }
}

fn flatten_callee(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => Some(format!("{}.{}", flatten_callee(object)?, name)),
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

fn source_hash(source: &str) -> u64 {
    source.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hks_capabilities::{StoryEffect, StoryWait};
    use hiraku_script::hks::{Stmt, parse_program};

    fn zoom(source: &str) -> Expr {
        let program = parse_program(source).unwrap();
        let [Stmt::Expr(expression)] = program.statements.as_slice() else {
            panic!("expected one expression");
        };
        expression.clone()
    }

    fn native_outputs(program: &IrProgram) -> Vec<crate::hks_capabilities::CapabilityOutput> {
        program
            .instructions
            .iter()
            .filter_map(|instruction| match instruction {
                IrInstruction::Emit(IrCommand::HksStatement { bytecode, .. }) => {
                    Some(crate::hks_capabilities::execute(bytecode.clone()).unwrap())
                }
                _ => None,
            })
            .collect()
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
                scope: CameraEffectScope::World,
            }
        );
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, duration: 1)")).unwrap(),
            CameraZoom {
                scale: 1.2,
                at: PositionSpec::Preset(PresetPosition::Center),
                duration: 1.0,
                ease: Ease::Linear,
                scope: CameraEffectScope::World,
            }
        );
        assert_eq!(
            normalize_camera_zoom(&zoom("camera.zoom(1.2, scope: .canvas)"))
                .unwrap()
                .scope,
            CameraEffectScope::Canvas
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
            "clearText()\nnarrate(\"Gallery\")\nreturnToTitle()",
        )
        .unwrap();
        let outputs = native_outputs(&program);
        assert!(matches!(outputs[0].commands[0], StoryEffect::ClearDialogue));
        assert!(
            matches!(&outputs[1].commands[0], StoryEffect::Say { text, .. } if text == "Gallery")
        );
        assert_eq!(outputs[1].wait, Some(StoryWait::DialogueAdvance));
        assert!(matches!(outputs[2].commands[0], StoryEffect::ReturnToTitle));
    }

    #[test]
    fn lowers_fluent_character_setup() {
        let program = compile_story_to_ir(
            "scripts/new_game.story.hks",
            "char(\"ema\").at(\"center\").e(\"happy\").scale(0.14)",
        )
        .unwrap();
        let Some(IrInstruction::Emit(IrCommand::HksStatement { bytecode, .. })) =
            program.instructions.first()
        else {
            panic!("expected a native HKS statement");
        };
        let output = crate::hks_capabilities::execute(bytecode.clone()).unwrap();
        assert!(matches!(&output.commands[0], StoryEffect::ShowCharacter {
            actor_id, character_name, expressions, position, scale,
        } if actor_id == "ema" && character_name == "ema"
            && expressions == &["happy"] && position == &[0.0, 0.0]
            && (*scale - 0.14).abs() < f32::EPSILON));
    }

    #[test]
    fn task_creation_is_non_blocking_until_wait_is_called() {
        let program = compile_story_to_ir(
            "scripts/task.story.hks",
            r#"
                let voices = par {
                    voice("voice/first")
                    voice("voice/second")
                }
                log("task started")
                wait(voices)
                log("task completed")
            "#,
        )
        .expect("task handle story must lower to transitional IR");

        assert!(matches!(
            &program.instructions[0],
            IrInstruction::Emit(IrCommand::HksStatement {
                task_result: Some(handle),
                ..
            }) if handle == "voices"
        ));
        assert!(matches!(
            &program.instructions[2],
            IrInstruction::Emit(IrCommand::WaitHksTask { handle }) if handle == "voices"
        ));
        assert!(!program.instructions.iter().any(|instruction| matches!(
            instruction,
            IrInstruction::Wait(IrWaitKind::TaskCompletion)
        )));
    }

    #[test]
    fn migrated_new_game_story_is_ready_for_ir_handoff() {
        let source =
            include_str!("../../../manosabars/assets/main_hdp_contents/scripts/new_game.story.hks");
        let program = compile_story_to_ir("scripts/new_game.story.hks", source).unwrap();
        assert!(program.validate().is_ok());
        assert_eq!(
            native_outputs(&program)
                .iter()
                .filter(|output| output.wait == Some(StoryWait::DialogueAdvance))
                .count(),
            24
        );
    }

    #[test]
    fn lowers_hks_startup_handoff() {
        let source = include_str!("../../../manosabars/assets/main_hdp_contents/startup.story.hks");
        let program = compile_story_to_ir("startup.story.hks", source).unwrap();
        let commands = native_outputs(&program)
            .into_iter()
            .flat_map(|output| output.commands)
            .collect::<Vec<_>>();
        assert!(
            matches!(&commands[0], StoryEffect::Log(message) if message == "manosaba bootstrap startup")
        );
        assert!(
            matches!(&commands[1], StoryEffect::LoadScript { path } if path == "system.story.hks")
        );
    }

    #[test]
    fn lowers_settings_story_control_flow() {
        let source =
            include_str!("../../../manosabars/assets/main_hdp_contents/scripts/settings.story.hks");
        let program = compile_story_to_ir("scripts/settings.story.hks", source).unwrap();
        assert!(program.validate().is_ok());
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::OpenUi { path, result })
                    if path == "../ui/settings.ui.hks" && result == "action"
            )
        }));
        assert!(program.instructions.windows(2).any(|instructions| matches!(
            instructions,
            [
                IrInstruction::Emit(IrCommand::OpenUi { .. }),
                IrInstruction::Wait(IrWaitKind::UiIntent)
            ]
        )));
        assert!(
            native_outputs(&program)
                .iter()
                .any(|output| output.commands.iter().any(|command| {
                    matches!(command, StoryEffect::AdjustSetting { name, delta }
                if name == "bgmVolume" && (*delta - 0.1).abs() < f32::EPSILON)
                }))
        );
    }

    #[test]
    fn lowers_the_hks_title_system() {
        let source = include_str!("../../../manosabars/assets/main_hdp_contents/system.story.hks");
        let program = compile_story_to_ir("system.story.hks", source).unwrap();
        assert!(program.validate().is_ok());
        assert!(
            native_outputs(&program)
                .iter()
                .any(|output| output.commands.iter().any(|command| {
                    matches!(command, StoryEffect::PlayBgm { path, .. } if path == "title")
                }))
        );
        assert!(program.instructions.iter().any(|instruction| matches!(
            instruction,
            IrInstruction::Emit(IrCommand::OpenUi { path, .. }) if path == "ui/title.ui.hks"
        )));
        assert!(program.instructions.windows(2).any(|instructions| matches!(
            instructions,
            [
                IrInstruction::Emit(IrCommand::OpenUi { path, .. }),
                IrInstruction::Wait(IrWaitKind::UiIntent)
            ] if path == "ui/title.ui.hks"
        )));
    }

    #[test]
    fn user_functions_call_registered_story_capabilities() {
        let program = compile_story_to_ir(
            "function.story.hks",
            r#"
                fn announce(text) {
                    log(text)
                }
                announce("from function")
            "#,
        )
        .unwrap();
        let outputs = native_outputs(&program);
        assert!(
            matches!(&outputs[0].commands[0], StoryEffect::Log(message) if message == "from function")
        );
    }
}
