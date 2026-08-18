use rhai::{ASTFlags, Engine, Expr, FnCallExpr, OptimizationLevel, Position, Stmt, StmtBlock};
use thiserror::Error;

use super::ir::{
    IrChoiceOption, IrCommand, IrExpression, IrExpressionId, IrInstruction, IrProgram, IrWaitKind,
};

/// Compiles the deterministic command-only Rhai subset into Hiraku IR.
///
/// This intentionally rejects closures and arbitrary calls
/// until their execution semantics are represented by the IR VM.
pub fn compile_to_ir(path: &str, source: &str) -> Result<IrProgram, IrCompileError> {
    let mut engine = Engine::new_raw();
    engine.set_optimization_level(OptimizationLevel::None);
    let ast = engine
        .compile(source)
        .map_err(|error| IrCompileError::Parse {
            path: path.to_string(),
            message: error.to_string(),
        })?;
    let mut lowerer = Lowerer {
        path,
        expressions: Vec::new(),
        instructions: Vec::new(),
        loops: Vec::new(),
        apis: IrApiRegistry::default(),
    };
    lowerer.lower_statements(ast.statements())?;
    lowerer.instructions.push(IrInstruction::Halt);
    Ok(IrProgram::with_expressions(
        source_hash(source),
        lowerer.expressions,
        lowerer.instructions,
    ))
}

struct Lowerer<'a> {
    path: &'a str,
    expressions: Vec<IrExpression>,
    instructions: Vec<IrInstruction>,
    loops: Vec<LoopContext>,
    apis: IrApiRegistry,
}

#[derive(Clone, Copy)]
struct IrApiRegistry {
    commands: &'static [(&'static str, CommandApi)],
    character: &'static [(&'static str, CharacterApi)],
}

type CommandApi = for<'a> fn(RhaiCall<'a>) -> Result<IrEmission, IrCompileError>;
type CharacterApi =
    for<'a> fn(&mut CharacterFlowLowering, RhaiCall<'a>) -> Result<(), IrCompileError>;

impl Default for IrApiRegistry {
    fn default() -> Self {
        Self {
            commands: &[
                ("log", api::log),
                ("stop_bgm", api::stop_bgm),
                ("play_bgm", api::play_bgm),
                ("quit", api::quit),
                ("load_script", api::load_script),
                ("return_to_title", api::return_to_title),
                ("clear_text", api::clear_text),
                ("narrate", api::narrate),
                ("bg", api::bg),
            ],
            character: &[
                ("char", api::char),
                ("at", api::at),
                ("scale", api::scale),
                ("e", api::expression),
            ],
        }
    }
}

impl IrApiRegistry {
    fn command(&self, name: &str) -> Option<CommandApi> {
        self.commands
            .iter()
            .find_map(|(registered, handler)| (*registered == name).then_some(*handler))
    }

    fn character(&self, name: &str) -> Option<CharacterApi> {
        self.character
            .iter()
            .find_map(|(registered, handler)| (*registered == name).then_some(*handler))
    }
}

struct LoopContext {
    continue_pc: u32,
    break_patches: Vec<usize>,
}

struct IrEmission {
    command: IrCommand,
    wait: Option<IrWaitKind>,
}

