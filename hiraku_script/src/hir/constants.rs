//! Compile-time scalar constants. No native calls or mutable objects are evaluated.
use super::LoweringError;
use crate::{BinaryOp, Block, Expr, ExprKind, NumberUnit, Stmt};
use std::collections::BTreeMap;

type Constants = BTreeMap<String, Expr>;

pub(super) fn evaluate(expr: &Expr, constants: &Constants) -> Result<Expr, LoweringError> {
    let fail = || {
        LoweringError { message: "constant initializer must be a compile-time scalar expression; calls and mutable objects are not constant".into(), span: expr.span }
    };
    let kind = match &expr.kind {
        ExprKind::Unit | ExprKind::Bool(_) | ExprKind::Number { .. } => expr.kind.clone(),
        ExprKind::String(value) if !value.contains("${") => expr.kind.clone(),
        ExprKind::Ident(name) => constants.get(name).ok_or_else(fail)?.kind.clone(),
        ExprKind::UnaryMinus(value) => match evaluate(value, constants)?.kind {
            ExprKind::Number { value, unit } => ExprKind::Number {
                value: -value,
                unit,
            },
            _ => return Err(fail()),
        },
        ExprKind::Binary { left, op, right } => {
            let left = evaluate(left, constants)?.kind;
            let right = evaluate(right, constants)?.kind;
            match (left, right) {
                (
                    ExprKind::Number {
                        value: a,
                        unit: NumberUnit::Scalar,
                    },
                    ExprKind::Number {
                        value: b,
                        unit: NumberUnit::Scalar,
                    },
                ) => {
                    let value = match op {
                        BinaryOp::Add => a + b,
                        BinaryOp::Subtract => a - b,
                        BinaryOp::Multiply => a * b,
                        BinaryOp::Divide if b != 0.0 => a / b,
                        BinaryOp::Equal => {
                            return Ok(Expr {
                                kind: ExprKind::Bool(a == b),
                                span: expr.span,
                            });
                        }
                        BinaryOp::Less => {
                            return Ok(Expr {
                                kind: ExprKind::Bool(a < b),
                                span: expr.span,
                            });
                        }
                        _ => return Err(fail()),
                    };
                    if !value.is_finite() {
                        return Err(fail());
                    }
                    ExprKind::Number {
                        value,
                        unit: NumberUnit::Scalar,
                    }
                }
                (ExprKind::String(a), ExprKind::String(b)) if *op == BinaryOp::Add => {
                    ExprKind::String(a + &b)
                }
                _ => return Err(fail()),
            }
        }
        _ => return Err(fail()),
    };
    Ok(Expr {
        kind,
        span: expr.span,
    })
}

pub(super) fn normalize(statements: &mut [Stmt]) -> Result<(), Vec<LoweringError>> {
    statements_in_scope(statements, &mut Constants::new()).map_err(|error| vec![error])
}

fn block(block: &mut Block, constants: &Constants) -> Result<(), LoweringError> {
    statements_in_scope(&mut block.statements, &mut constants.clone())
}

fn statements_in_scope(
    statements: &mut [Stmt],
    constants: &mut Constants,
) -> Result<(), LoweringError> {
    for statement in statements {
        match statement {
            Stmt::Const {
                exported,
                name,
                type_annotation,
                value,
                span,
            } => {
                let value = evaluate(value, constants)?;
                constants.insert(name.clone(), value.clone());
                *statement = if *exported {
                    Stmt::Global {
                        mutable: false,
                        name: name.clone(),
                        type_annotation: type_annotation.clone(),
                        value: Some(value),
                        span: *span,
                    }
                } else {
                    Stmt::Let {
                        mutable: false,
                        name: name.clone(),
                        type_annotation: type_annotation.clone(),
                        value,
                        span: *span,
                    }
                };
            }
            Stmt::Let { name, value, .. }
            | Stmt::Global {
                name,
                value: Some(value),
                ..
            } => {
                expression(value, constants)?;
                constants.remove(name);
            }
            Stmt::Function {
                parameters, body, ..
            } => {
                let mut scope = constants.clone();
                for parameter in parameters {
                    scope.remove(&parameter.name);
                }
                block(body, &scope)?;
            }
            Stmt::Expr(value) => expression(value, constants)?,
            Stmt::Assign { target, value, .. } => {
                expression(target, constants)?;
                expression(value, constants)?;
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
                ..
            } => {
                expression(condition, constants)?;
                block(then_block, constants)?;
                if let Some(body) = else_block {
                    block(body, constants)?;
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                expression(condition, constants)?;
                block(body, constants)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn expression(expr: &mut Expr, constants: &Constants) -> Result<(), LoweringError> {
    match &mut expr.kind {
        ExprKind::Block(body) => block(body, constants)?,
        ExprKind::Lambda { parameters, body } => {
            let mut scope = constants.clone();
            for parameter in parameters {
                scope.remove(&parameter.name);
            }
            block(body, &scope)?;
        }
        ExprKind::Call {
            callee,
            arguments,
            trailing_block,
            ..
        } => {
            expression(callee, constants)?;
            for argument in arguments {
                expression(&mut argument.value, constants)?;
            }
            if let Some(body) = trailing_block {
                block(body, constants)?;
            }
        }
        ExprKind::Member { object, .. } | ExprKind::SafeMember { object, .. } => {
            expression(object, constants)?
        }
        ExprKind::UnaryMinus(value)
        | ExprKind::Binding(value)
        | ExprKind::NonNull(value)
        | ExprKind::Cast { value, .. } => expression(value, constants)?,
        ExprKind::Binary { left, right, .. } => {
            expression(left, constants)?;
            expression(right, constants)?;
        }
        ExprKind::Elvis { value, fallback } => {
            expression(value, constants)?;
            expression(fallback, constants)?;
        }
        ExprKind::Tuple(values) | ExprKind::List(values) => {
            for value in values {
                expression(value, constants)?;
            }
        }
        ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => {
            for field in fields {
                expression(&mut field.value, constants)?;
            }
        }
        _ => {}
    }
    Ok(())
}
