use std::collections::BTreeMap;

use bumpalo::Bump;

use crate::{
    BinaryOp, Block, Expr, ExprKind, NumberUnit, Program, Span, Stmt, SymbolId, SymbolInterner,
    SymbolManifest, TypeExpr, TypeExprKind,
    vm::{BuiltinId, BuiltinManifest},
};

use super::{ScriptType, TypeId, TypeTable, normalize_program_symbols};

macro_rules! hir_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);
    };
}

hir_id!(HirLocalId);
hir_id!(HirGlobalId);
hir_id!(HirFunctionId);

/// Session-owned allocation arena for HIR nodes.
///
/// HIR references are valid for exactly the lifetime of this arena. Semantic
/// identities which survive HIR (symbols, locals, globals, functions and
/// types) remain compact IDs; tree edges are direct references.
#[derive(Default)]
pub struct HirArena {
    bump: Bump,
}

impl HirArena {
    pub fn new() -> Self {
        Self::default()
    }

    fn alloc<T>(&self, value: T) -> &T {
        self.bump.alloc(value)
    }

    fn alloc_slice_copy<T: Copy>(&self, values: &[T]) -> &[T] {
        self.bump.alloc_slice_copy(values)
    }

    fn alloc_str(&self, value: &str) -> &str {
        self.bump.alloc_str(value)
    }

    pub fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
    }
}