impl Lowerer<'_> {
    fn lower_statements(&mut self, statements: &[Stmt]) -> Result<(), IrCompileError> {
        for statement in statements {
            self.lower_statement(statement)?;
        }
        Ok(())
    }

    fn lower_statement(&mut self, statement: &Stmt) -> Result<(), IrCompileError> {
        match statement {
            Stmt::Noop(_) => Ok(()),
            Stmt::Block(block) => self.lower_block(block),
            Stmt::FnCall(call, position) => self.lower_command_call(call, *position),
            Stmt::Var(variable, _, position) => {
                let result = variable.0.as_str().to_string();
                let Expr::FnCall(call, call_position) = &variable.1 else {
                    return Err(IrCompileError::UnsupportedStatement {
                        path: self.path.to_string(),
                        kind: "variable declaration".to_string(),
                        line: position.line(),
                        column: position.position(),
                    });
                };
                let call = RhaiCall::new(call, *call_position, self.path);
                let emission = match call.name() {
                    "choice" => api::choice(call, result)?,
                    "open_ui" => api::open_ui(call, result)?,
                    _ => {
                        return Err(IrCompileError::UnsupportedStatement {
                            path: self.path.to_string(),
                            kind: "variable declaration".to_string(),
                            line: position.line(),
                            column: position.position(),
                        });
                    }
                };
                self.instructions
                    .push(IrInstruction::Emit(emission.command));
                if let Some(wait) = emission.wait {
                    self.instructions.push(IrInstruction::Wait(wait));
                }
                Ok(())
            }
            Stmt::Expr(expression) => match expression.as_ref() {
                Expr::FnCall(call, position) => self.lower_command_call(call, *position),
                Expr::Dot(_, _, _) if self.is_camera_chain(expression) => {
                    self.lower_camera_chain(expression)
                }
                Expr::Dot(_, _, _) => self.lower_fluent_character(expression),
                expression => Err(self.unsupported_expression(expression)),
            },
            Stmt::If(flow, _) => self.lower_if(flow),
            Stmt::While(flow, _) => self.lower_while(flow),
            Stmt::BreakLoop(expression, flags, position) => {
                if expression.is_some() {
                    return Err(IrCompileError::UnsupportedStatement {
                        path: self.path.to_string(),
                        kind: "break/continue with expression".to_string(),
                        line: position.line(),
                        column: position.position(),
                    });
                }
                let Some(loop_context) = self.loops.last_mut() else {
                    return Err(IrCompileError::UnsupportedStatement {
                        path: self.path.to_string(),
                        kind: "break/continue outside loop".to_string(),
                        line: position.line(),
                        column: position.position(),
                    });
                };
                if flags.contains(ASTFlags::BREAK) {
                    loop_context.break_patches.push(self.instructions.len());
                    self.instructions.push(IrInstruction::Jump(0));
                } else {
                    self.instructions
                        .push(IrInstruction::Jump(loop_context.continue_pc));
                }
                Ok(())
            }
            other => Err(self.unsupported_statement(other)),
        }
    }

    fn lower_block(&mut self, block: &StmtBlock) -> Result<(), IrCompileError> {
        self.lower_statements(block.statements())
    }

    fn lower_if(&mut self, flow: &rhai::FlowControl) -> Result<(), IrCompileError> {
        let expression = self.lower_condition(&flow.expr)?;
        let branch_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Branch {
            expression,
            then_pc: 0,
            else_pc: 0,
        });

        self.lower_block(&flow.body)?;
        let end_jump_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Jump(0));
        let else_pc = self.instructions.len() as u32;
        self.lower_block(&flow.branch)?;
        let end_pc = self.instructions.len() as u32;

        self.instructions[branch_pc] = IrInstruction::Branch {
            expression,
            then_pc: (branch_pc + 1) as u32,
            else_pc,
        };
        self.instructions[end_jump_pc] = IrInstruction::Jump(end_pc);
        Ok(())
    }

    fn lower_condition(&mut self, expression: &Expr) -> Result<IrExpressionId, IrCompileError> {
        let expression = match expression {
            Expr::BoolConstant(value, _) => IrExpression::BoolLiteral(*value),
            Expr::Variable(value, _, _) => IrExpression::BoolVariable(value.1.to_string()),
            Expr::FnCall(call, _) if call.name == "==" && call.args.len() == 2 => {
                let (variable, value) = match (&call.args[0], &call.args[1]) {
                    (Expr::Variable(variable, _, _), Expr::StringConstant(value, _)) => {
                        (variable.1.to_string(), value.to_string())
                    }
                    (Expr::StringConstant(value, _), Expr::Variable(variable, _, _)) => {
                        (variable.1.to_string(), value.to_string())
                    }
                    _ => return Err(self.unsupported_expression(expression)),
                };
                IrExpression::StringEquals { variable, value }
            }
            expression => return Err(self.unsupported_expression(expression)),
        };
        let id = IrExpressionId(self.expressions.len() as u32);
        self.expressions.push(expression);
        Ok(id)
    }

    fn lower_while(&mut self, flow: &rhai::FlowControl) -> Result<(), IrCompileError> {
        let is_loop = matches!(&flow.expr, Expr::Unit(_));
        if is_loop {
            let body_pc = self.instructions.len() as u32;
            self.loops.push(LoopContext {
                continue_pc: body_pc,
                break_patches: Vec::new(),
            });
            self.lower_block(&flow.body)?;
            self.instructions.push(IrInstruction::Jump(body_pc));
            let end_pc = self.instructions.len() as u32;
            self.patch_loop_breaks(end_pc);
            self.loops.pop();
            return Ok(());
        }

        let condition_pc = self.instructions.len() as u32;
        let expression = self.lower_condition(&flow.expr)?;
        let branch_pc = self.instructions.len();
        self.instructions.push(IrInstruction::Branch {
            expression,
            then_pc: 0,
            else_pc: 0,
        });
        let body_pc = self.instructions.len() as u32;
        self.loops.push(LoopContext {
            continue_pc: condition_pc,
            break_patches: Vec::new(),
        });
        self.lower_block(&flow.body)?;
        self.instructions.push(IrInstruction::Jump(condition_pc));
        let end_pc = self.instructions.len() as u32;
        self.patch_loop_breaks(end_pc);
        self.loops.pop();
        self.instructions[branch_pc] = IrInstruction::Branch {
            expression,
            then_pc: body_pc,
            else_pc: end_pc,
        };
        Ok(())
    }

    fn patch_loop_breaks(&mut self, end_pc: u32) {
        let break_patches = self
            .loops
            .last_mut()
            .map(|context| std::mem::take(&mut context.break_patches))
            .unwrap_or_default();
        for pc in break_patches {
            self.instructions[pc] = IrInstruction::Jump(end_pc);
        }
    }

    fn lower_command_call(
        &mut self,
        call: &FnCallExpr,
        position: Position,
    ) -> Result<(), IrCompileError> {
        let call = RhaiCall::new(call, position, self.path);
        let Some(handler) = self.apis.command(call.name()) else {
            return Err(call.unsupported_call());
        };
        let emission = handler(call)?;
        self.instructions
            .push(IrInstruction::Emit(emission.command));
        if let Some(wait) = emission.wait {
            self.instructions.push(IrInstruction::Wait(wait));
        }
        Ok(())
    }

    fn is_camera_chain(&self, expression: &Expr) -> bool {
        match expression {
            Expr::Dot(binary, _, _) => match &binary.lhs {
                Expr::Variable(value, _, _) => value.1 == "camera",
                Expr::Dot(_, _, _) => self.is_camera_chain(&binary.lhs),
                _ => false,
            },
            _ => false,
        }
    }

    fn lower_camera_chain(&mut self, expression: &Expr) -> Result<(), IrCompileError> {
        let mut flow = CameraFlowLowering::default();
        self.read_camera_chain(expression, &mut flow)?;
        self.instructions
            .push(IrInstruction::Emit(IrCommand::SetCamera {
                blur: flow.blur,
                zoom: flow.zoom,
                duration_ms: flow.duration_ms,
                ease: flow.ease,
            }));
        Ok(())
    }

    fn read_camera_chain(
        &self,
        expression: &Expr,
        flow: &mut CameraFlowLowering,
    ) -> Result<(), IrCompileError> {
        match expression {
            Expr::Dot(binary, _, _) => {
                if let Expr::Variable(value, _, _) = &binary.lhs {
                    if value.1 != "camera" {
                        return Err(self.unsupported_expression(expression));
                    }
                } else {
                    self.read_camera_chain(&binary.lhs, flow)?;
                }
                self.read_camera_method(&binary.rhs, flow)
            }
            _ => Err(self.unsupported_expression(expression)),
        }
    }

    fn read_camera_method(
        &self,
        expression: &Expr,
        flow: &mut CameraFlowLowering,
    ) -> Result<(), IrCompileError> {
        match expression {
            Expr::MethodCall(call, position) => {
                let call = RhaiCall::new(call, *position, self.path);
                match call.name() {
                    "blur" => flow.blur = Some(call.one_float()?),
                    "zoom" => flow.zoom = Some(call.one_float()?),
                    "duration" => {
                        let seconds = call.one_float()?;
                        if seconds < 0.0 {
                            return Err(call.invalid("duration must not be negative"));
                        }
                        flow.duration_ms = (seconds * 1000.0).round() as u64;
                    }
                    "ease" => flow.ease = call.one_string()?,
                    _ => return Err(call.unsupported_call()),
                }
                Ok(())
            }
            Expr::Dot(binary, _, _) => {
                self.read_camera_method(&binary.lhs, flow)?;
                self.read_camera_method(&binary.rhs, flow)
            }
            _ => Err(self.unsupported_expression(expression)),
        }
    }

    fn lower_fluent_character(&mut self, expression: &Expr) -> Result<(), IrCompileError> {
        let mut flow = CharacterFlowLowering {
            scale: 1.0,
            ..Default::default()
        };
        self.read_character_chain(expression, &mut flow)?;
        self.instructions
            .push(IrInstruction::Emit(IrCommand::ShowCharacter {
                actor_id: flow.actor_id.clone(),
                character_name: flow.character_name,
                expressions: flow.expressions,
                position: flow.position,
                scale: flow.scale,
            }));
        Ok(())
    }

    fn read_character_chain(
        &self,
        expression: &Expr,
        flow: &mut CharacterFlowLowering,
    ) -> Result<(), IrCompileError> {
        match expression {
            Expr::FnCall(call, position) => {
                let call = RhaiCall::new(call, *position, self.path);
                let Some(handler) = self.apis.character(call.name()) else {
                    return Err(self.unsupported_expression(expression));
                };
                handler(flow, call)
            }
            Expr::Dot(binary, _, _) => {
                self.read_character_chain(&binary.lhs, flow)?;
                self.read_character_suffix(&binary.rhs, flow)
            }
            _ => Err(self.unsupported_expression(expression)),
        }
    }

    fn read_character_suffix(
        &self,
        expression: &Expr,
        flow: &mut CharacterFlowLowering,
    ) -> Result<(), IrCompileError> {
        match expression {
            Expr::MethodCall(call, position) => {
                let call = RhaiCall::new(call, *position, self.path);
                let Some(handler) = self.apis.character(call.name()) else {
                    return Err(call.unsupported_call());
                };
                handler(flow, call)
            }
            Expr::Dot(binary, _, _) => {
                self.read_character_suffix(&binary.lhs, flow)?;
                self.read_character_suffix(&binary.rhs, flow)
            }
            _ => Err(self.unsupported_expression(expression)),
        }
    }

    fn unsupported_statement(&self, statement: &Stmt) -> IrCompileError {
        let position = statement.position();
        IrCompileError::UnsupportedStatement {
            path: self.path.to_string(),
            kind: statement_kind(statement).to_string(),
            line: position.line(),
            column: position.position(),
        }
    }

    fn unsupported_expression(&self, expression: &Expr) -> IrCompileError {
        let position = expression.position();
        IrCompileError::UnsupportedExpression {
            path: self.path.to_string(),
            kind: expression_kind(expression).to_string(),
            line: position.line(),
            column: position.position(),
        }
    }
}

