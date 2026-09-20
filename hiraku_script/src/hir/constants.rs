//! Constant expressions for extension `let` members. No host calls are evaluated.
use super::LoweringError;
use crate::{BinaryOp, Expr, ExprKind, NumberUnit};
use std::collections::BTreeMap;

type Constants = BTreeMap<String, Expr>;

pub(super) fn evaluate(expr: &Expr, constants: &Constants) -> Result<Expr, LoweringError> {
    evaluate_scalar(expr, constants, true)
}

// Skipped logical operands still have to be well-typed constant expressions,
// but must not execute arithmetic (for example, division by zero).
fn evaluate_scalar(
    expr: &Expr,
    constants: &Constants,
    execute: bool,
) -> Result<Expr, LoweringError> {
    let fail = || {
        LoweringError { message: "extension let initializer must be a constant expression; use var with get() for runtime computation".into(), span: expr.span }
    };
    let kind = match &expr.kind {
        ExprKind::Unit
        | ExprKind::Null
        | ExprKind::Bool(_)
        | ExprKind::Number { .. }
        | ExprKind::Integer(_) => expr.kind.clone(),
        ExprKind::Tuple(values) | ExprKind::List(values) => {
            let values = values
                .iter()
                .map(|value| evaluate_scalar(value, constants, execute))
                .collect::<Result<Vec<_>, _>>()?;
            if matches!(expr.kind, ExprKind::Tuple(_)) {
                ExprKind::Tuple(values)
            } else {
                ExprKind::List(values)
            }
        }
        ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => {
            let mut fields = fields.clone();
            for field in &mut fields {
                field.value = evaluate_scalar(&field.value, constants, execute)?;
            }
            match &expr.kind {
                ExprKind::TypedStructLiteral { type_name, .. } => ExprKind::TypedStructLiteral {
                    type_name: type_name.clone(),
                    fields,
                },
                _ => ExprKind::StructLiteral(fields),
            }
        }
        ExprKind::String(value) if !value.contains("${") => expr.kind.clone(),
        ExprKind::Ident(name) => constants.get(name).ok_or_else(fail)?.kind.clone(),
        ExprKind::Not(value) => match evaluate_scalar(value, constants, execute)?.kind {
            ExprKind::Bool(value) => ExprKind::Bool(!value),
            _ => return Err(fail()),
        },
        ExprKind::UnaryMinus(value) => match evaluate_scalar(value, constants, execute)?.kind {
            ExprKind::Integer(value) if value <= 1u64 << 63 => {
                ExprKind::UnaryMinus(Box::new(Expr {
                    kind: ExprKind::Integer(value),
                    span: expr.span,
                }))
            }
            ExprKind::Number { value, unit } => ExprKind::Number {
                value: -value,
                unit,
            },
            _ => return Err(fail()),
        },
        ExprKind::Binary { left, op, right } => {
            let left = evaluate_scalar(left, constants, execute)?.kind;
            let skipped = matches!(
                (&left, op),
                (ExprKind::Bool(false), BinaryOp::And) | (ExprKind::Bool(true), BinaryOp::Or)
            );
            let right = evaluate_scalar(right, constants, execute && !skipped)?.kind;
            let integer = |kind: &ExprKind| match kind {
                ExprKind::Integer(n) => Some(i128::from(*n)),
                ExprKind::UnaryMinus(value) => match value.kind {
                    ExprKind::Integer(n) => Some(-i128::from(n)),
                    _ => None,
                },
                _ => None,
            };
            if let (Some(a), Some(b)) = (integer(&left), integer(&right)) {
                let boolean = match op {
                    BinaryOp::Equal => Some(a == b),
                    BinaryOp::NotEqual => Some(a != b),
                    BinaryOp::Less => Some(a < b),
                    BinaryOp::LessEqual => Some(a <= b),
                    BinaryOp::Greater => Some(a > b),
                    BinaryOp::GreaterEqual => Some(a >= b),
                    _ => None,
                };
                let kind = if let Some(value) = boolean {
                    ExprKind::Bool(value)
                } else {
                    let n = if !execute {
                        0
                    } else {
                        match op {
                            BinaryOp::Add => a.checked_add(b),
                            BinaryOp::Subtract => a.checked_sub(b),
                            BinaryOp::Multiply => a.checked_mul(b),
                            BinaryOp::Divide => a.checked_div(b),
                            _ => None,
                        }
                        .ok_or_else(fail)?
                    };
                    if n < i64::MIN as i128 || n > u64::MAX as i128 {
                        return Err(fail());
                    }
                    if n < 0 {
                        ExprKind::UnaryMinus(Box::new(Expr {
                            kind: ExprKind::Integer((-n) as u64),
                            span: expr.span,
                        }))
                    } else {
                        ExprKind::Integer(n as u64)
                    }
                };
                return Ok(Expr {
                    kind,
                    span: expr.span,
                });
            }
            match (left, right) {
                (ExprKind::Bool(a), ExprKind::Bool(b)) => ExprKind::Bool(match op {
                    BinaryOp::And => a && b,
                    BinaryOp::Or => a || b,
                    BinaryOp::Equal => a == b,
                    BinaryOp::NotEqual => a != b,
                    _ => return Err(fail()),
                }),
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
                        BinaryOp::Add
                        | BinaryOp::Subtract
                        | BinaryOp::Multiply
                        | BinaryOp::Divide
                            if !execute =>
                        {
                            0.0
                        }
                        BinaryOp::Add => a + b,
                        BinaryOp::Subtract => a - b,
                        BinaryOp::Multiply => a * b,
                        BinaryOp::Divide if b != 0.0 => a / b,
                        BinaryOp::Equal
                        | BinaryOp::NotEqual
                        | BinaryOp::Less
                        | BinaryOp::LessEqual
                        | BinaryOp::Greater
                        | BinaryOp::GreaterEqual => {
                            return Ok(Expr {
                                kind: ExprKind::Bool(match op {
                                    BinaryOp::Equal => a == b,
                                    BinaryOp::NotEqual => a != b,
                                    BinaryOp::Less => a < b,
                                    BinaryOp::LessEqual => a <= b,
                                    BinaryOp::Greater => a > b,
                                    BinaryOp::GreaterEqual => a >= b,
                                    _ => unreachable!("comparison operator"),
                                }),
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
                (ExprKind::String(a), ExprKind::String(b))
                    if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) =>
                {
                    ExprKind::Bool((a == b) == (*op == BinaryOp::Equal))
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
