//! Syntax tree produced by [`crate::parse`].

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub statements: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Function {
        name: String,
        parameters: Vec<String>,
        body: Block,
        span: Span,
    },
    Let {
        mutable: bool,
        name: String,
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
    UnaryMinus(Box<Expr>),
    Member {
        object: Box<Expr>,
        name: String,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
        trailing_block: Option<Block>,
    },
    Tuple(Vec<Expr>),
    Map(Vec<MapField>),
    Block(Block),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Equal,
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