/// Typed, Rust-like access to a Rhai call. This is the only layer that
/// understands Rhai's raw `FnCallExpr` argument representation.
struct RhaiCall<'a> {
    call: &'a FnCallExpr,
    position: Position,
    path: &'a str,
}

impl<'a> RhaiCall<'a> {
    fn new(call: &'a FnCallExpr, position: Position, path: &'a str) -> Self {
        Self {
            call,
            position,
            path,
        }
    }

    fn name(&self) -> &str {
        self.call.name.as_str()
    }

    fn no_args(&self) -> Result<(), IrCompileError> {
        if self.call.args.is_empty() {
            Ok(())
        } else {
            Err(self.invalid("does not accept arguments"))
        }
    }

    fn one_string(&self) -> Result<String, IrCompileError> {
        let Some(Expr::StringConstant(value, _)) = self.call.args.first() else {
            return Err(self.invalid("requires one string literal argument"));
        };
        if self.call.args.len() != 1 {
            return Err(self.invalid("requires exactly one argument"));
        }
        Ok(value.to_string())
    }

    fn one_float(&self) -> Result<f32, IrCompileError> {
        if self.call.args.len() != 1 {
            return Err(self.invalid("requires exactly one argument"));
        }
        self.float_at(0)
    }

    fn args(&self, count: usize) -> Result<(), IrCompileError> {
        if self.call.args.len() == count {
            Ok(())
        } else {
            Err(self.invalid(format!("requires exactly {count} arguments")))
        }
    }