#[derive(Debug)]
pub struct HirProgram<'hir> {
    pub symbols: SymbolManifest,
    pub types: TypeTable,
    pub locals: &'hir [HirLocal],
    pub globals: &'hir [HirGlobal],
    pub functions: &'hir [HirFunction<'hir>],
    pub entry: &'hir HirBlock<'hir>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirExpr<'hir> {
    pub kind: HirExprKind<'hir>,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HirExprKind<'hir> {
    Literal(HirLiteral<'hir>),
    Local(HirLocalId),
    Global(HirGlobalId),
    Function(HirFunctionId),
    Builtin(BuiltinId),
    Selector(SymbolId),
    Symbol(SymbolId),
    Unresolved(SymbolId),
    UnaryMinus(&'hir HirExpr<'hir>),
    Member {
        object: &'hir HirExpr<'hir>,
        member: SymbolId,
        safe: bool,
    },
    Elvis {
        value: &'hir HirExpr<'hir>,
        fallback: &'hir HirExpr<'hir>,
    },
    NonNull(&'hir HirExpr<'hir>),
    Call {
        callee: &'hir HirExpr<'hir>,
        arguments: &'hir [HirArgument<'hir>],
        trailing_block: Option<&'hir HirBlock<'hir>>,
        function: ResolvedFunction,
    },
    Tuple(&'hir [&'hir HirExpr<'hir>]),
    List(&'hir [&'hir HirExpr<'hir>]),
    Map {
        type_name: Option<SymbolId>,
        fields: &'hir [(SymbolId, &'hir HirExpr<'hir>)],
    },
    Block(&'hir HirBlock<'hir>),
    Binary {
        left: &'hir HirExpr<'hir>,
        op: BinaryOp,
        right: &'hir HirExpr<'hir>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HirLiteral<'hir> {
    Null,
    Ellipsis,
    Bool(bool),
    Number { value: f64, unit: NumberUnit },
    String(&'hir str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedFunction {
    User(HirFunctionId),
    Builtin(BuiltinId),
    Syntax(SymbolId),
    Dynamic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirArgument<'hir> {
    pub label: Option<SymbolId>,
    pub value: &'hir HirExpr<'hir>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirStmt<'hir> {
    pub kind: HirStmtKind<'hir>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HirStmtKind<'hir> {
    Let {
        local: HirLocalId,
        value: &'hir HirExpr<'hir>,
    },
    Global {
        global: HirGlobalId,
        value: Option<&'hir HirExpr<'hir>>,
    },
    Assign {
        target: &'hir HirPlace<'hir>,
        value: &'hir HirExpr<'hir>,
    },
    Expr(&'hir HirExpr<'hir>),
    If {
        condition: &'hir HirExpr<'hir>,
        then_block: &'hir HirBlock<'hir>,
        else_block: Option<&'hir HirBlock<'hir>>,
    },
    While {
        condition: &'hir HirExpr<'hir>,
        body: &'hir HirBlock<'hir>,
    },
}

/// Recursive lvalue representation. `player.stats.health` is two nested
/// `Member` nodes, not a root string plus a flattened path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirPlace<'hir> {
    Local(HirLocalId),
    Global(HirGlobalId),
    Member {
        object: &'hir HirPlace<'hir>,
        member: SymbolId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirBlock<'hir> {
    pub statements: &'hir [&'hir HirStmt<'hir>],
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HirLocal {
    pub name: SymbolId,
    pub ty: TypeId,
    pub mutable: bool,
    pub owner: Option<HirFunctionId>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HirGlobal {
    pub name: SymbolId,
    pub ty: TypeId,
    pub embedding_owned: bool,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirFunction<'hir> {
    pub name: SymbolId,
    pub parameters: &'hir [HirLocalId],
    pub result: TypeId,
    pub body: &'hir HirBlock<'hir>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweringError {
    pub message: String,
    pub span: Span,
}

/// Lowers source syntax into arena-backed, typed HIR. Source literals are
/// copied into the same arena, so the AST may be dropped after this returns.
pub fn lower_to_hir<'hir>(
    arena: &'hir HirArena,
    program: &Program,
    manifest: Option<&BuiltinManifest>,
) -> Result<HirProgram<'hir>, Vec<LoweringError>> {
    Lowerer::new(arena, program, manifest).lower(program)
}

struct FunctionDeclaration {
    name: SymbolId,
    result: ScriptType,
    span: Span,
}

struct Lowerer<'hir, 'manifest> {
    arena: &'hir HirArena,
    manifest: Option<&'manifest BuiltinManifest>,
    symbols: SymbolInterner,
    types: TypeTable,
    locals: Vec<HirLocal>,
    globals: Vec<HirGlobal>,
    functions: Vec<FunctionDeclaration>,
    lowered_functions: Vec<HirFunction<'hir>>,
    scopes: Vec<BTreeMap<SymbolId, HirLocalId>>,
    global_names: BTreeMap<SymbolId, HirGlobalId>,
    function_names: BTreeMap<SymbolId, HirFunctionId>,
    aliases: BTreeMap<String, ScriptType>,
    current_function: Option<HirFunctionId>,
    errors: Vec<LoweringError>,
}

impl<'hir, 'manifest> Lowerer<'hir, 'manifest> {
    fn new(
        arena: &'hir HirArena,
        program: &Program,
        manifest: Option<&'manifest BuiltinManifest>,
    ) -> Self {
        let symbols = normalize_program_symbols(program, manifest.map(BuiltinManifest::symbols));
        Self {
            arena,
            manifest,
            symbols: SymbolInterner::from_manifest(symbols)
                .expect("normalized HIR symbols are unique"),
            types: TypeTable::default(),
            locals: Vec::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            lowered_functions: Vec::new(),
            scopes: vec![BTreeMap::new()],
            global_names: BTreeMap::new(),
            function_names: BTreeMap::new(),
            aliases: BTreeMap::new(),
            current_function: None,
            errors: Vec::new(),
        }
    }

    fn lower(mut self, source: &Program) -> Result<HirProgram<'hir>, Vec<LoweringError>> {
        self.declare_types(source);
        self.declare_globals(source);
        self.declare_functions(source);
        self.lower_functions(source);
        self.current_function = None;
        self.scopes.clear();
        self.scopes.push(BTreeMap::new());
        let entry = self.lower_statements(
            source.statements.iter().filter(|statement| {
                !matches!(statement, Stmt::TypeAlias { .. } | Stmt::Function { .. })
            }),
            Span::new(
                0,
                u32::try_from(source_end(source)).expect("source span exceeds u32 capacity"),
            ),
            false,
        );
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        let locals = self.arena.alloc_slice_copy(&self.locals);
        let globals = self.arena.alloc_slice_copy(&self.globals);
        let functions = self.arena.alloc_slice_copy(&self.lowered_functions);
        Ok(HirProgram {
            symbols: self.symbols.manifest(),
            types: self.types,
            locals,
            globals,
            functions,
            entry,
        })
    }

    fn symbol(&mut self, name: &str) -> SymbolId {
        self.symbols.intern(name)
    }

    fn declare_types(&mut self, program: &Program) {
        for statement in &program.statements {
            let Stmt::TypeAlias { name, ty, span } = statement else {
                continue;
            };
            if self.aliases.contains_key(name) {
                self.error(format!("type `{name}` is defined more than once"), *span);
            } else if let Some(ty) = self.type_from_ast(ty) {
                self.aliases.insert(name.clone(), ty);
            } else {
                self.error(format!("type `{name}` refers to an unknown type"), ty.span);
            }
        }
    }

    fn declare_globals(&mut self, program: &Program) {
        if let Some(manifest) = self.manifest {
            let globals = manifest.globals().clone();
            for (name, ty) in globals {
                self.push_global(&name, ty, true, None);
            }
        }
        for statement in &program.statements {
            let Stmt::Global {
                name,
                type_annotation,
                span,
                ..
            } = statement
            else {
                continue;
            };
            let symbol = self.symbol(name);
            if self.global_names.contains_key(&symbol) {
                self.error(format!("global `{name}` is defined more than once"), *span);
                continue;
            }
            let ty = type_annotation
                .as_ref()
                .and_then(|ty| self.type_from_ast(ty))
                .unwrap_or(ScriptType::Any);
            self.push_global(name, ty, false, Some(*span));
        }
    }

    fn push_global(
        &mut self,
        name: &str,
        ty: ScriptType,
        embedding_owned: bool,
        span: Option<Span>,
    ) {
        let name = self.symbol(name);
        let ty = self.types.intern(ty);
        let id = HirGlobalId(self.globals.len() as u32);
        self.globals.push(HirGlobal {
            name,
            ty,
            embedding_owned,
            span,
        });
        self.global_names.insert(name, id);
    }

    fn declare_functions(&mut self, program: &Program) {
        for statement in &program.statements {
            let Stmt::Function {
                name,
                return_type,
                span,
                ..
            } = statement
            else {
                continue;
            };
            let symbol = self.symbol(name);
            if self.function_names.contains_key(&symbol) {
                self.error(
                    format!("function `{name}` is defined more than once"),
                    *span,
                );
                continue;
            }
            let id = HirFunctionId(self.functions.len() as u32);
            let result = return_type
                .as_ref()
                .and_then(|ty| self.type_from_ast(ty))
                .unwrap_or(ScriptType::Any);
            self.functions.push(FunctionDeclaration {
                name: symbol,
                result,
                span: *span,
            });
            self.function_names.insert(symbol, id);
        }
    }

    fn lower_functions(&mut self, program: &Program) {
        for statement in &program.statements {
            let Stmt::Function {
                name,
                parameters,
                body,
                ..
            } = statement
            else {
                continue;
            };
            let symbol = self.symbol(name);
            let Some(function_id) = self.function_names.get(&symbol).copied() else {
                continue;
            };
            self.current_function = Some(function_id);
            self.scopes.clear();
            self.scopes.push(BTreeMap::new());
            let mut lowered_parameters = Vec::new();
            for parameter in parameters {
                let ty = parameter
                    .ty
                    .as_ref()
                    .and_then(|ty| self.type_from_ast(ty))
                    .unwrap_or(ScriptType::Any);
                lowered_parameters.push(self.declare_local(
                    &parameter.name,
                    ty,
                    false,
                    parameter.span,
                ));
            }
            let body = self.lower_block(body, false);
            let declaration = &self.functions[function_id.0 as usize];
            let parameters = self.arena.alloc_slice_copy(&lowered_parameters);
            let result = self.types.intern(declaration.result.clone());
            self.lowered_functions.push(HirFunction {
                name: declaration.name,
                parameters,
                result,
                body,
                span: declaration.span,
            });
        }
    }

    fn lower_block(&mut self, block: &Block, scoped: bool) -> &'hir HirBlock<'hir> {
        self.lower_statements(block.statements.iter(), block.span, scoped)
    }

    fn lower_statements<'source>(
        &mut self,
        statements: impl IntoIterator<Item = &'source Stmt>,
        span: Span,
        scoped: bool,
    ) -> &'hir HirBlock<'hir> {
        if scoped {
            self.scopes.push(BTreeMap::new());
        }
        let statements = statements
            .into_iter()
            .filter_map(|statement| self.lower_statement(statement))
            .collect::<Vec<_>>();
        if scoped {
            self.scopes.pop();
        }
        let statements = self.arena.alloc_slice_copy(&statements);
        self.arena.alloc(HirBlock { statements, span })
    }

    fn lower_statement(&mut self, statement: &Stmt) -> Option<&'hir HirStmt<'hir>> {
        let (kind, span) = match statement {
            Stmt::TypeAlias { .. } => return None,
            Stmt::Function { span, .. } => {
                self.error("nested function definitions are not supported", *span);
                return None;
            }
            Stmt::Let {
                mutable,
                name,
                type_annotation,
                value,
                span,
            } => {
                let value = self.lower_expression(value);
                let inferred = self.expression_type(value).clone();
                let ty = type_annotation
                    .as_ref()
                    .and_then(|ty| self.type_from_ast(ty))
                    .unwrap_or(inferred);
                let local = self.declare_local(name, ty, *mutable, *span);
                (HirStmtKind::Let { local, value }, *span)
            }
            Stmt::Global {
                name, value, span, ..
            } => {
                let symbol = self.symbol(name);
                let global = self.global_names.get(&symbol).copied();
                let value = value.as_ref().map(|value| self.lower_expression(value));
                let Some(global) = global else {
                    self.error(format!("unknown global `{name}`"), *span);
                    return None;
                };
                let any = self.any_type();
                if self.globals[global.0 as usize].ty == any
                    && let Some(value) = value
                {
                    self.globals[global.0 as usize].ty = value.ty;
                }
                (HirStmtKind::Global { global, value }, *span)
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let target = self.lower_place(target)?;
                let value = self.lower_expression(value);
                (HirStmtKind::Assign { target, value }, *span)
            }
            Stmt::Expr(expression) => {
                let value = self.lower_expression(expression);
                (HirStmtKind::Expr(value), expression.span)
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
                span,
            } => {
                let condition = self.lower_expression(condition);
                let then_block = self.lower_block(then_block, true);
                let else_block = else_block
                    .as_ref()
                    .map(|block| self.lower_block(block, true));
                (
                    HirStmtKind::If {
                        condition,
                        then_block,
                        else_block,
                    },
                    *span,
                )
            }
            Stmt::While {
                condition,
                body,
                span,
            } => {
                let condition = self.lower_expression(condition);
                let body = self.lower_block(body, true);
                (HirStmtKind::While { condition, body }, *span)
            }
        };
        Some(self.arena.alloc(HirStmt { kind, span }))
    }

    fn lower_expression(&mut self, expression: &Expr) -> &'hir HirExpr<'hir> {
        let (kind, ty) = match &expression.kind {
            ExprKind::Null => (HirExprKind::Literal(HirLiteral::Null), ScriptType::Any),
            ExprKind::Ellipsis => (HirExprKind::Literal(HirLiteral::Ellipsis), ScriptType::Any),
            ExprKind::Bool(value) => (
                HirExprKind::Literal(HirLiteral::Bool(*value)),
                ScriptType::Bool,
            ),
            ExprKind::Number { value, unit } => (
                HirExprKind::Literal(HirLiteral::Number {
                    value: *value,
                    unit: *unit,
                }),
                match unit {
                    NumberUnit::Percent => ScriptType::Percent,
                    NumberUnit::Scalar if value.fract() == 0.0 => ScriptType::Int,
                    NumberUnit::Scalar => ScriptType::Number,
                },
            ),
            ExprKind::String(value) => (
                HirExprKind::Literal(HirLiteral::String(self.arena.alloc_str(value))),
                ScriptType::String,
            ),
            ExprKind::Ident(name) => return self.lower_identifier(name, expression.span),
            ExprKind::Symbol(name) => {
                let symbol = self.symbol(name);
                let ty = self
                    .manifest
                    .and_then(|manifest| manifest.resolve_getter(name).ok())
                    .and_then(|member| {
                        self.manifest
                            .and_then(|manifest| manifest.signature(member.builtin))
                    })
                    .map(|signature| signature.result.clone())
                    .unwrap_or(ScriptType::Symbol);
                (HirExprKind::Symbol(symbol), ty)
            }
            ExprKind::UnaryMinus(value) => {
                let value = self.lower_expression(value);
                (
                    HirExprKind::UnaryMinus(value),
                    self.expression_type(value).clone(),
                )
            }
            ExprKind::Member { object, name } | ExprKind::SafeMember { object, name } => {
                if matches!(expression.kind, ExprKind::Member { .. })
                    && let Some(selector) = flatten_selector(expression)
                    && self
                        .manifest
                        .is_some_and(|manifest| manifest.has_selector(&selector))
                {
                    let selector = self.symbol(&selector);
                    return self.alloc_expression(
                        HirExprKind::Selector(selector),
                        ScriptType::Selector,
                        expression.span,
                    );
                }
                let object = self.lower_expression(object);
                let member = self.symbol(name);
                let safe = matches!(expression.kind, ExprKind::SafeMember { .. });
                let mut ty = member_type(self.expression_type(object), name);
                if safe {
                    ty = ScriptType::Nullable(Box::new(ty));
                }
                (
                    HirExprKind::Member {
                        object,
                        member,
                        safe,
                    },
                    ty,
                )
            }
            ExprKind::Elvis { value, fallback } => {
                let value = self.lower_expression(value);
                let fallback = self.lower_expression(fallback);
                let ty = match self.expression_type(value) {
                    ScriptType::Nullable(inner) => (**inner).clone(),
                    ScriptType::Any => self.expression_type(fallback).clone(),
                    ty => ty.clone(),
                };
                (HirExprKind::Elvis { value, fallback }, ty)
            }
            ExprKind::NonNull(value) => {
                let value = self.lower_expression(value);
                let ty = match self.expression_type(value) {
                    ScriptType::Nullable(inner) => (**inner).clone(),
                    ty => ty.clone(),
                };
                (HirExprKind::NonNull(value), ty)
            }
            ExprKind::Call {
                callee,
                arguments,
                trailing_block,
            } => {
                let callee = self.lower_expression(callee);
                let function = self.resolve_call(expression);
                let arguments = arguments
                    .iter()
                    .map(|argument| HirArgument {
                        label: argument.label.as_deref().map(|label| self.symbol(label)),
                        value: self.lower_expression(&argument.value),
                        span: argument.span,
                    })
                    .collect::<Vec<_>>();
                let arguments = self.arena.alloc_slice_copy(&arguments);
                let trailing_block = trailing_block
                    .as_ref()
                    .map(|block| self.lower_block(block, true));
                let ty = self.call_result(function);
                (
                    HirExprKind::Call {
                        callee,
                        arguments,
                        trailing_block,
                        function,
                    },
                    ty,
                )
            }
            ExprKind::Tuple(values) | ExprKind::List(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_expression(value))
                    .collect::<Vec<_>>();
                let ty = if matches!(expression.kind, ExprKind::Tuple(_)) {
                    ScriptType::Tuple
                } else {
                    let element = values
                        .first()
                        .map(|value| self.expression_type(value).clone())
                        .unwrap_or(ScriptType::Any);
                    let element = if values
                        .iter()
                        .all(|value| element.accepts(self.expression_type(value)))
                    {
                        element
                    } else {
                        ScriptType::Any
                    };
                    ScriptType::List(Box::new(element))
                };
                let values = self.arena.alloc_slice_copy(&values);
                if matches!(expression.kind, ExprKind::Tuple(_)) {
                    (HirExprKind::Tuple(values), ty)
                } else {
                    (HirExprKind::List(values), ty)
                }
            }
            ExprKind::Map(fields) | ExprKind::TypedMap { fields, .. } => {
                let type_name = match &expression.kind {
                    ExprKind::TypedMap { type_name, .. } => Some(self.symbol(type_name)),
                    _ => None,
                };
                let mut record = BTreeMap::new();
                let fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.symbol(&field.name);
                        let value = self.lower_expression(&field.value);
                        record.insert(field.name.clone(), self.expression_type(value).clone());
                        (name, value)
                    })
                    .collect::<Vec<_>>();
                let fields = self.arena.alloc_slice_copy(&fields);
                let ty = match &expression.kind {
                    ExprKind::TypedMap { type_name, .. } => self
                        .aliases
                        .get(type_name)
                        .cloned()
                        .unwrap_or(ScriptType::Record(record)),
                    _ => ScriptType::Record(record),
                };
                (HirExprKind::Map { type_name, fields }, ty)
            }
            ExprKind::Block(block) => {
                let block = self.lower_block(block, true);
                (HirExprKind::Block(block), ScriptType::Any)
            }
            ExprKind::Binary { left, op, right } => {
                let left = self.lower_expression(left);
                let right = self.lower_expression(right);
                let ty = binary_type(*op, self.expression_type(left), self.expression_type(right));
                (
                    HirExprKind::Binary {
                        left,
                        op: *op,
                        right,
                    },
                    ty,
                )
            }
        };
        self.alloc_expression(kind, ty, expression.span)
    }

    fn lower_identifier(&mut self, name: &str, span: Span) -> &'hir HirExpr<'hir> {
        let symbol = self.symbol(name);
        if let Some(local) = self.resolve_local(symbol) {
            return self.alloc_typed_expression(
                HirExprKind::Local(local),
                self.locals[local.0 as usize].ty,
                span,
            );
        }
        if let Some(global) = self.global_names.get(&symbol).copied() {
            return self.alloc_typed_expression(
                HirExprKind::Global(global),
                self.globals[global.0 as usize].ty,
                span,
            );
        }
        if let Some(function) = self.function_names.get(&symbol).copied() {
            return self.alloc_expression(HirExprKind::Function(function), ScriptType::Any, span);
        }
        if let Some(builtin) = self.manifest.and_then(|manifest| manifest.resolve(name)) {
            return self.alloc_expression(HirExprKind::Builtin(builtin), ScriptType::Any, span);
        }
        if self
            .manifest
            .is_some_and(|manifest| manifest.has_selector(name))
        {
            return self.alloc_expression(
                HirExprKind::Selector(symbol),
                ScriptType::Selector,
                span,
            );
        }
        self.alloc_expression(HirExprKind::Unresolved(symbol), ScriptType::Any, span)
    }

    fn lower_place(&mut self, expression: &Expr) -> Option<&'hir HirPlace<'hir>> {
        let place = match &expression.kind {
            ExprKind::Ident(name) => {
                let symbol = self.symbol(name);
                if let Some(local) = self.resolve_local(symbol) {
                    HirPlace::Local(local)
                } else if let Some(global) = self.global_names.get(&symbol).copied() {
                    HirPlace::Global(global)
                } else {
                    self.error(format!("unknown assignment root `{name}`"), expression.span);
                    return None;
                }
            }
            ExprKind::Member { object, name } => HirPlace::Member {
                object: self.lower_place(object)?,
                member: self.symbol(name),
            },
            _ => {
                self.error(
                    "assignment target must be a local, global, or member expression",
                    expression.span,
                );
                return None;
            }
        };
        Some(self.arena.alloc(place))
    }

    fn resolve_call(&mut self, expression: &Expr) -> ResolvedFunction {
        let ExprKind::Call { callee, .. } = &expression.kind else {
            return ResolvedFunction::Dynamic;
        };
        if let ExprKind::Ident(name) = &callee.kind {
            let symbol = self.symbol(name);
            if let Some(function) = self.function_names.get(&symbol).copied() {
                return ResolvedFunction::User(function);
            }
            if let Some(builtin) = self.manifest.and_then(|manifest| manifest.resolve(name)) {
                return ResolvedFunction::Builtin(builtin);
            }
            if self
                .manifest
                .and_then(BuiltinManifest::choice_syntax)
                .is_some_and(|syntax| syntax.choice == *name)
            {
                return ResolvedFunction::Syntax(symbol);
            }
        }
        if let ExprKind::Symbol(name) = &callee.kind
            && let Some(member) = self
                .manifest
                .and_then(|manifest| manifest.resolve_static_method(name).ok())
        {
            return ResolvedFunction::Builtin(member.builtin);
        }
        if let ExprKind::Member { object, name } = &callee.kind
            && let Some(builtin) = flatten_selector(object)
                .and_then(|selector| {
                    self.manifest
                        .and_then(|manifest| manifest.resolve_selector(&selector, name))
                })
                .or_else(|| self.manifest.and_then(|manifest| manifest.resolve(name)))
        {
            return ResolvedFunction::Builtin(builtin);
        }
        ResolvedFunction::Dynamic
    }

    fn call_result(&self, function: ResolvedFunction) -> ScriptType {
        match function {
            ResolvedFunction::User(function) => self
                .functions
                .get(function.0 as usize)
                .map(|function| function.result.clone())
                .unwrap_or(ScriptType::Any),
            ResolvedFunction::Builtin(builtin) => self
                .manifest
                .and_then(|manifest| manifest.signature(builtin))
                .map(|signature| signature.result.clone())
                .unwrap_or(ScriptType::Any),
            ResolvedFunction::Syntax(_) | ResolvedFunction::Dynamic => ScriptType::Any,
        }
    }

    fn declare_local(
        &mut self,
        name: &str,
        ty: ScriptType,
        mutable: bool,
        span: Span,
    ) -> HirLocalId {
        let name = self.symbol(name);
        let ty = self.types.intern(ty);
        let id = HirLocalId(self.locals.len() as u32);
        self.locals.push(HirLocal {
            name,
            ty,
            mutable,
            owner: self.current_function,
            span,
        });
        self.scopes
            .last_mut()
            .expect("HIR lowering always has a lexical scope")
            .insert(name, id);
        id
    }

    fn resolve_local(&self, name: SymbolId) -> Option<HirLocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    fn type_from_ast(&mut self, ty: &TypeExpr) -> Option<ScriptType> {
        match &ty.kind {
            TypeExprKind::Named(name) => match name.as_str() {
                "Any" => Some(ScriptType::Any),
                "Unit" => Some(ScriptType::Unit),
                "Bool" => Some(ScriptType::Bool),
                "Int" => Some(ScriptType::Int),
                "Float" | "Number" => Some(ScriptType::Number),
                "String" => Some(ScriptType::String),
                _ => self.aliases.get(name).cloned().or_else(|| {
                    self.manifest
                        .and_then(|manifest| manifest.symbols().find(name))
                        .map(ScriptType::Named)
                }),
            },
            TypeExprKind::Nullable(inner) => {
                Some(ScriptType::Nullable(Box::new(self.type_from_ast(inner)?)))
            }
            TypeExprKind::List(inner) => {
                Some(ScriptType::List(Box::new(self.type_from_ast(inner)?)))
            }
            TypeExprKind::Record(fields) => Some(ScriptType::Record(
                fields
                    .iter()
                    .map(|field| Some((field.name.clone(), self.type_from_ast(&field.ty)?)))
                    .collect::<Option<_>>()?,
            )),
        }
    }

    fn alloc_expression(
        &mut self,
        kind: HirExprKind<'hir>,
        ty: ScriptType,
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        let ty = self.types.intern(ty);
        self.alloc_typed_expression(kind, ty, span)
    }

    fn alloc_typed_expression(
        &self,
        kind: HirExprKind<'hir>,
        ty: TypeId,
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        self.arena.alloc(HirExpr { kind, ty, span })
    }

    fn expression_type(&self, expression: &HirExpr<'hir>) -> &ScriptType {
        self.types
            .get(expression.ty)
            .expect("HIR expression type is interned")
    }

    fn any_type(&mut self) -> TypeId {
        self.types.intern(ScriptType::Any)
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(LoweringError {
            message: message.into(),
            span,
        });
    }
}

fn member_type(object: &ScriptType, member: &str) -> ScriptType {
    match object {
        ScriptType::Record(fields) => fields.get(member).cloned().unwrap_or(ScriptType::Any),
        ScriptType::Nullable(inner) => member_type(inner, member),
        _ => ScriptType::Any,
    }
}

fn binary_type(op: BinaryOp, left: &ScriptType, right: &ScriptType) -> ScriptType {
    match op {
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => ScriptType::Bool,
        BinaryOp::Divide => ScriptType::Number,
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply
            if left == &ScriptType::Int && right == &ScriptType::Int =>
        {
            ScriptType::Int
        }
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => ScriptType::Number,
        BinaryOp::Colon => ScriptType::Any,
    }
}

fn flatten_selector(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => {
            Some(format!("{}.{}", flatten_selector(object)?, name))
        }
        _ => None,
    }
}

