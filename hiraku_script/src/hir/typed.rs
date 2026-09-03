use std::collections::BTreeMap;

use bumpalo::Bump;

use crate::{
    BinaryOp, Block, CastMode, Expr, ExprKind, NumberUnit, Program, Span, Stmt, SymbolId,
    SymbolInterner, SymbolManifest, TypeExpr, TypeExprKind,
    runtime::{BuiltinId, BuiltinManifest},
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
    OptionalSome(&'hir HirExpr<'hir>),
    Cast {
        value: &'hir HirExpr<'hir>,
        target: &'hir ScriptType,
        mode: CastMode,
    },
    Call {
        callee: &'hir HirExpr<'hir>,
        arguments: &'hir [HirArgument<'hir>],
        function: ResolvedFunction,
    },
    Tuple(&'hir [&'hir HirExpr<'hir>]),
    List(&'hir [&'hir HirExpr<'hir>]),
    Map {
        type_name: Option<SymbolId>,
        fields: &'hir [(SymbolId, &'hir HirExpr<'hir>)],
    },
    Lambda {
        parameters: &'hir [HirLocalId],
        body: &'hir HirBlock<'hir>,
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
    Unit,
    Null,
    Ellipsis,
    Bool(bool),
    Number { value: f64, unit: NumberUnit },
    String(&'hir str),
    TextTemplate(&'hir str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedFunction {
    User(HirFunctionId),
    Builtin(BuiltinId),
    /// A symbol intentionally left for the runtime linker (for example a
    /// `global fn` exported by another script module).
    External(SymbolId),
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
    pub exported: bool,
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
    exported: bool,
    type_parameters: Vec<SymbolId>,
    parameters: Vec<ScriptType>,
    result: ScriptType,
    span: Span,
}

#[derive(Clone)]
struct TypeAliasDeclaration {
    parameters: Vec<String>,
    body: TypeExpr,
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
    aliases: BTreeMap<String, TypeAliasDeclaration>,
    type_parameters: Vec<BTreeMap<String, ScriptType>>,
    type_expansions: Vec<String>,
    named_imports: BTreeMap<String, String>,
    wildcard_import: Option<String>,
    current_function: Option<HirFunctionId>,
    refinements: Vec<BTreeMap<HirLocalId, ScriptType>>,
    errors: Vec<LoweringError>,
}

impl<'hir, 'manifest> Lowerer<'hir, 'manifest> {
    fn new(
        arena: &'hir HirArena,
        program: &Program,
        manifest: Option<&'manifest BuiltinManifest>,
    ) -> Self {
        let symbols = normalize_program_symbols(program, manifest.map(BuiltinManifest::symbols));
        let mut named_imports = BTreeMap::new();
        let mut wildcard_import = None;
        let mut import_errors = Vec::new();
        for statement in &program.statements {
            let Stmt::Import {
                path,
                wildcard,
                span,
            } = statement
            else {
                continue;
            };
            let qualified = path.join(".");
            if *wildcard {
                if let Some(existing) = &wildcard_import {
                    import_errors.push(LoweringError {
                        message: format!(
                            "wildcard import `{qualified}.*` conflicts with `{existing}.*`; import the required names explicitly"
                        ),
                        span: *span,
                    });
                } else {
                    wildcard_import = Some(qualified);
                }
            } else if let Some(local) = path.last() {
                if let Some(existing) = named_imports.insert(local.clone(), qualified.clone())
                    && existing != qualified
                {
                    import_errors.push(LoweringError {
                        message: format!(
                            "imported name `{local}` refers to both `{existing}` and `{qualified}`"
                        ),
                        span: *span,
                    });
                }
            }
        }
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
            type_parameters: Vec::new(),
            type_expansions: Vec::new(),
            named_imports,
            wildcard_import,
            current_function: None,
            refinements: Vec::new(),
            errors: import_errors,
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
                !matches!(
                    statement,
                    Stmt::Import { .. } | Stmt::TypeAlias { .. } | Stmt::Function { .. }
                )
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
            let Stmt::TypeAlias {
                name,
                type_parameters,
                ty,
                span,
            } = statement
            else {
                continue;
            };
            if self.aliases.contains_key(name) {
                self.error(format!("type `{name}` is defined more than once"), *span);
            } else {
                self.aliases.insert(
                    name.clone(),
                    TypeAliasDeclaration {
                        parameters: type_parameters.clone(),
                        body: ty.clone(),
                    },
                );
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
                exported,
                name,
                type_parameters,
                parameters,
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
            self.push_type_parameters(type_parameters);
            let parameter_types = parameters
                .iter()
                .map(|parameter| {
                    parameter
                        .ty
                        .as_ref()
                        .and_then(|ty| self.type_from_ast(ty))
                        .unwrap_or(ScriptType::Any)
                })
                .collect();
            let result = return_type
                .as_ref()
                .and_then(|ty| self.type_from_ast(ty))
                .unwrap_or(ScriptType::Any);
            self.type_parameters.pop();
            let generic_parameters = type_parameters
                .iter()
                .map(|name| self.symbol(name))
                .collect();
            self.functions.push(FunctionDeclaration {
                name: symbol,
                exported: *exported,
                type_parameters: generic_parameters,
                parameters: parameter_types,
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
                type_parameters,
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
            self.push_type_parameters(type_parameters);
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
                exported: declaration.exported,
                parameters,
                result,
                body,
                span: declaration.span,
            });
            self.type_parameters.pop();
        }
    }

    fn push_type_parameters(&mut self, parameters: &[String]) {
        let values = parameters
            .iter()
            .map(|name| {
                let symbol = self.symbol(name);
                (name.clone(), ScriptType::TypeParameter(symbol))
            })
            .collect();
        self.type_parameters.push(values);
    }

    fn lower_block(&mut self, block: &Block, scoped: bool) -> &'hir HirBlock<'hir> {
        self.lower_statements(block.statements.iter(), block.span, scoped)
    }

    fn lower_refined_block(
        &mut self,
        block: &Block,
        refinements: BTreeMap<HirLocalId, ScriptType>,
    ) -> &'hir HirBlock<'hir> {
        self.refinements.push(refinements);
        let lowered = self.lower_block(block, true);
        self.refinements.pop();
        lowered
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
            Stmt::Import { span, .. } => {
                self.error("imports are only allowed at module scope", *span);
                return None;
            }
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
                let annotation = type_annotation
                    .as_ref()
                    .and_then(|ty| self.type_from_ast(ty));
                let value = self.lower_expression_expected(value, annotation.as_ref());
                let inferred = self.expression_type(value).clone();
                if annotation.is_none() && is_untyped_none(&inferred) {
                    self.error(
                        format!(
                            "cannot infer the element type of `{name}` from `null`; add an explicit optional type such as `{name}: String?`"
                        ),
                        value.span,
                    );
                }
                let ty = annotation.unwrap_or_else(|| inferred.clone());
                self.check_assignment(&ty, &inferred, value.span);
                let local = self.declare_local(name, ty, *mutable, *span);
                (HirStmtKind::Let { local, value }, *span)
            }
            Stmt::Global {
                name, value, span, ..
            } => {
                let symbol = self.symbol(name);
                let global = self.global_names.get(&symbol).copied();
                let Some(global) = global else {
                    self.error(format!("unknown global `{name}`"), *span);
                    return None;
                };
                let declared = self.types.get(self.globals[global.0 as usize].ty).cloned();
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression_expected(value, declared.as_ref()));
                let any = self.any_type();
                if self.globals[global.0 as usize].ty == any
                    && let Some(value) = value
                {
                    if is_untyped_none(self.expression_type(value)) {
                        self.error(
                            format!(
                                "cannot infer the element type of global `{name}` from `null`; add an explicit optional type"
                            ),
                            value.span,
                        );
                    }
                    self.globals[global.0 as usize].ty = value.ty;
                } else if let (Some(expected), Some(value)) = (declared, value) {
                    let actual = self.expression_type(value).clone();
                    self.check_assignment(&expected, &actual, value.span);
                }
                (HirStmtKind::Global { global, value }, *span)
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let target = self.lower_place(target)?;
                let expected = self.place_type(target);
                let value = self.lower_expression_expected(value, expected.as_ref());
                if let Some(expected) = expected {
                    let actual = self.expression_type(value).clone();
                    self.check_assignment(&expected, &actual, value.span);
                }
                if let Some(local) = place_root_local(target) {
                    for refinements in self.refinements.iter_mut().rev() {
                        refinements.remove(&local);
                    }
                }
                (HirStmtKind::Assign { target, value }, *span)
            }
            Stmt::Expr(expression) => {
                let value = self.lower_expression(expression);
                if is_untyped_none(self.expression_type(value)) {
                    self.error(
                        "`null`/`.none` needs an expected Optional<T> type",
                        expression.span,
                    );
                }
                (HirStmtKind::Expr(value), expression.span)
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
                span,
            } => {
                let (truthy, falsy) = self.condition_refinements(condition);
                let condition = self.lower_expression(condition);
                if !ScriptType::Bool.accepts(self.expression_type(condition)) {
                    self.error(
                        format!(
                            "condition expects Bool, got {:?}",
                            self.expression_type(condition)
                        ),
                        condition.span,
                    );
                }
                let then_block = self.lower_refined_block(then_block, truthy);
                let else_block = else_block
                    .as_ref()
                    .map(|block| self.lower_refined_block(block, falsy));
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
                let (truthy, _) = self.condition_refinements(condition);
                let condition = self.lower_expression(condition);
                if !ScriptType::Bool.accepts(self.expression_type(condition)) {
                    self.error(
                        format!(
                            "condition expects Bool, got {:?}",
                            self.expression_type(condition)
                        ),
                        condition.span,
                    );
                }
                let body = self.lower_refined_block(body, truthy);
                (HirStmtKind::While { condition, body }, *span)
            }
        };
        Some(self.arena.alloc(HirStmt { kind, span }))
    }

    fn lower_expression(&mut self, expression: &Expr) -> &'hir HirExpr<'hir> {
        let (kind, ty) = match &expression.kind {
            ExprKind::Unit => (HirExprKind::Literal(HirLiteral::Unit), ScriptType::Unit),
            ExprKind::Null => (
                HirExprKind::Literal(HirLiteral::Null),
                ScriptType::Optional(Box::new(ScriptType::Any)),
            ),
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
                    NumberUnit::Scalar => ScriptType::Float,
                },
            ),
            ExprKind::String(value) => (
                HirExprKind::Literal(HirLiteral::String(self.arena.alloc_str(value))),
                ScriptType::String,
            ),
            ExprKind::Binding(value) => {
                let value = self.lower_expression(value);
                let result = self.expression_type(value).clone();
                let statement = self.arena.alloc(HirStmt {
                    kind: HirStmtKind::Expr(value),
                    span: value.span,
                });
                let statements = self.arena.alloc_slice_copy(&[statement]);
                let block = self.arena.alloc(HirBlock {
                    statements,
                    span: expression.span,
                });
                (
                    HirExprKind::Block(block),
                    ScriptType::Binding(Box::new(result)),
                )
            }
            ExprKind::Ident(name) => return self.lower_identifier(name, expression.span),
            ExprKind::Symbol(name) => {
                if name == "none" {
                    return self.alloc_expression(
                        HirExprKind::Literal(HirLiteral::Null),
                        ScriptType::Optional(Box::new(ScriptType::Any)),
                        expression.span,
                    );
                }
                let symbol = self.symbol(name);
                if let Some(member) = self
                    .manifest
                    .and_then(|manifest| manifest.resolve_getter(name).ok())
                {
                    let ty = self
                        .manifest
                        .and_then(|manifest| manifest.signature(member.builtin))
                        .map(|signature| signature.result.clone())
                        .unwrap_or(ScriptType::Any);
                    let callee = self.alloc_expression(
                        HirExprKind::Symbol(symbol),
                        ScriptType::Symbol,
                        expression.span,
                    );
                    return self.alloc_expression(
                        HirExprKind::Call {
                            callee,
                            arguments: &[],
                            function: ResolvedFunction::Builtin(member.builtin),
                        },
                        ty,
                        expression.span,
                    );
                }
                let ty = ScriptType::Symbol;
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
                if let ExprKind::Binding(bound) = &object.kind {
                    let bound = flatten_selector(bound).unwrap_or_else(|| "expression".into());
                    self.error(
                        format!(
                            "member access is outside the `$` binding; use `${{{bound}.{name}}}` to bind the complete selector"
                        ),
                        expression.span,
                    );
                }
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
                if !safe && matches!(self.expression_type(object), ScriptType::Optional(_)) {
                    self.error(
                        "optional member access requires `?.`, `!`, or a preceding null check",
                        expression.span,
                    );
                }
                let mut ty = member_type(self.expression_type(object), name);
                if safe {
                    ty = ScriptType::Optional(Box::new(ty));
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
                    ScriptType::Optional(inner) => (**inner).clone(),
                    ScriptType::Any => self.expression_type(fallback).clone(),
                    ty => ty.clone(),
                };
                (HirExprKind::Elvis { value, fallback }, ty)
            }
            ExprKind::NonNull(value) => {
                let value = self.lower_expression(value);
                let ty = match self.expression_type(value) {
                    ScriptType::Optional(inner) => (**inner).clone(),
                    ty => ty.clone(),
                };
                (HirExprKind::NonNull(value), ty)
            }
            ExprKind::Cast { value, ty, mode } => {
                let value = self.lower_expression(value);
                let source = self.expression_type(value).clone();
                let Some(target) = self.type_from_ast(ty) else {
                    self.error("cast refers to an unknown type", ty.span);
                    return self.alloc_expression(
                        HirExprKind::Cast {
                            value,
                            target: self.arena.alloc(ScriptType::Any),
                            mode: *mode,
                        },
                        ScriptType::Any,
                        expression.span,
                    );
                };
                match cast_certainty(&source, &target) {
                    CastCertainty::Impossible if *mode == CastMode::Static => self.error(
                        format!("cannot cast {source:?} to {target:?}"),
                        expression.span,
                    ),
                    CastCertainty::Runtime if *mode == CastMode::Static => self.error(
                        format!(
                            "cannot prove a cast from {source:?} to {target:?}; use `as?` for an optional result or `as!` for a runtime-checked cast"
                        ),
                        expression.span,
                    ),
                    CastCertainty::Always
                    | CastCertainty::Runtime
                    | CastCertainty::Impossible => {}
                }
                let result = if *mode == CastMode::Optional {
                    ScriptType::Optional(Box::new(target.clone()))
                } else {
                    target.clone()
                };
                (
                    HirExprKind::Cast {
                        value,
                        target: self.arena.alloc(target),
                        mode: *mode,
                    },
                    result,
                )
            }
            ExprKind::Call {
                callee: syntax_callee,
                type_arguments,
                arguments,
                trailing_block,
            } => {
                let explicit_types = type_arguments
                    .iter()
                    .filter_map(|ty| self.type_from_ast(ty))
                    .collect::<Vec<_>>();
                if matches!(&syntax_callee.kind, ExprKind::Symbol(name) if name == "some") {
                    if trailing_block.is_some() || arguments.len() != 1 {
                        self.error("`.some` expects exactly one value", expression.span);
                        return self.alloc_expression(
                            HirExprKind::Literal(HirLiteral::Null),
                            ScriptType::Optional(Box::new(ScriptType::Any)),
                            expression.span,
                        );
                    }
                    let value = self.lower_expression(&arguments[0].value);
                    let ty = ScriptType::Optional(Box::new(self.expression_type(value).clone()));
                    return self.alloc_expression(
                        HirExprKind::OptionalSome(value),
                        ty,
                        expression.span,
                    );
                }
                let callee = self.lower_expression(syntax_callee);
                let function = self.resolve_call(expression);
                if function == ResolvedFunction::Dynamic
                    && let ExprKind::Member { name, .. } = &syntax_callee.kind
                    && let HirExprKind::Member { object, .. } = callee.kind
                    && let ScriptType::Named(owner) = self.expression_type(object)
                {
                    let owner = self.symbols.resolve(*owner).unwrap_or("<unknown>");
                    self.error(
                        format!("unknown method `{name}` for `{owner}`"),
                        syntax_callee.span,
                    );
                }
                let (expected_parameters, expected_variadic) = match function {
                    ResolvedFunction::Builtin(builtin) => self
                        .manifest
                        .and_then(|manifest| manifest.signature(builtin))
                        .map(|signature| (signature.parameters.clone(), signature.variadic.clone()))
                        .map_or((None, None), |(parameters, variadic)| {
                            (Some(parameters), variadic)
                        }),
                    ResolvedFunction::User(function) => self
                        .functions
                        .get(function.0 as usize)
                        .map(|function| (Some(function.parameters.clone()), None))
                        .unwrap_or((None, None)),
                    _ => (None, None),
                };
                let mut arguments = arguments
                    .iter()
                    .enumerate()
                    .map(|(index, argument)| HirArgument {
                        label: argument.label.as_deref().map(|label| self.symbol(label)),
                        value: self.lower_expression_expected(
                            &argument.value,
                            expected_parameters
                                .as_ref()
                                .and_then(|parameters| parameters.get(index))
                                .or(expected_variadic.as_ref()),
                        ),
                        span: argument.span,
                    })
                    .collect::<Vec<_>>();
                if let Some(block) = trailing_block {
                    let block = self.lower_block(block, true);
                    let closure = self.alloc_expression(
                        HirExprKind::Block(block),
                        ScriptType::Function,
                        block.span,
                    );
                    arguments.push(HirArgument {
                        label: None,
                        value: closure,
                        span: block.span,
                    });
                }
                let arguments = self.arena.alloc_slice_copy(&arguments);
                self.check_call(
                    function,
                    callee,
                    arguments,
                    &explicit_types,
                    expression.span,
                );
                let ty = self.call_result(function, arguments, &explicit_types);
                (
                    HirExprKind::Call {
                        callee,
                        arguments,
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
                    let element = values.iter().fold(None, |element, value| {
                        let value = self.expression_type(value).clone();
                        Some(match element {
                            None => value,
                            Some(element) => join_types(element, value),
                        })
                    });
                    let element = element.unwrap_or(ScriptType::Any);
                    ScriptType::List(Box::new(element))
                };
                let values = self.arena.alloc_slice_copy(&values);
                if matches!(expression.kind, ExprKind::Tuple(_)) {
                    (HirExprKind::Tuple(values), ty)
                } else {
                    (HirExprKind::List(values), ty)
                }
            }
            ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } => {
                let type_name = match &expression.kind {
                    ExprKind::TypedStructLiteral { type_name, .. } => Some(self.symbol(type_name)),
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
                    ExprKind::TypedStructLiteral { type_name, .. } => self
                        .instantiate_named_type(type_name, &[], expression.span)
                        .unwrap_or(ScriptType::Record(record)),
                    _ => ScriptType::Record(record),
                };
                (HirExprKind::Map { type_name, fields }, ty)
            }
            ExprKind::Lambda { parameters, body } => {
                self.scopes.push(BTreeMap::new());
                let parameters = parameters
                    .iter()
                    .map(|parameter| {
                        let ty = parameter
                            .ty
                            .as_ref()
                            .and_then(|ty| self.type_from_ast(ty))
                            .unwrap_or(ScriptType::Any);
                        self.declare_local(&parameter.name, ty, false, parameter.span)
                    })
                    .collect::<Vec<_>>();
                let body = self.lower_block(body, false);
                self.scopes.pop();
                (
                    HirExprKind::Lambda {
                        parameters: self.arena.alloc_slice_copy(&parameters),
                        body,
                    },
                    ScriptType::Function,
                )
            }
            ExprKind::Block(block) => {
                let block = self.lower_block(block, true);
                (HirExprKind::Block(block), ScriptType::Function)
            }
            ExprKind::Binary { left, op, right } => {
                let left = self.lower_expression(left);
                let right = self.lower_expression(right);
                if let Some(builtin) = self.manifest.and_then(|manifest| {
                    manifest.resolve_operator(match op {
                        crate::BinaryOp::Colon => ":",
                        _ => "",
                    })
                }) {
                    let arguments = self.arena.alloc_slice_copy(&[
                        HirArgument {
                            label: None,
                            value: left,
                            span: left.span,
                        },
                        HirArgument {
                            label: None,
                            value: right,
                            span: right.span,
                        },
                    ]);
                    let callee = self.alloc_expression(
                        HirExprKind::Builtin(builtin),
                        ScriptType::Any,
                        expression.span,
                    );
                    return self.alloc_expression(
                        HirExprKind::Call {
                            callee,
                            arguments,
                            function: ResolvedFunction::Builtin(builtin),
                        },
                        self.call_result(ResolvedFunction::Builtin(builtin), arguments, &[]),
                        expression.span,
                    );
                }
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

    /// Lowers expressions whose abbreviated static member is determined by
    /// the surrounding parameter type. For example, when `at` expects
    /// `UiPosition`, `.rel(50, 50)` is resolved as `UiPosition.rel(50, 50)`.
    /// This keeps selector syntax concise without making static member names
    /// globally unique.
    fn lower_expression_expected(
        &mut self,
        expression: &Expr,
        expected: Option<&ScriptType>,
    ) -> &'hir HirExpr<'hir> {
        if expected == Some(&ScriptType::TextTemplate)
            && let ExprKind::String(value) = &expression.kind
        {
            return self.alloc_expression(
                HirExprKind::Literal(HirLiteral::TextTemplate(self.arena.alloc_str(value))),
                ScriptType::TextTemplate,
                expression.span,
            );
        }
        if let Some(ScriptType::Struct {
            name,
            arguments,
            fields: expected_fields,
        }) = expected
            && let ExprKind::StructLiteral(fields) = &expression.kind
        {
            let mut actual_fields = BTreeMap::new();
            let fields = fields
                .iter()
                .map(|field| {
                    let field_name = self.symbol(&field.name);
                    let value = self
                        .lower_expression_expected(&field.value, expected_fields.get(&field.name));
                    actual_fields.insert(field.name.clone(), self.expression_type(value).clone());
                    (field_name, value)
                })
                .collect::<Vec<_>>();
            self.check_assignment(
                &ScriptType::Record(expected_fields.clone()),
                &ScriptType::Record(actual_fields),
                expression.span,
            );
            let ty = ScriptType::Struct {
                name: *name,
                arguments: arguments.clone(),
                fields: expected_fields.clone(),
            };
            return self.alloc_expression(
                HirExprKind::Map {
                    type_name: Some(*name),
                    fields: self.arena.alloc_slice_copy(&fields),
                },
                ty,
                expression.span,
            );
        }
        if let Some(ScriptType::Map(key, element)) = expected
            && let ExprKind::StructLiteral(fields) = &expression.kind
        {
            let fields = fields
                .iter()
                .map(|field| {
                    let name = self.symbol(&field.name);
                    let value = self.lower_expression_expected(&field.value, Some(element));
                    (name, value)
                })
                .collect::<Vec<_>>();
            return self.alloc_expression(
                HirExprKind::Map {
                    type_name: None,
                    fields: self.arena.alloc_slice_copy(&fields),
                },
                ScriptType::Map(key.clone(), element.clone()),
                expression.span,
            );
        }
        if let Some(ScriptType::Record(expected_fields)) = expected
            && let ExprKind::StructLiteral(fields) | ExprKind::TypedStructLiteral { fields, .. } =
                &expression.kind
        {
            let type_name = match &expression.kind {
                ExprKind::TypedStructLiteral { type_name, .. } => Some(self.symbol(type_name)),
                _ => None,
            };
            let mut actual_fields = BTreeMap::new();
            let fields = fields
                .iter()
                .map(|field| {
                    let name = self.symbol(&field.name);
                    let value = self
                        .lower_expression_expected(&field.value, expected_fields.get(&field.name));
                    actual_fields.insert(field.name.clone(), self.expression_type(value).clone());
                    (name, value)
                })
                .collect::<Vec<_>>();
            self.check_assignment(
                &ScriptType::Record(expected_fields.clone()),
                &ScriptType::Record(actual_fields),
                expression.span,
            );
            return self.alloc_expression(
                HirExprKind::Map {
                    type_name,
                    fields: self.arena.alloc_slice_copy(&fields),
                },
                ScriptType::Record(expected_fields.clone()),
                expression.span,
            );
        }
        if let Some(ScriptType::Optional(inner)) = expected {
            if matches!(expression.kind, ExprKind::Null)
                || matches!(&expression.kind, ExprKind::Symbol(name) if name == "none")
            {
                return self.alloc_expression(
                    HirExprKind::Literal(HirLiteral::Null),
                    ScriptType::Optional(inner.clone()),
                    expression.span,
                );
            }
            if let ExprKind::Call {
                callee,
                type_arguments: _,
                arguments,
                trailing_block: None,
            } = &expression.kind
                && matches!(&callee.kind, ExprKind::Symbol(name) if name == "some")
                && arguments.len() == 1
            {
                let value = self.lower_expression_expected(&arguments[0].value, Some(inner));
                return self.alloc_expression(
                    HirExprKind::OptionalSome(value),
                    ScriptType::Optional(inner.clone()),
                    expression.span,
                );
            }
        }
        if let Some(ScriptType::List(element)) = expected
            && let ExprKind::List(values) = &expression.kind
        {
            let values = values
                .iter()
                .map(|value| self.lower_expression_expected(value, Some(element)))
                .collect::<Vec<_>>();
            let values = self.arena.alloc_slice_copy(&values);
            return self.alloc_expression(
                HirExprKind::List(values),
                ScriptType::List(element.clone()),
                expression.span,
            );
        }
        let Some(ScriptType::Named(owner)) = expected else {
            let value = self.lower_expression(expression);
            if let Some(expected @ ScriptType::Optional(inner)) = expected
                && !matches!(self.expression_type(value), ScriptType::Optional(_))
                && inner.accepts(self.expression_type(value))
            {
                return self.alloc_expression(
                    HirExprKind::Cast {
                        value,
                        target: self.arena.alloc(expected.clone()),
                        mode: CastMode::Static,
                    },
                    expected.clone(),
                    expression.span,
                );
            }
            return value;
        };
        if let ExprKind::Symbol(name) = &expression.kind
            && let Some(member) = self
                .manifest
                .and_then(|manifest| manifest.resolve_getter_for(*owner, name))
        {
            let builtin = member.builtin;
            let name = self.symbol(name);
            let callee = self.alloc_expression(
                HirExprKind::Symbol(name),
                ScriptType::Symbol,
                expression.span,
            );
            return self.alloc_expression(
                HirExprKind::Call {
                    callee,
                    arguments: &[],
                    function: ResolvedFunction::Builtin(builtin),
                },
                self.call_result(ResolvedFunction::Builtin(builtin), &[], &[]),
                expression.span,
            );
        }
        let ExprKind::Call {
            callee,
            type_arguments,
            arguments,
            trailing_block,
        } = &expression.kind
        else {
            return self.lower_expression(expression);
        };
        if !type_arguments.is_empty() {
            self.error(
                "engine static methods do not accept script type arguments",
                expression.span,
            );
        }
        let ExprKind::Symbol(name) = &callee.kind else {
            return self.lower_expression(expression);
        };
        let Some(builtin) = self
            .manifest
            .and_then(|manifest| manifest.resolve_static_method_for(*owner, name))
            .map(|member| member.builtin)
        else {
            return self.lower_expression(expression);
        };
        let expected_parameters = self
            .manifest
            .and_then(|manifest| manifest.signature(builtin))
            .map(|signature| signature.parameters.clone())
            .unwrap_or_default();
        let name = self.symbol(name);
        let callee =
            self.alloc_expression(HirExprKind::Symbol(name), ScriptType::Symbol, callee.span);
        let mut lowered_arguments = arguments
            .iter()
            .enumerate()
            .map(|(index, argument)| HirArgument {
                label: argument.label.as_deref().map(|label| self.symbol(label)),
                value: self
                    .lower_expression_expected(&argument.value, expected_parameters.get(index)),
                span: argument.span,
            })
            .collect::<Vec<_>>();
        if let Some(block) = trailing_block {
            let block = self.lower_block(block, true);
            lowered_arguments.push(HirArgument {
                label: None,
                value: self.alloc_expression(
                    HirExprKind::Block(block),
                    ScriptType::Function,
                    block.span,
                ),
                span: block.span,
            });
        }
        let lowered_arguments = self.arena.alloc_slice_copy(&lowered_arguments);
        let function = ResolvedFunction::Builtin(builtin);
        self.check_call(function, callee, lowered_arguments, &[], expression.span);
        self.alloc_expression(
            HirExprKind::Call {
                callee,
                arguments: lowered_arguments,
                function,
            },
            self.call_result(function, lowered_arguments, &[]),
            expression.span,
        )
    }

    fn check_call(
        &mut self,
        function: ResolvedFunction,
        callee: &HirExpr<'_>,
        arguments: &[HirArgument<'_>],
        explicit_types: &[ScriptType],
        span: Span,
    ) {
        if let ResolvedFunction::User(function) = function {
            let Some(declaration) = self.functions.get(function.0 as usize) else {
                return;
            };
            let parameters = declaration.parameters.clone();
            let generic_parameters = declaration.type_parameters.clone();
            if parameters.len() != arguments.len() {
                self.error(
                    format!(
                        "function expects {} arguments, got {}",
                        parameters.len(),
                        arguments.len()
                    ),
                    span,
                );
                return;
            }
            if !explicit_types.is_empty() && explicit_types.len() != generic_parameters.len() {
                self.error(
                    format!(
                        "generic function expects {} type arguments, got {}",
                        generic_parameters.len(),
                        explicit_types.len()
                    ),
                    span,
                );
                return;
            }
            let substitutions = if explicit_types.is_empty() {
                infer_type_arguments(&parameters, arguments, |value| {
                    self.expression_type(value).clone()
                })
            } else {
                generic_parameters
                    .iter()
                    .copied()
                    .zip(explicit_types.iter().cloned())
                    .collect()
            };
            for parameter in &generic_parameters {
                if !substitutions.contains_key(parameter) {
                    let name = self.symbols.resolve(*parameter).unwrap_or("<unknown>");
                    self.error(
                        format!(
                            "cannot infer generic parameter `{name}` from this call; add a value whose type determines it"
                        ),
                        span,
                    );
                }
            }
            for (expected, actual) in parameters.iter().zip(arguments) {
                let expected = substitute_type(expected, &substitutions);
                if !expected.accepts(self.expression_type(actual.value)) {
                    self.error(
                        format!(
                            "argument expects {expected:?}, got {:?}",
                            self.expression_type(actual.value)
                        ),
                        actual.span,
                    );
                }
            }
            return;
        }
        let ResolvedFunction::Builtin(builtin) = function else {
            return;
        };
        if !explicit_types.is_empty() {
            self.error("native functions do not accept script type arguments", span);
        }
        let Some(signature) = self
            .manifest
            .and_then(|manifest| manifest.signature(builtin))
        else {
            return;
        };
        let required = signature
            .parameters
            .iter()
            .rposition(|parameter| !matches!(parameter, ScriptType::Optional(_)))
            .map_or(0, |index| index + 1);
        if arguments.len() < required
            || (signature.variadic.is_none() && arguments.len() > signature.parameters.len())
        {
            let maximum = signature.variadic.as_ref().map_or_else(
                || signature.parameters.len().to_string(),
                |_| "unbounded".into(),
            );
            self.error(
                format!(
                    "function expects {required} to {maximum} arguments, got {}",
                    arguments.len()
                ),
                span,
            );
            return;
        }
        if let Some(expected) = &signature.receiver
            && let HirExprKind::Member { object, .. } = callee.kind
            && !expected.accepts(self.expression_type(object))
        {
            self.error(
                format!(
                    "receiver expects {expected:?}, got {:?}",
                    self.expression_type(object)
                ),
                callee.span,
            );
        }
        for (index, actual) in arguments.iter().enumerate() {
            let Some(expected) = signature
                .parameters
                .get(index)
                .or(signature.variadic.as_ref())
            else {
                continue;
            };
            if !expected.accepts(self.expression_type(actual.value)) {
                self.error(
                    format!(
                        "argument expects {expected:?}, got {:?}",
                        self.expression_type(actual.value)
                    ),
                    actual.span,
                );
            }
        }
    }

    fn lower_identifier(&mut self, name: &str, span: Span) -> &'hir HirExpr<'hir> {
        let symbol = self.symbol(name);
        if let Some(local) = self.resolve_local(symbol) {
            let refined = self
                .refinements
                .iter()
                .rev()
                .find_map(|refinements| refinements.get(&local))
                .cloned();
            if let Some(refined) = refined {
                let declared = self.locals[local.0 as usize].ty;
                let value = self.alloc_typed_expression(HirExprKind::Local(local), declared, span);
                return self.alloc_expression(HirExprKind::NonNull(value), refined, span);
            }
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
            return self.alloc_expression(
                HirExprKind::Function(function),
                ScriptType::Function,
                span,
            );
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
        let imported = self.imported_name(name);
        let symbol = self.symbol(&imported);
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

    fn place_type(&self, place: &HirPlace<'hir>) -> Option<ScriptType> {
        match place {
            HirPlace::Local(local) => self
                .types
                .get(self.locals.get(local.0 as usize)?.ty)
                .cloned(),
            HirPlace::Global(global) => self
                .types
                .get(self.globals.get(global.0 as usize)?.ty)
                .cloned(),
            HirPlace::Member { object, member } => {
                let object = self.place_type(object)?;
                let name = self.symbols.resolve(*member)?;
                Some(member_type(&object, name))
            }
        }
    }

    fn condition_refinements(
        &mut self,
        condition: &Expr,
    ) -> (
        BTreeMap<HirLocalId, ScriptType>,
        BTreeMap<HirLocalId, ScriptType>,
    ) {
        let ExprKind::Binary { left, op, right } = &condition.kind else {
            return (BTreeMap::new(), BTreeMap::new());
        };
        let name = match (&left.kind, &right.kind) {
            (ExprKind::Ident(name), ExprKind::Null) | (ExprKind::Null, ExprKind::Ident(name)) => {
                name
            }
            _ => return (BTreeMap::new(), BTreeMap::new()),
        };
        let symbol = self.symbol(name);
        let Some(local) = self.resolve_local(symbol) else {
            return (BTreeMap::new(), BTreeMap::new());
        };
        let Some(ScriptType::Optional(inner)) =
            self.types.get(self.locals[local.0 as usize].ty).cloned()
        else {
            return (BTreeMap::new(), BTreeMap::new());
        };
        let narrowed = BTreeMap::from([(local, *inner)]);
        match op {
            BinaryOp::NotEqual => (narrowed, BTreeMap::new()),
            BinaryOp::Equal => (BTreeMap::new(), narrowed),
            _ => (BTreeMap::new(), BTreeMap::new()),
        }
    }

    fn check_assignment(&mut self, expected: &ScriptType, actual: &ScriptType, span: Span) {
        if !expected.accepts(actual) {
            self.error(format!("expected {expected:?}, got {actual:?}"), span);
        }
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
            if self.resolve_local(symbol).is_none() {
                let imported = self.imported_name(name);
                return ResolvedFunction::External(self.symbol(&imported));
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

    fn call_result(
        &self,
        function: ResolvedFunction,
        arguments: &[HirArgument<'hir>],
        explicit_types: &[ScriptType],
    ) -> ScriptType {
        match function {
            ResolvedFunction::User(function) => {
                self.functions
                    .get(function.0 as usize)
                    .map_or(ScriptType::Any, |function| {
                        let substitutions = if explicit_types.is_empty() {
                            infer_type_arguments(&function.parameters, arguments, |value| {
                                self.expression_type(value).clone()
                            })
                        } else {
                            function
                                .type_parameters
                                .iter()
                                .copied()
                                .zip(explicit_types.iter().cloned())
                                .collect()
                        };
                        substitute_type(&function.result, &substitutions)
                    })
            }
            ResolvedFunction::Builtin(builtin) => self
                .manifest
                .and_then(|manifest| manifest.signature(builtin))
                .map(|signature| signature.result.clone())
                .unwrap_or(ScriptType::Any),
            ResolvedFunction::External(_) | ResolvedFunction::Dynamic => ScriptType::Any,
        }
    }

    fn imported_name(&self, name: &str) -> String {
        self.named_imports
            .get(name)
            .cloned()
            .or_else(|| {
                self.wildcard_import
                    .as_ref()
                    .map(|namespace| format!("{namespace}.{name}"))
            })
            .unwrap_or_else(|| name.to_string())
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
            TypeExprKind::Named(name) => {
                if let Some(parameter) = self
                    .type_parameters
                    .iter()
                    .rev()
                    .find_map(|parameters| parameters.get(name))
                {
                    return Some(parameter.clone());
                }
                self.instantiate_named_type(name, &[], ty.span)
            }
            TypeExprKind::Applied { name, arguments } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.type_from_ast(argument))
                    .collect::<Option<Vec<_>>>()?;
                self.instantiate_named_type(name, &arguments, ty.span)
            }
            TypeExprKind::Nullable(inner) => {
                let inner = self.type_from_ast(inner)?;
                Some(match inner {
                    ScriptType::Optional(inner) => ScriptType::Optional(inner),
                    inner => ScriptType::Optional(Box::new(inner)),
                })
            }
            TypeExprKind::List(inner) => {
                Some(ScriptType::List(Box::new(self.type_from_ast(inner)?)))
            }
            TypeExprKind::Binding(inner) => {
                Some(ScriptType::Binding(Box::new(self.type_from_ast(inner)?)))
            }
            TypeExprKind::Record(fields) => Some(ScriptType::Record(
                fields
                    .iter()
                    .map(|field| Some((field.name.clone(), self.type_from_ast(&field.ty)?)))
                    .collect::<Option<_>>()?,
            )),
        }
    }

    fn instantiate_named_type(
        &mut self,
        name: &str,
        arguments: &[ScriptType],
        span: Span,
    ) -> Option<ScriptType> {
        let builtin = match name {
            "Any" => Some(ScriptType::Any),
            "Unit" => Some(ScriptType::Unit),
            "Bool" => Some(ScriptType::Bool),
            "Int" => Some(ScriptType::Int),
            "Float" => Some(ScriptType::Float),
            "String" => Some(ScriptType::String),
            "TextTemplate" => Some(ScriptType::TextTemplate),
            "Symbol" => Some(ScriptType::Symbol),
            "Selector" => Some(ScriptType::Selector),
            "Function" => Some(ScriptType::Function),
            "Task" => Some(ScriptType::Task),
            _ => None,
        };
        if let Some(builtin) = builtin {
            if !arguments.is_empty() {
                self.error(
                    format!("type `{name}` does not accept type arguments"),
                    span,
                );
                return None;
            }
            return Some(builtin);
        }
        if matches!(name, "List" | "Binding" | "Optional") {
            if arguments.len() != 1 {
                self.error(
                    format!("type `{name}` expects exactly one type argument"),
                    span,
                );
                return None;
            }
            return Some(match name {
                "List" => ScriptType::List(Box::new(arguments[0].clone())),
                "Binding" => ScriptType::Binding(Box::new(arguments[0].clone())),
                "Optional" => ScriptType::Optional(Box::new(arguments[0].clone())),
                _ => unreachable!(),
            });
        }
        if name == "Map" {
            if arguments.len() != 2 {
                self.error(
                    "type `Map` expects exactly two type arguments; raw Map is not allowed",
                    span,
                );
                return None;
            }
            if arguments[0] != ScriptType::String {
                self.error("HKS maps currently require String keys", span);
                return None;
            }
            return Some(ScriptType::Map(
                Box::new(arguments[0].clone()),
                Box::new(arguments[1].clone()),
            ));
        }
        if let Some(alias) = self.aliases.get(name).cloned() {
            if alias.parameters.len() != arguments.len() {
                self.error(
                    format!(
                        "type `{name}` expects {} type arguments; raw generic types are not allowed",
                        alias.parameters.len()
                    ),
                    span,
                );
                return None;
            }
            if self.type_expansions.iter().any(|expanded| expanded == name) {
                self.error(
                    format!("recursive type alias `{name}` requires an indirection type"),
                    span,
                );
                return None;
            }
            self.type_expansions.push(name.to_string());
            self.type_parameters.push(
                alias
                    .parameters
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect(),
            );
            let body = self.type_from_ast(&alias.body);
            self.type_parameters.pop();
            self.type_expansions.pop();
            let body = body?;
            let fields = match body {
                ScriptType::Record(fields) => fields,
                other => return Some(other),
            };
            return Some(ScriptType::Struct {
                name: self.symbol(name),
                arguments: arguments.to_vec(),
                fields,
            });
        }
        self.manifest
            .and_then(|manifest| manifest.symbols().find(name))
            .map(ScriptType::Named)
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
        ScriptType::Struct { fields, .. } => fields.get(member).cloned().unwrap_or(ScriptType::Any),
        ScriptType::Map(_, value) => (**value).clone(),
        ScriptType::Optional(inner) => member_type(inner, member),
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
        BinaryOp::Divide => ScriptType::Float,
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply
            if left == &ScriptType::Int && right == &ScriptType::Int =>
        {
            ScriptType::Int
        }
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => ScriptType::Float,
        BinaryOp::Colon => ScriptType::Any,
    }
}

fn join_types(left: ScriptType, right: ScriptType) -> ScriptType {
    use ScriptType::*;
    if left == right {
        return left;
    }
    match (left, right) {
        (Int, Float) | (Float, Int) => Float,
        (Optional(left), Optional(right)) => Optional(Box::new(join_types(*left, *right))),
        (Optional(left), right) | (right, Optional(left)) => {
            Optional(Box::new(join_types(*left, right)))
        }
        (Any, _) | (_, Any) => Any,
        (left, right) if left.accepts(&right) => left,
        (left, right) if right.accepts(&left) => right,
        (left, right) => Union(vec![left, right]),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CastCertainty {
    Always,
    Runtime,
    Impossible,
}

fn cast_certainty(source: &ScriptType, target: &ScriptType) -> CastCertainty {
    use CastCertainty::{Always, Impossible, Runtime};
    use ScriptType::*;

    if target == &Any || source == target || target.accepts(source) && source != &Any {
        return Always;
    }
    if source == &Any {
        return Runtime;
    }
    match (source, target) {
        // Numeric storage is normalized to f64. `as Int` supplies the explicit
        // truncation requested by the author; Int -> Float is implicit.
        (Float, Int) | (Int, Float) => Always,
        (Optional(source), Optional(target)) => cast_certainty(source, target),
        (Optional(source), target) => match cast_certainty(source, target) {
            Impossible => Impossible,
            Always | Runtime => Runtime,
        },
        (source, Optional(target)) => cast_certainty(source, target),
        (Union(sources), target) => {
            let mut certainty = Always;
            for source in sources {
                match cast_certainty(source, target) {
                    Impossible => return Runtime,
                    Runtime => certainty = Runtime,
                    Always => {}
                }
            }
            certainty
        }
        (source, Union(targets)) => targets
            .iter()
            .map(|target| cast_certainty(source, target))
            .min_by_key(|certainty| match certainty {
                Always => 0,
                Runtime => 1,
                Impossible => 2,
            })
            .unwrap_or(Impossible),
        (List(source), List(target)) => cast_certainty(source, target),
        (Record(_), Record(_))
        | (Map(_, _), Record(_))
        | (Record(_), Map(_, _))
        | (Struct { .. }, Record(_))
        | (Record(_), Struct { .. }) => Runtime,
        (Named(source), Named(target)) if source == target => Always,
        _ => Impossible,
    }
}

fn is_untyped_none(ty: &ScriptType) -> bool {
    matches!(ty, ScriptType::Optional(inner) if inner.as_ref() == &ScriptType::Any)
}

fn place_root_local(place: &HirPlace<'_>) -> Option<HirLocalId> {
    match place {
        HirPlace::Local(local) => Some(*local),
        HirPlace::Global(_) => None,
        HirPlace::Member { object, .. } => place_root_local(object),
    }
}

fn infer_type_arguments<'hir>(
    parameters: &[ScriptType],
    arguments: &[HirArgument<'hir>],
    mut type_of: impl FnMut(&HirExpr<'hir>) -> ScriptType,
) -> BTreeMap<SymbolId, ScriptType> {
    let mut substitutions = BTreeMap::new();
    for (parameter, argument) in parameters.iter().zip(arguments) {
        infer_type_argument(parameter, &type_of(argument.value), &mut substitutions);
    }
    substitutions
}

fn infer_type_argument(
    parameter: &ScriptType,
    actual: &ScriptType,
    substitutions: &mut BTreeMap<SymbolId, ScriptType>,
) {
    match (parameter, actual) {
        (ScriptType::TypeParameter(parameter), actual) => {
            substitutions
                .entry(*parameter)
                .and_modify(|current| *current = join_types(current.clone(), actual.clone()))
                .or_insert_with(|| actual.clone());
        }
        (ScriptType::Optional(parameter), ScriptType::Optional(actual))
        | (ScriptType::List(parameter), ScriptType::List(actual)) => {
            infer_type_argument(parameter, actual, substitutions)
        }
        (
            ScriptType::Map(parameter_key, parameter_value),
            ScriptType::Map(actual_key, actual_value),
        ) => {
            infer_type_argument(parameter_key, actual_key, substitutions);
            infer_type_argument(parameter_value, actual_value, substitutions);
        }
        (
            ScriptType::Struct {
                arguments: parameters,
                ..
            },
            ScriptType::Struct {
                arguments: actuals, ..
            },
        ) => {
            for (parameter, actual) in parameters.iter().zip(actuals) {
                infer_type_argument(parameter, actual, substitutions);
            }
        }
        _ => {}
    }
}

fn substitute_type(ty: &ScriptType, substitutions: &BTreeMap<SymbolId, ScriptType>) -> ScriptType {
    match ty {
        ScriptType::TypeParameter(parameter) => substitutions
            .get(parameter)
            .cloned()
            .unwrap_or(ScriptType::Any),
        ScriptType::Optional(inner) => {
            ScriptType::Optional(Box::new(substitute_type(inner, substitutions)))
        }
        ScriptType::List(inner) => {
            ScriptType::List(Box::new(substitute_type(inner, substitutions)))
        }
        ScriptType::Binding(inner) => {
            ScriptType::Binding(Box::new(substitute_type(inner, substitutions)))
        }
        ScriptType::Map(key, value) => ScriptType::Map(
            Box::new(substitute_type(key, substitutions)),
            Box::new(substitute_type(value, substitutions)),
        ),
        ScriptType::Struct {
            name,
            arguments,
            fields,
        } => ScriptType::Struct {
            name: *name,
            arguments: arguments
                .iter()
                .map(|argument| substitute_type(argument, substitutions))
                .collect(),
            fields: fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type(ty, substitutions)))
                .collect(),
        },
        ScriptType::Union(types) => ScriptType::Union(
            types
                .iter()
                .map(|ty| substitute_type(ty, substitutions))
                .collect(),
        ),
        ScriptType::Record(fields) => ScriptType::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type(ty, substitutions)))
                .collect(),
        ),
        ty => ty.clone(),
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
        Stmt::Import { span, .. }
        | Stmt::TypeAlias { span, .. }
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

    #[test]
    fn selector_bindings_require_braces() {
        let shorthand = parse_program("let dialogue = .{ text: \"hello\" }\n$dialogue.text")
            .expect("shorthand source parses");
        let arena = HirArena::new();
        let errors = lower_to_hir(&arena, &shorthand, None)
            .expect_err("member access outside shorthand binding must be rejected");
        assert!(errors.iter().any(|error| {
            error
                .message
                .contains("use `${dialogue.text}` to bind the complete selector")
        }));

        let explicit = parse_program("let dialogue = .{ text: \"hello\" }\n${dialogue.text}")
            .expect("explicit binding source parses");
        let arena = HirArena::new();
        lower_to_hir(&arena, &explicit, None).expect("explicit selector binding must lower");
    }

    #[test]
    fn normalized_inference_promotes_int_to_float_but_requires_an_explicit_downcast() {
        let accepted = parse_program("let a = 1\nlet b: Float = a\nlet c: Int = b as Int")
            .expect("source parses");
        let arena = HirArena::new();
        lower_to_hir(&arena, &accepted, None).expect("numeric conversions must lower");

        let rejected = parse_program("let a = 1.5\nlet b: Int = a").expect("source parses");
        let arena = HirArena::new();
        let errors = lower_to_hir(&arena, &rejected, None)
            .expect_err("implicit Float to Int conversion must be rejected");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("expected Int"))
        );
    }

    #[test]
    fn static_cast_rejects_dynamic_sources_with_actionable_guidance() {
        let syntax = parse_program("fn convert(value) -> String { value as String }")
            .expect("source parses");
        let arena = HirArena::new();
        let errors = lower_to_hir(&arena, &syntax, None)
            .expect_err("a static cast from Any cannot be proven");
        assert!(errors.iter().any(|error| {
            error.message.contains("use `as?`") && error.message.contains("`as!`")
        }));
    }

    #[test]
    fn null_checks_narrow_optional_locals_inside_the_selected_branch() {
        let consume = crate::BuiltinId(1);
        let manifest = crate::BuiltinManifest::new([("consume", consume)]).with_type_metadata(
            crate::SymbolManifest::default(),
            BTreeMap::from([(
                consume,
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![ScriptType::String],
                    variadic: None,
                    result: ScriptType::Unit,
                },
            )]),
            Vec::new(),
        );
        let syntax =
            parse_program("let name: String? = \"alice\"\nif name != null { consume(name) }")
                .expect("source parses");
        let arena = HirArena::new();
        lower_to_hir(&arena, &syntax, Some(&manifest))
            .expect("the non-null branch must see String rather than String?");
    }

    #[test]
    fn bare_null_requires_an_explicit_optional_type() {
        let syntax = parse_program("let name = null").expect("source parses");
        let arena = HirArena::new();
        let errors = lower_to_hir(&arena, &syntax, None)
            .expect_err("none cannot determine its own element type");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("explicit optional type"))
        );
    }

    #[test]
    fn list_inference_joins_int_and_float_as_float() {
        let syntax = parse_program("let values = [1, 2.5]").expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("list lowers");
        assert_eq!(
            hir.types.get(hir.locals[0].ty),
            Some(&ScriptType::List(Box::new(ScriptType::Float)))
        );
    }

    #[test]
    fn generic_struct_literals_are_target_typed_and_nominal() {
        let valid = parse_program(
            "type Player<T> = .{ name: T, score: Int }\nlet player: Player<String?> = .{ name: null, score: 12 }",
        )
        .expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &valid, None).expect("generic struct instantiates");
        assert!(matches!(
            hir.types.get(hir.locals[0].ty),
            Some(ScriptType::Struct { arguments, .. })
                if arguments == &vec![ScriptType::Optional(Box::new(ScriptType::String))]
        ));

        let map_conversion = parse_program(
            "let value = .{ name: \"alice\", score: 12 }\nlet erased: Map<String, Any> = value",
        )
        .expect("source parses");
        let arena = HirArena::new();
        lower_to_hir(&arena, &map_conversion, None)
            .expect("an anonymous struct can erase to a string-keyed map");

        let invalid = parse_program(
            "type Player<T> = .{ name: T, score: Int }\nlet value = .{ name: \"alice\", score: 12 }\nlet player: Player<String> = value",
        )
        .expect("source parses");
        let arena = HirArena::new();
        lower_to_hir(&arena, &invalid, None)
            .expect_err("an anonymous struct must not become a nominal Player implicitly");
    }

    #[test]
    fn raw_generic_types_are_rejected() {
        let syntax = parse_program(
            "type Box<T> = .{ value: T }\nlet values: List = []\nlet boxed: Box = .{ value: 1 }",
        )
        .expect("source parses");
        let arena = HirArena::new();
        let errors = lower_to_hir(&arena, &syntax, None).expect_err("raw List is invalid");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("exactly one type argument"))
        );
        assert!(errors.iter().any(|error| {
            error.message.contains("raw generic types are not allowed")
                && error.message.contains("Box")
        }));
    }

    #[test]
    fn recursive_generic_aliases_report_an_error_instead_of_recursing_forever() {
        let syntax = parse_program("type Loop<T> = Loop<T>\nglobal value: Loop<String>")
            .expect("source parses");
        let arena = HirArena::new();
        let errors =
            lower_to_hir(&arena, &syntax, None).expect_err("recursive aliases are invalid");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("recursive type alias `Loop`"))
        );
    }
}
