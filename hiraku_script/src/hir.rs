//! High-level statement semantics shared by the compiler and embedding host.

mod typed;
mod types;

pub use typed::*;
pub use types::{ScriptType, TypeId, TypeTable};

use serde::{Deserialize, Serialize};

use crate::{
    ast::{Block, Expr, ExprKind, Stmt, TypeExpr, TypeExprKind},
    symbol::{SymbolInterner, SymbolManifest},
};

/// Interns every semantic name in lexical source order before bytecode
/// generation. The syntax AST intentionally retains source text for diagnostics;
/// all later executable representations use this canonical manifest.
pub fn normalize_program_symbols(
    program: &crate::Program,
    base: Option<&SymbolManifest>,
) -> SymbolManifest {
    let mut symbols = base
        .cloned()
        .map(SymbolInterner::from_manifest)
        .transpose()
        .expect("base symbol manifests are already validated")
        .unwrap_or_default();
    for statement in &program.statements {
        intern_statement(statement, &mut symbols);
    }
    symbols.manifest()
}

fn intern_statement(statement: &Stmt, symbols: &mut SymbolInterner) {
    match statement {
        Stmt::TypeAlias { name, ty, .. } => {
            symbols.intern(name);
            intern_type(ty, symbols);
        }
        Stmt::Function {
            name,
            parameters,
            return_type,
            body,
            ..
        } => {
            symbols.intern(name);
            for parameter in parameters {
                symbols.intern(&parameter.name);
                if let Some(ty) = &parameter.ty {
                    intern_type(ty, symbols);
                }
            }
            if let Some(ty) = return_type {
                intern_type(ty, symbols);
            }
            intern_block(body, symbols);
        }
        Stmt::Let {
            name,
            type_annotation,
            value,
            ..
        } => {
            symbols.intern(name);
            if let Some(ty) = type_annotation {
                intern_type(ty, symbols);
            }
            intern_expression(value, symbols);
        }
        Stmt::Global {
            name,
            type_annotation,
            value,
            ..
        } => {
            symbols.intern(name);
            if let Some(ty) = type_annotation {
                intern_type(ty, symbols);
            }
            if let Some(value) = value {
                intern_expression(value, symbols);
            }
        }
        Stmt::Assign { target, value, .. } => {
            intern_expression(target, symbols);
            intern_expression(value, symbols);
        }
        Stmt::Expr(expression) => intern_expression(expression, symbols),
        Stmt::If {
            condition,
            then_block,
            else_block,
            ..
        } => {
            intern_expression(condition, symbols);
            intern_block(then_block, symbols);
            if let Some(block) = else_block {
                intern_block(block, symbols);
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            intern_expression(condition, symbols);
            intern_block(body, symbols);
        }
    }
}

fn intern_block(block: &Block, symbols: &mut SymbolInterner) {
    for statement in &block.statements {
        intern_statement(statement, symbols);
    }
}

fn intern_type(ty: &TypeExpr, symbols: &mut SymbolInterner) {
    match &ty.kind {
        TypeExprKind::Named(name) => {
            symbols.intern(name);
        }
        TypeExprKind::Nullable(inner) | TypeExprKind::List(inner) => intern_type(inner, symbols),
        TypeExprKind::Record(fields) => {
            for field in fields {
                symbols.intern(&field.name);
                intern_type(&field.ty, symbols);
            }
        }
    }
}

fn intern_expression(expression: &Expr, symbols: &mut SymbolInterner) {
    match &expression.kind {
        ExprKind::Ident(name) | ExprKind::Symbol(name) => {
            symbols.intern(name);
        }
        ExprKind::Member { object, name } | ExprKind::SafeMember { object, name } => {
            intern_expression(object, symbols);
            symbols.intern(name);
        }
        ExprKind::UnaryMinus(value) | ExprKind::NonNull(value) => intern_expression(value, symbols),
        ExprKind::Elvis { value, fallback } => {
            intern_expression(value, symbols);
            intern_expression(fallback, symbols);
        }
        ExprKind::Call {
            callee,
            arguments,
            trailing_block,
        } => {
            intern_expression(callee, symbols);
            for argument in arguments {
                if let Some(label) = &argument.label {
                    symbols.intern(label);
                }
                intern_expression(&argument.value, symbols);
            }
            if let Some(block) = trailing_block {
                intern_block(block, symbols);
            }
        }
        ExprKind::Tuple(values) | ExprKind::List(values) => {
            for value in values {
                intern_expression(value, symbols);
            }
        }
        ExprKind::Map(fields) => {
            for field in fields {
                symbols.intern(&field.name);
                intern_expression(&field.value, symbols);
            }
        }
        ExprKind::TypedMap { type_name, fields } => {
            symbols.intern(type_name);
            for field in fields {
                symbols.intern(&field.name);
                intern_expression(&field.value, symbols);
            }
        }
        ExprKind::Block(block) => intern_block(block, symbols),
        ExprKind::Binary { left, right, .. } => {
            intern_expression(left, symbols);
            intern_expression(right, symbols);
        }
        ExprKind::Null
        | ExprKind::Ellipsis
        | ExprKind::Bool(_)
        | ExprKind::Number { .. }
        | ExprKind::String(_) => {}
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_interns_nested_selectors_and_bindings_once() {
        let program = crate::parse_program(
            "global player = .{ stats: .{ health: 1 } }\nplayer.stats.health = 2",
        )
        .expect("source parses");
        let symbols = normalize_program_symbols(&program, None);
        assert_eq!(symbols.find("player"), Some(crate::SymbolId(0)));
        assert_eq!(symbols.find("stats"), Some(crate::SymbolId(1)));
        assert_eq!(symbols.find("health"), Some(crate::SymbolId(2)));
        assert_eq!(symbols.symbols().len(), 3);
    }
}
