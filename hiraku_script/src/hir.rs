//! High-level statement semantics shared by the compiler and embedding host.

use serde::{Deserialize, Serialize};

use crate::ast::{ExprKind, Stmt};

/// A statement boundary yielded to the embedding host.
///
/// This is deliberately engine-agnostic. An embedding may interpret a string as
/// narration, a console line, or something else entirely.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatementValue {
    Commit,
    String(String),
}

pub(crate) fn lower_statement(statement: &Stmt) -> StatementValue {
    match statement {
        Stmt::Expr(expression) => match &expression.kind {
            ExprKind::String(value) => StatementValue::String(value.clone()),
            _ => StatementValue::Commit,
        },
        _ => StatementValue::Commit,
    }
}
