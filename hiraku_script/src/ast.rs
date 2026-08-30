//! Syntax tree produced by [`crate::parse`].

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub statements: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Import {
        path: Vec<String>,
        wildcard: bool,
        span: Span,
    },
    TypeAlias {
        name: String,
        ty: TypeExpr,
        span: Span,
    },
    Function {
        /// Exported functions participate in runtime linking across scripts.
        exported: bool,
        name: String,
        parameters: Vec<FunctionParameter>,
        return_type: Option<TypeExpr>,
        body: Block,
        span: Span,
    },
    Let {
        mutable: bool,
        name: String,
        type_annotation: Option<TypeExpr>,
        value: Expr,
        span: Span,
    },
    Global {
        name: String,
        type_annotation: Option<TypeExpr>,
        value: Option<Expr>,
        span: Span,
    },
    Assign {
        target: Expr,
        value: Expr,
        span: Span,
    },
    Expr(Expr),
    If {
        condition: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    While {
        condition: Expr,
        body: Block,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionParameter {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeExprKind {
    Named(String),
    Nullable(Box<TypeExpr>),
    List(Box<TypeExpr>),
    Binding(Box<TypeExpr>),
    Record(Vec<TypeField>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeField {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Unit,
    Null,
    Ellipsis,
    Ident(String),
    Symbol(String),
    Bool(bool),
    Number {
        value: f64,
        unit: NumberUnit,
    },
    String(String),
    /// An explicitly captured reactive expression, written `$name` or `${expr}`.
    Binding(Box<Expr>),
    UnaryMinus(Box<Expr>),
    Member {
        object: Box<Expr>,
        name: String,
    },
    SafeMember {
        object: Box<Expr>,
        name: String,
    },
    Elvis {
        value: Box<Expr>,
        fallback: Box<Expr>,
    },
    NonNull(Box<Expr>),
    Call {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
        trailing_block: Option<Block>,
    },
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Map(Vec<MapField>),
    TypedMap {
        type_name: String,
        fields: Vec<MapField>,
    },
    Block(Block),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Colon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberUnit {
    Scalar,
    Percent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    pub label: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapField {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}
