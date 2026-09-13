//! High-level statement semantics shared by the compiler and embedding host.

mod prepare;
mod typed;
mod types;

pub use typed::*;
pub use types::{ScriptType, TypeId, TypeTable};

/// An embedding-owned, typed compiler pass. No embedding concepts belong in
/// the parser or VM. Allocated nodes must live in the compilation's arena.
pub trait HirPass {
    fn begin_module(&mut self, _path: &str) {}
    fn run<'hir>(
        &mut self,
        arena: &'hir HirArena,
        program: &mut HirProgram<'hir>,
        manifest: &crate::BuiltinManifest,
    ) -> Result<(), Vec<crate::CompileError>>;
}

use serde::{Deserialize, Serialize};

use crate::{
    ast::{Block, Expr, ExprKind, Stmt, TypeExpr, TypeExprKind},
    runtime::Value,
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
        Stmt::Return { value, .. } => {
            if let Some(value) = value {
                intern_expression(value, symbols);
            }
        }
        Stmt::Property {
            name,
            ty,
            getter,
            setter,
            ..
        } => {
            symbols.intern(name);
            intern_type(ty, symbols);
            intern_block(getter, symbols);
            if let Some((parameter, body)) = setter {
                symbols.intern(parameter);
                intern_block(body, symbols);
            }
        }
        Stmt::Impl {
            target, methods, ..
        } => {
            intern_type(target, symbols);
            for method in methods {
                intern_statement(method, symbols);
            }
        }
        Stmt::Import { path, .. } => {
            if !path.is_empty() {
                symbols.intern(path.join("."));
            }
        }
        Stmt::Enum {
            name,
            type_parameters,
            variants,
            ..
        } => {
            symbols.intern(name);
            for parameter in type_parameters {
                symbols.intern(parameter);
            }
            for variant in variants {
                symbols.intern(&variant.name);
                for field in &variant.fields {
                    intern_type(field, symbols);
                }
            }
        }
        Stmt::TypeAlias {
            name,
            type_parameters,
            ty,
            ..
        }
        | Stmt::Struct {
            name,
            type_parameters,
            ty,
            ..
        } => {
            symbols.intern(name);
            for parameter in type_parameters {
                symbols.intern(parameter);
            }
            intern_type(ty, symbols);
        }
        Stmt::Function {
            name,
            type_parameters,
            parameters,
            return_type,
            body,
            ..
        } => {
            symbols.intern(name);
            for parameter in type_parameters {
                symbols.intern(parameter);
            }
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
        Stmt::Const {
            name,
            type_annotation,
            value,
            ..
        }
        | Stmt::Let {
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
        TypeExprKind::Unit => {}
        TypeExprKind::Tuple(values) => {
            for value in values {
                intern_type(value, symbols);
            }
        }
        TypeExprKind::Function { parameters, result } => {
            for parameter in parameters {
                intern_type(parameter, symbols);
            }
            intern_type(result, symbols);
        }
        TypeExprKind::Named(name) => {
            symbols.intern(name);
        }
        TypeExprKind::Applied { name, arguments } => {
            symbols.intern(name);
            for argument in arguments {
                intern_type(argument, symbols);
            }
        }
        TypeExprKind::Nullable(inner)
        | TypeExprKind::List(inner)
        | TypeExprKind::Binding(inner) => intern_type(inner, symbols),
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
        ExprKind::When { value, arms } => {
            intern_expression(value, symbols);
            for arm in arms {
                symbols.intern(&arm.variant);
                for name in &arm.bindings {
                    symbols.intern(name);
                }
                intern_block(&arm.body, symbols);
            }
        }
        ExprKind::Ident(name) | ExprKind::Symbol(name) => {
            symbols.intern(name);
        }
        ExprKind::Member { object, name } | ExprKind::SafeMember { object, name } => {
            intern_expression(object, symbols);
            symbols.intern(name);
        }
        ExprKind::Binding(value)
        | ExprKind::Not(value)
        | ExprKind::UnaryMinus(value)
        | ExprKind::NonNull(value) => intern_expression(value, symbols),
        ExprKind::Cast { value, ty, .. } => {
            intern_expression(value, symbols);
            intern_type(ty, symbols);
        }
        ExprKind::Elvis { value, fallback } => {
            intern_expression(value, symbols);
            intern_expression(fallback, symbols);
        }
        ExprKind::Call {
            callee,
            type_arguments,
            arguments,
            trailing_block,
        } => {
            intern_expression(callee, symbols);
            for argument in type_arguments {
                intern_type(argument, symbols);
            }
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
        ExprKind::StructLiteral(fields) => {
            for field in fields {
                symbols.intern(&field.name);
                intern_expression(&field.value, symbols);
            }
        }
        ExprKind::TypedStructLiteral { type_name, fields } => {
            symbols.intern(type_name);
            for field in fields {
                symbols.intern(&field.name);
                intern_expression(&field.value, symbols);
            }
        }
        ExprKind::Lambda { parameters, body } => {
            for parameter in parameters {
                symbols.intern(&parameter.name);
                if let Some(ty) = &parameter.ty {
                    intern_type(ty, symbols);
                }
            }
            intern_block(body, symbols);
        }
        ExprKind::Block(block) => intern_block(block, symbols),
        ExprKind::Binary { left, right, .. } => {
            intern_expression(left, symbols);
            intern_expression(right, symbols);
        }
        ExprKind::String(text) => {
            let mut remaining = text.as_str();
            while let Some(start) = remaining.find("${") {
                let source = &remaining[start + 2..];
                let Some(end) = crate::template::expression_end(source) else {
                    break;
                };
                if let Ok(program) =
                    crate::parse::parse_interpolation(&source[..end], expression.span)
                {
                    for statement in &program.statements {
                        if let Stmt::Expr(value) = statement {
                            intern_expression(value, symbols);
                        }
                    }
                }
                remaining = &source[end + 1..];
            }
        }
        ExprKind::Unit
        | ExprKind::Null
        | ExprKind::Ellipsis
        | ExprKind::Bool(_)
        | ExprKind::Number { .. } => {}
    }
}

/// A statement boundary yielded to the embedding host.
///
/// This is deliberately engine-agnostic. An embedding may interpret a string as
/// narration, a console line, or something else entirely.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StatementValue {
    Commit,
    String(String),
    /// Raw localizable text plus `${...}` expressions. Embeddings may translate
    /// this source before asking the VM to evaluate it.
    TextTemplate(String),
    /// A non-string expression statement exposed to the embedding host.
    ///
    /// Story embeddings may treat this exactly like [`Commit`](Self::Commit),
    /// while declarative embeddings can collect typed values such as UI nodes.
    Value(Value),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_interns_nested_selectors_and_bindings_once() {
        let program = crate::parse_program(
            "global var player = .{ stats: .{ health: 1 } }\nplayer.stats.health = 2",
        )
        .expect("source parses");
        let symbols = normalize_program_symbols(&program, None);
        assert_eq!(symbols.find("player"), Some(crate::SymbolId(0)));
        assert_eq!(symbols.find("stats"), Some(crate::SymbolId(1)));
        assert_eq!(symbols.find("health"), Some(crate::SymbolId(2)));
        assert_eq!(symbols.symbols().len(), 3);
    }
}