    fn string_at(&self, index: usize) -> Result<String, IrCompileError> {
        let Some(Expr::StringConstant(value, _)) = self.call.args.get(index) else {
            return Err(self.invalid("requires a string literal"));
        };
        Ok(value.to_string())
    }

    fn float_at(&self, index: usize) -> Result<f32, IrCompileError> {
        match self.call.args.get(index) {
            Some(Expr::FloatConstant(value, _)) => Ok(**value as f32),
            Some(Expr::IntegerConstant(value, _)) => Ok(*value as f32),
            _ => Err(self.invalid("requires a numeric literal")),
        }
    }

    fn choice_options(&self, index: usize) -> Result<Vec<IrChoiceOption>, IrCompileError> {
        let Some(Expr::Array(items, _)) = self.call.args.get(index) else {
            return Err(self.invalid("choice options must be an array literal"));
        };
        items
            .iter()
            .map(|item| {
                let Expr::Map(entries, _) = item else {
                    return Err(self.invalid("choice options must be map literals"));
                };
                let text = entries
                    .0
                    .iter()
                    .find(|(key, _)| key.as_str() == "text")
                    .and_then(|(_, value)| literal_string(value))
                    .ok_or_else(|| self.invalid("choice option requires string `text`"))?;
                let value = entries
                    .0
                    .iter()
                    .find(|(key, _)| key.as_str() == "value")
                    .and_then(|(_, value)| literal_string(value))
                    .ok_or_else(|| self.invalid("choice option requires string `value`"))?;
                Ok(IrChoiceOption { text, value })
            })
            .collect()
    }

