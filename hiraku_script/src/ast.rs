//! Syntax tree produced by [`crate::parse`].

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub statements: Vec<Stmt>,
    pub warnings: Vec<SyntaxWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxWarning {
    pub message: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolBound {
    pub parameter: String,
    pub protocol: TypeExpr,
}

/// Explicit normalized evidence parameters, generated from source bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolWitness {
    pub parameter: String,
    pub protocol: String,
    pub method: String,
    pub parameters: Vec<TypeExpr>,
    pub result: TypeExpr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Const {
        exported: bool,
        name: String,
        type_annotation: Option<TypeExpr>,
        value: Expr,
        span: Span,
    },
    Property {
        name: String,
        ty: TypeExpr,
        getter: Block,
        setter: Option<(String, Block)>,
        span: Span,
    },
    Extend {
        target: TypeExpr,
        protocol: Option<TypeExpr>,
        methods: Vec<Stmt>,
        span: Span,
    },
    Protocol {
        name: String,
        type_parameters: Vec<String>,
        associated_types: Vec<String>,
        methods: Vec<Stmt>,
        span: Span,
    },
    Import {
        path: Vec<String>,
        wildcard: bool,
        span: Span,
    },
    /// A nominal record declaration; unlike a transparent type alias its
    /// identity is the declaring module and name.
    Enum {
        name: String,
        type_parameters: Vec<String>,
        variants: Vec<EnumVariant>,
        span: Span,
    },
    Struct {
        name: String,
        type_parameters: Vec<String>,
        ty: TypeExpr,
        span: Span,
    },
    TypeAlias {
        name: String,
        type_parameters: Vec<String>,
        ty: TypeExpr,
        span: Span,
    },
    Function {
        /// Set only when the compiler injects its bundled standard library.
        /// Source syntax cannot request this capability.
        compiler_intrinsics: bool,
        attributes: Vec<Attribute>,
        /// Exported functions participate in runtime linking across scripts.
        exported: bool,
        name: String,
        type_parameters: Vec<String>,
        bounds: Vec<ProtocolBound>,
        witnesses: Vec<ProtocolWitness>,
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
        mutable: bool,
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
pub struct WhenArm {
    pub variant: String,
    pub bindings: Vec<String>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
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
    Unit,
    Tuple(Vec<TypeExpr>),
    Function {
        parameters: Vec<TypeExpr>,
        result: Box<TypeExpr>,
    },
    Named(String),
    Applied {
        name: String,
        arguments: Vec<TypeExpr>,
    },
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
    When {
        value: Box<Expr>,
        arms: Vec<WhenArm>,
    },
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
    Not(Box<Expr>),
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
    Cast {
        value: Box<Expr>,
        ty: TypeExpr,
        mode: CastMode,
    },
    Call {
        callee: Box<Expr>,
        type_arguments: Vec<TypeExpr>,
        arguments: Vec<Argument>,
        trailing_block: Option<Block>,
    },
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    /// A structural object literal. Its type is anonymous unless a target type
    /// context supplies a nominal struct or `Map<String, V>`.
    StructLiteral(Vec<MapField>),
    TypedStructLiteral {
        type_name: String,
        fields: Vec<MapField>,
    },
    Lambda {
        parameters: Vec<FunctionParameter>,
        body: Block,
    },
    Block(Block),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CastMode {
    /// A cast whose validity must be proven from static types.
    Static,
    /// A checked runtime cast which produces `Optional<T>`.
    Optional,
    /// A checked runtime cast which raises an error on failure.
    Forced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BinaryOp {
    And,
    Or,
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
