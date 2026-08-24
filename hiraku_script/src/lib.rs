pub mod ast;
pub mod hir;
pub mod hson;
pub mod lex;
pub mod native;
pub mod parse;
pub mod span;
pub mod symbol;
pub mod template;
pub mod vm;

pub use ast::{
    Argument, BinaryOp, Block, Expr, ExprKind, FunctionParameter, MapField, NumberUnit, Program,
    Stmt, TypeExpr, TypeExprKind, TypeField,
};
pub use hir::StatementValue;
pub use hiraku_script_derive::{HksHandle, hks_define, hks_module};
pub use parse::{ParseError, parse_program};
pub use span::Span;
pub use symbol::{SymbolId, SymbolInterner, SymbolManifest};
pub use template::{TemplateCallArgument, TemplateContext, TemplateError, eval_template};
extern crate self as hiraku_script;