    fn invalid(&self, message: impl Into<String>) -> IrCompileError {
        IrCompileError::InvalidCall {
            path: self.path.to_string(),
            name: self.call.name.to_string(),
            message: message.into(),
            line: self.position.line(),
            column: self.position.position(),
        }
    }

    fn unsupported_call(&self) -> IrCompileError {
        IrCompileError::UnsupportedCall {
            path: self.path.to_string(),
            name: self.call.name.to_string(),
            line: self.position.line(),
            column: self.position.position(),
        }
    }
}

fn literal_string(expression: &Expr) -> Option<String> {
    match expression {
        Expr::StringConstant(value, _) => Some(value.to_string()),
        _ => None,
    }
}

mod api {
    use super::*;

    pub(super) fn log(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        Ok(IrEmission {
            command: IrCommand::Log(call.one_string()?),
            wait: None,
        })
    }

    pub(super) fn stop_bgm(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        call.no_args()?;
        Ok(IrEmission {
            command: IrCommand::StopBgm,
            wait: None,
        })
    }

    pub(super) fn play_bgm(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        call.args(3)?;
        let fade_in_ms = call.float_at(2)?;
        if fade_in_ms < 0.0 {
            return Err(call.invalid("fade duration must not be negative"));
        }
        Ok(IrEmission {
            command: IrCommand::PlayBgm {
                path: call.string_at(0)?,
                volume: call.float_at(1)?,
                fade_in_ms: Some(fade_in_ms.round() as u64),
            },
            wait: None,
        })
    }