fn source_end(program: &Program) -> usize {
    program
        .statements
        .last()
        .map(statement_span)
        .map(|span| span.end)
        .unwrap_or_default()
}

fn statement_span(statement: &Stmt) -> Span {
    match statement {
        Stmt::TypeAlias { span, .. }
        | Stmt::Function { span, .. }
        | Stmt::Let { span, .. }
        | Stmt::Global { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. } => *span,
        Stmt::Expr(expression) => expression.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_program;

    #[test]
    fn arena_hir_uses_recursive_member_references() {
        let syntax =
            parse_program("let player = .{ stats: .{ health: 1 } }\nplayer.stats.health = 2")
                .expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("source lowers");
        let HirStmtKind::Assign { target, .. } = hir.entry.statements[1].kind else {
            panic!("expected assignment")
        };
        let HirPlace::Member { object, member } = target else {
            panic!("expected health member")
        };
        assert_eq!(hir.symbols.resolve(*member), Some("health"));
        let HirPlace::Member { object, member } = *object else {
            panic!("expected stats member")
        };
        assert_eq!(hir.symbols.resolve(*member), Some("stats"));
        assert!(matches!(*object, HirPlace::Local(_)));
        assert!(arena.allocated_bytes() > 0);
    }

    #[test]
    fn string_literals_outlive_the_syntax_tree() {
        let arena = HirArena::new();
        let hir = {
            let syntax = parse_program("\"hello\"").expect("source parses");
            lower_to_hir(&arena, &syntax, None).expect("source lowers")
        };
        let HirStmtKind::Expr(HirExpr {
            kind: HirExprKind::Literal(HirLiteral::String(value)),
            ..
        }) = hir.entry.statements[0].kind
        else {
            panic!("expected string literal")
        };
        assert_eq!(*value, "hello");
    }
}
