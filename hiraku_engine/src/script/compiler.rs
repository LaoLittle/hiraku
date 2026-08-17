use rhai::{Engine, Expr, FnCallExpr, OptimizationLevel, Position, Stmt, StmtBlock};
use thiserror::Error;

use super::ir::{IrCommand, IrExpression, IrExpressionId, IrInstruction, IrProgram};

/// Compiles the deterministic command-only Rhai subset into Hiraku IR.
///
/// This intentionally rejects variables, closures, loops, and arbitrary calls
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
            Stmt::Expr(expression) => match expression.as_ref() {
                Expr::FnCall(call, position) => self.lower_command_call(call, *position),
                expression => Err(self.unsupported_expression(expression)),
            },
            Stmt::If(flow, _) => self.lower_if(flow),
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
            expression => return Err(self.unsupported_expression(expression)),
        };
        let id = IrExpressionId(self.expressions.len() as u32);
        self.expressions.push(expression);
        Ok(id)
    }

    fn lower_command_call(
        &mut self,
        call: &FnCallExpr,
        position: Position,
    ) -> Result<(), IrCompileError> {
        let command = match call.name.as_str() {
            "log" => IrCommand::Log(one_string_argument(call, self.path, position)?),
            "clear_text" => {
                no_arguments(call, self.path, position)?;
                IrCommand::ClearDialogue
            }
            "narrate" => IrCommand::Say {
                speaker: String::new(),
                text: one_string_argument(call, self.path, position)?,
            },
            "bg" => IrCommand::SetBackground {
                path: one_string_argument(call, self.path, position)?,
            },
            _ => {
                return Err(IrCompileError::UnsupportedCall {
                    path: self.path.to_string(),
                    name: call.name.to_string(),
                    line: position.line(),
                    column: position.position(),
                });
            }
        };
        self.instructions.push(IrInstruction::Emit(command));
        Ok(())
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

fn no_arguments(call: &FnCallExpr, path: &str, position: Position) -> Result<(), IrCompileError> {
    if call.args.is_empty() {
        Ok(())
    } else {
        Err(IrCompileError::InvalidCall {
            path: path.to_string(),
            name: call.name.to_string(),
            message: "does not accept arguments".to_string(),
            line: position.line(),
            column: position.position(),
        })
    }
}

fn one_string_argument(
    call: &FnCallExpr,
    path: &str,
    position: Position,
) -> Result<String, IrCompileError> {
    let Some(Expr::StringConstant(value, _)) = call.args.first() else {
        return Err(IrCompileError::InvalidCall {
            path: path.to_string(),
            name: call.name.to_string(),
            message: "requires one string literal argument".to_string(),
            line: position.line(),
            column: position.position(),
        });
    };
    if call.args.len() != 1 {
        return Err(IrCompileError::InvalidCall {
            path: path.to_string(),
            name: call.name.to_string(),
            message: "requires exactly one argument".to_string(),
            line: position.line(),
            column: position.position(),
        });
    }
    Ok(value.to_string())
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
                    else_pc: 4,
                },
                IrInstruction::Emit(IrCommand::Say {
                    speaker: String::new(),
                    text: "yes".to_string(),
                }),
                IrInstruction::Jump(5),
                IrInstruction::Emit(IrCommand::Say {
                    speaker: String::new(),
                    text: "no".to_string(),
                }),
                IrInstruction::Emit(IrCommand::ClearDialogue),
                IrInstruction::Emit(IrCommand::SetBackground {
                    path: "bg/opening".to_string(),
                }),
                IrInstruction::Halt,
            ]
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
}