    pub(super) fn quit(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        call.no_args()?;
        Ok(IrEmission {
            command: IrCommand::Exit,
            wait: None,
        })
    }

    pub(super) fn choice(call: RhaiCall<'_>, result: String) -> Result<IrEmission, IrCompileError> {
        call.args(2)?;
        let options = call.choice_options(1)?;
        if options.is_empty() {
            return Err(call.invalid("choice requires at least one option"));
        }
        Ok(IrEmission {
            command: IrCommand::Choose {
                prompt: call.string_at(0)?,
                options,
                result,
            },
            wait: Some(IrWaitKind::ScreenChoice),
        })
    }

    pub(super) fn open_ui(
        call: RhaiCall<'_>,
        result: String,
    ) -> Result<IrEmission, IrCompileError> {
        Ok(IrEmission {
            command: IrCommand::OpenUi {
                path: call.one_string()?,
                result,
            },
            wait: Some(IrWaitKind::UiIntent),
        })
    }

    pub(super) fn clear_text(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        call.no_args()?;
        Ok(IrEmission {
            command: IrCommand::ClearDialogue,
            wait: None,
        })
    }

    pub(super) fn return_to_title(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        call.no_args()?;
        Ok(IrEmission {
            command: IrCommand::ReturnToTitle,
            wait: None,
        })
    }

    pub(super) fn load_script(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        Ok(IrEmission {
            command: IrCommand::LoadScript {
                path: call.one_string()?,
            },
            wait: None,
        })
    }

    pub(super) fn narrate(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        Ok(IrEmission {
            command: IrCommand::Say {
                speaker: String::new(),
                text: call.one_string()?,
            },
            wait: Some(IrWaitKind::DialogueAdvance),
        })
    }

    pub(super) fn bg(call: RhaiCall<'_>) -> Result<IrEmission, IrCompileError> {
        Ok(IrEmission {
            command: IrCommand::SetBackground {
                texture: call.one_string()?,
            },
            wait: None,
        })
    }

    pub(super) fn char(
        flow: &mut CharacterFlowLowering,
        call: RhaiCall<'_>,
    ) -> Result<(), IrCompileError> {
        let actor_id = call.one_string()?;
        flow.actor_id = actor_id.clone();
        flow.character_name = actor_id;
        Ok(())
    }

    pub(super) fn at(
        flow: &mut CharacterFlowLowering,
        call: RhaiCall<'_>,
    ) -> Result<(), IrCompileError> {
        flow.position = match call.one_string()?.as_str() {
            "left" => [-600.0, 0.0],
            "center" => [0.0, 0.0],
            "right" => [600.0, 0.0],
            _ => return Err(call.invalid("position must be left, center, or right")),
        };
        Ok(())
    }

    pub(super) fn scale(
        flow: &mut CharacterFlowLowering,
        call: RhaiCall<'_>,
    ) -> Result<(), IrCompileError> {
        let scale = call.one_float()?;
        if scale <= 0.0 {
            return Err(call.invalid("scale must be positive"));
        }
        flow.scale = scale;
        Ok(())
    }

    pub(super) fn expression(
        flow: &mut CharacterFlowLowering,
        call: RhaiCall<'_>,
    ) -> Result<(), IrCompileError> {
        flow.expressions.push(call.one_string()?);
        Ok(())
    }
}

#[derive(Default)]
struct CharacterFlowLowering {
    actor_id: String,
    character_name: String,
    expressions: Vec<String>,
    position: [f32; 2],
    scale: f32,
}

#[derive(Default)]
struct CameraFlowLowering {
    blur: Option<f32>,
    zoom: Option<f32>,
    duration_ms: u64,
    ease: String,
}

fn source_hash(source: &str) -> u64 {
    source.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
    })
}

fn statement_kind(statement: &Stmt) -> &'static str {
    match statement {
        Stmt::Noop(_) => "noop",
        Stmt::If(_, _) => "if",
        Stmt::Switch(_, _) => "switch",
        Stmt::While(_, _) => "while/loop",
        Stmt::Do(_, _, _) => "do",
        Stmt::For(_, _) => "for",
        Stmt::Var(_, _, _) => "variable declaration",
        Stmt::Assignment(_) => "assignment",
        Stmt::FnCall(_, _) => "function call",
        Stmt::Block(_) => "block",
        Stmt::TryCatch(_, _) => "try/catch",
        Stmt::Expr(_) => "expression",
        Stmt::BreakLoop(_, _, _) => "break/continue",
        Stmt::Return(_, _, _) => "return/throw",
        Stmt::Import(_, _) => "import",
        Stmt::Export(_, _) => "export",
        Stmt::Share(_) => "closure capture",
        _ => "unknown statement",
    }
}

fn expression_kind(expression: &Expr) -> &'static str {
    match expression {
        Expr::BoolConstant(_, _) => "boolean expression",
        Expr::Variable(_, _, _) => "variable expression",
        Expr::FnCall(_, _) | Expr::MethodCall(_, _) => "function expression",
        Expr::And(_, _) | Expr::Or(_, _) | Expr::Coalesce(_, _) => "logical expression",
        _ => "expression",
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IrCompileError {
    #[error("failed to parse `{path}`: {message}")]
    Parse { path: String, message: String },
    #[error("unsupported Rhai statement `{kind}` in `{path}` at {line:?}:{column:?}")]
    UnsupportedStatement {
        path: String,
        kind: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("unsupported Rhai expression `{kind}` in `{path}` at {line:?}:{column:?}")]
    UnsupportedExpression {
        path: String,
        kind: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("unsupported Rhai call `{name}` in `{path}` at {line:?}:{column:?}")]
    UnsupportedCall {
        path: String,
        name: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("invalid Rhai call `{name}` in `{path}` at {line:?}:{column:?}: {message}")]
    InvalidCall {
        path: String,
        name: String,
        message: String,
        line: Option<usize>,
        column: Option<usize>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::ir::{IrEvent, IrVm};

    #[test]
    fn compiles_supported_commands_and_literal_if() {
        let program = compile_to_ir(
            "test.rhai",
            "log(\"start\"); if true { narrate(\"yes\"); } else { narrate(\"no\"); } clear_text(); bg(\"bg/opening\");",
        )
        .unwrap();
        assert_eq!(program.expressions, vec![IrExpression::BoolLiteral(true)]);
        assert_eq!(
            program.instructions,
            vec![
                IrInstruction::Emit(IrCommand::Log("start".to_string())),
                IrInstruction::Branch {
                    expression: IrExpressionId(0),
                    then_pc: 2,
                    else_pc: 5,
                },
                IrInstruction::Emit(IrCommand::Say {
                    speaker: String::new(),
                    text: "yes".to_string(),
                }),
                IrInstruction::Wait(IrWaitKind::DialogueAdvance),
                IrInstruction::Jump(7),
                IrInstruction::Emit(IrCommand::Say {
                    speaker: String::new(),
                    text: "no".to_string(),
                }),
                IrInstruction::Wait(IrWaitKind::DialogueAdvance),
                IrInstruction::Emit(IrCommand::ClearDialogue),
                IrInstruction::Emit(IrCommand::SetBackground {
                    texture: "bg/opening".to_string(),
                }),
                IrInstruction::Halt,
            ]
        );
    }

    #[test]
    fn compiles_title_runtime_audio_and_camera_commands() {
        let program = compile_to_ir(
            "test.rhai",
            r#"
                camera.blur(12);
                camera.zoom(1.05);
                camera.zoom(1).duration(1.5).ease("ease_out");
                play_bgm("title", 0.8, 800);
                quit();
            "#,
        )
        .unwrap();
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::SetCamera {
                    blur: Some(12.0),
                    ..
                })
            )
        }));
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::PlayBgm { path, volume, fade_in_ms: Some(800) })
                    if path == "title" && (*volume - 0.8).abs() < f32::EPSILON
            )
        }));
        assert!(
            program
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, IrInstruction::Emit(IrCommand::Exit)))
        );
    }

    #[test]
    fn compiles_choice_and_ui_results_into_string_branches() {
        let program = compile_to_ir(
            "test.rhai",
            r#"
                let action = choice("Menu", [#{ text: "Back", value: "back" }]);
                if action == "back" { quit(); }
                let settings = open_ui("ui/settings.ui.rhai");
                if settings == "close" { return_to_title(); }
            "#,
        )
        .unwrap();

        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::Choose { result, .. }) if result == "action"
            )
        }));
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::OpenUi { path, result })
                    if path == "ui/settings.ui.rhai" && result == "settings"
            )
        }));
        assert_eq!(
            program
                .expressions
                .iter()
                .filter(|expression| matches!(expression, IrExpression::StringEquals { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn rejects_dynamic_conditions_without_executing_them() {
        let error =
            compile_to_ir("test.rhai", "if is_unlocked() { narrate(\"yes\"); }").unwrap_err();
        assert!(matches!(
            error,
            IrCompileError::UnsupportedExpression { .. }
        ));
    }

    #[test]
    fn lowers_while_loop_break_and_continue_to_valid_jumps() {
        let while_program =
            compile_to_ir("test.rhai", "while ready { if false { continue; } break; }").unwrap();
        assert!(while_program.validate().is_ok());

        let loop_program = compile_to_ir("test.rhai", "loop { break; }").unwrap();
        assert!(loop_program.validate().is_ok());
    }

    #[test]
    fn evaluates_a_boolean_variable_when_the_vm_reaches_a_branch() {
        let program = compile_to_ir("test.rhai", "if ready { log(\"yes\"); }").unwrap();
        let mut vm = IrVm::new(program).unwrap();
        vm.set_bool_variable("ready", true);
        assert_eq!(
            vm.step(),
            Some(IrEvent::Command(IrCommand::Log("yes".to_string())))
        );
    }

    #[test]
    fn current_new_game_script_is_ready_for_ir_handoff() {
        let source =
            include_str!("../../../../manosabars/assets/main_hdp_contents/scripts/new_game.rhai");
        let program = compile_to_ir("scripts/new_game.rhai", source).unwrap();
        assert!(program.validate().is_ok());
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::ShowCharacter { .. })
            )
        }));
    }

    #[test]
    fn each_character_statement_commits_without_finish() {
        let program = compile_to_ir(
            "test.rhai",
            r#"char("alice").e("hand"); char("bob").e("hand");"#,
        )
        .unwrap();
        assert_eq!(
            program
                .instructions
                .iter()
                .filter(|instruction| {
                    matches!(
                        instruction,
                        IrInstruction::Emit(IrCommand::ShowCharacter { .. })
                    )
                })
                .count(),
            2
        );
    }

    #[test]
    fn current_gallery_script_is_ready_for_ir_handoff() {
        let source =
            include_str!("../../../../manosabars/assets/main_hdp_contents/scripts/gallery.rhai");
        let program = compile_to_ir("scripts/gallery.rhai", source).unwrap();
        assert!(program.instructions.iter().any(|instruction| {
            matches!(instruction, IrInstruction::Emit(IrCommand::ReturnToTitle))
        }));
    }

    #[test]
    fn current_startup_script_is_ready_for_ir_boot() {
        let source = include_str!("../../../../manosabars/assets/main_hdp_contents/startup.rhai");
        let program = compile_to_ir("startup.rhai", source).unwrap();
        assert!(program.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IrInstruction::Emit(IrCommand::LoadScript { path }) if path == "system.rhai"
            )
        }));
    }

    #[test]
    fn finish_is_not_an_ir_api() {
        let error = compile_to_ir("test.rhai", r#"char("alice").finish();"#).unwrap_err();
        assert!(matches!(
            error,
            IrCompileError::UnsupportedCall { name, .. } if name == "finish"
        ));
    }
}
