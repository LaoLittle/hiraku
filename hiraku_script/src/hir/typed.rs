#[path = "enums.rs"]
mod enums;
#[path = "strings.rs"]
mod strings;

use std::collections::{BTreeMap, BTreeSet};

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

    pub fn alloc<T>(&self, value: T) -> &T {
        self.bump.alloc(value)
    }

    pub fn alloc_slice_copy<T: Copy>(&self, values: &[T]) -> &[T] {
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
    When {
        value: &'hir HirExpr<'hir>,
        type_name: SymbolId,
        arms: &'hir [HirWhenArm<'hir>],
    },
    Intrinsic {
        operation: crate::intrinsics::Intrinsic,
        argument: &'hir HirExpr<'hir>,
    },
    GuardNever(&'hir HirExpr<'hir>),
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
    NonNull(&'hir HirExpr<'hir>),
    OptionalSome(&'hir HirExpr<'hir>),
    Cast {
        value: &'hir HirExpr<'hir>,
        target: &'hir ScriptType,
        mode: CastMode,
    },
    Call {
        type_bindings: &'hir [(SymbolId, TypeId)],
        callee: &'hir HirExpr<'hir>,
        arguments: &'hir [HirArgument<'hir>],
        function: ResolvedFunction,
    },
    Tuple(&'hir [&'hir HirExpr<'hir>]),
    List(&'hir [&'hir HirExpr<'hir>]),
    Variant {
        type_name: SymbolId,
        variant: SymbolId,
        values: &'hir [&'hir HirExpr<'hir>],
    },
    Map {
        type_name: Option<SymbolId>,
        fields: &'hir [(SymbolId, &'hir HirExpr<'hir>)],
    },
    Lambda {
        parameters: &'hir [HirLocalId],
        body: &'hir HirBlock<'hir>,
        return_value: bool,
    },
    Block(&'hir HirBlock<'hir>),
    Binary {
        left: &'hir HirExpr<'hir>,
        op: BinaryOp,
        right: &'hir HirExpr<'hir>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirWhenArm<'hir> {
    pub variant: SymbolId,
    pub bindings: &'hir [HirLocalId],
    pub body: &'hir HirBlock<'hir>,
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
    Return(Option<&'hir HirExpr<'hir>>),
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
    pub mutable: bool,
    pub embedding_owned: bool,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HirFunction<'hir> {
    pub track_caller: bool,
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
    let program = super::prepare::prepare(program)?;
    Lowerer::new(arena, &program, manifest).lower(&program)
}

pub(crate) fn collect_project_exports(
    program: &Program,
    namespace: Option<&str>,
    manifest: &BuiltinManifest,
    interface: &mut crate::project::ProjectInterface,
) -> Result<(), Vec<LoweringError>> {
    let program = super::prepare::prepare(program)?;
    let arena = HirArena::new();
    let mut lowerer = Lowerer::new(&arena, &program, Some(manifest));
    lowerer.symbols = SymbolInterner::from_manifest(super::normalize_program_symbols(
        &program,
        Some(&interface.symbols),
    ))
    .expect("project symbols are unique");
    lowerer.declare_types(&program);
    lowerer.declare_functions(&program);
    for function in &lowerer.functions {
        if !function.exported {
            continue;
        }
        let name = lowerer
            .symbols
            .resolve(function.name)
            .expect("function is interned");
        let name = namespace.map_or_else(
            || name.to_string(),
            |namespace| format!("{namespace}.{name}"),
        );
        let symbol = lowerer.symbols.intern(&name);
        let signature = crate::FunctionSignature {
            receiver: None,
            parameters: function.parameters.clone(),
            variadic: None,
            result: function.result.clone(),
        };
        interface
            .type_parameters
            .insert(symbol, function.type_parameters.clone());
        if interface.functions.insert(symbol, signature).is_some() {
            lowerer.errors.push(LoweringError {
                message: format!("duplicate exported function `{name}`"),
                span: function.span,
            });
        }
    }
    interface.symbols = lowerer.symbols.manifest();
    if lowerer.errors.is_empty() {
        Ok(())
    } else {
        Err(lowerer.errors)
    }
}

pub(crate) fn lower_with_project_interface<'hir>(
    arena: &'hir HirArena,
    program: &Program,
    manifest: &BuiltinManifest,
    interface: &crate::project::ProjectInterface,
) -> Result<HirProgram<'hir>, Vec<LoweringError>> {
    let program = super::prepare::prepare(program)?;
    let mut lowerer = Lowerer::new(arena, &program, Some(manifest));
    lowerer.symbols = SymbolInterner::from_manifest(super::normalize_program_symbols(
        &program,
        Some(&interface.symbols),
    ))
    .expect("project symbols are unique");
    lowerer.external_functions = interface.functions.clone();
    lowerer.external_type_parameters = interface.type_parameters.clone();
    lowerer.lower(&program)
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
    external_functions: BTreeMap<SymbolId, crate::FunctionSignature>,
    external_type_parameters: BTreeMap<SymbolId, Vec<SymbolId>>,
    lowered_functions: Vec<HirFunction<'hir>>,
    scopes: Vec<BTreeMap<SymbolId, HirLocalId>>,
    global_names: BTreeMap<SymbolId, HirGlobalId>,
    function_names: BTreeMap<SymbolId, HirFunctionId>,
    methods: BTreeMap<(TypeId, SymbolId), HirFunctionId>,
    static_methods: BTreeMap<(TypeId, SymbolId), HirFunctionId>,
    aliases: BTreeMap<String, TypeAliasDeclaration>,
    enums: BTreeMap<String, (Vec<String>, Vec<crate::ast::EnumVariant>)>,
    type_parameters: Vec<BTreeMap<String, ScriptType>>,
    type_expansions: Vec<String>,
    named_imports: BTreeMap<String, String>,
    wildcard_import: Option<String>,
    current_function: Option<HirFunctionId>,
    return_context: Vec<Option<ScriptType>>,
    refinements: Vec<BTreeMap<HirLocalId, ScriptType>>,
    errors: Vec<LoweringError>,
    numeric_hints: BTreeSet<usize>,
    inferred_numeric: BTreeSet<HirLocalId>,
    /// Evidence for explicit casts only; this never changes an Any binding's
    /// public type or permits implicit calls/member access.
    immutable_any_types: BTreeMap<HirLocalId, ScriptType>,
    numeric_dependencies: BTreeMap<usize, BTreeSet<usize>>,
    float_requirements: BTreeSet<usize>,
    numeric_resolved: bool,
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
            external_functions: BTreeMap::new(),
            external_type_parameters: BTreeMap::new(),
            lowered_functions: Vec::new(),
            scopes: vec![BTreeMap::new()],
            global_names: BTreeMap::new(),
            function_names: BTreeMap::new(),
            methods: BTreeMap::new(),
            static_methods: BTreeMap::new(),
            aliases: BTreeMap::new(),
            enums: BTreeMap::new(),
            type_parameters: Vec::new(),
            type_expansions: Vec::new(),
            named_imports,
            wildcard_import,
            current_function: None,
            return_context: Vec::new(),
            refinements: Vec::new(),
            errors: import_errors,
            numeric_hints: BTreeSet::new(),
            inferred_numeric: BTreeSet::new(),
            immutable_any_types: BTreeMap::new(),
            numeric_dependencies: BTreeMap::new(),
            float_requirements: BTreeSet::new(),
            numeric_resolved: false,
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
                    Stmt::Enum { .. }
                        | Stmt::Import { .. }
                        | Stmt::TypeAlias { .. }
                        | Stmt::Struct { .. }
                        | Stmt::Function { .. }
                )
            }),
            Span::new(
                0,
                u32::try_from(source_end(source)).expect("source span exceeds u32 capacity"),
            ),
            false,
        );
        if !self.numeric_resolved && !self.float_requirements.is_empty() {
            let mut hints = self.float_requirements.clone();
            let mut pending = hints.iter().copied().collect::<Vec<_>>();
            while let Some(binding) = pending.pop() {
                if let Some(dependencies) = self.numeric_dependencies.get(&binding) {
                    for dependency in dependencies {
                        if hints.insert(*dependency) {
                            pending.push(*dependency);
                        }
                    }
                }
            }
            // Solve use-site constraints before producing the final typed HIR. The
            // second pass also rechecks earlier uses against the resolved types.
            let mut resolved = Self::new(self.arena, source, self.manifest);
            resolved.numeric_hints = hints;
            resolved.numeric_resolved = true;
            return resolved.lower(source);
        }
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
            if let Stmt::Enum {
                name,
                type_parameters,
                variants,
                span,
            } = statement
            {
                if self
                    .enums
                    .insert(name.clone(), (type_parameters.clone(), variants.clone()))
                    .is_some()
                    || self.aliases.contains_key(name)
                {
                    self.error(format!("type `{name}` is defined more than once"), *span);
                }
                continue;
            }
            let (Stmt::TypeAlias {
                name,
                type_parameters,
                ty,
                span,
            }
            | Stmt::Struct {
                name,
                type_parameters,
                ty,
                span,
            }) = statement
            else {
                continue;
            };
            if self.aliases.contains_key(name) || self.enums.contains_key(name) {
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
                self.push_global(&name, ty, true, true, None);
            }
        }
        for statement in &program.statements {
            let Stmt::Global {
                mutable,
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
            self.push_global(name, ty, *mutable, false, Some(*span));
        }
    }

    fn push_global(
        &mut self,
        name: &str,
        ty: ScriptType,
        mutable: bool,
        embedding_owned: bool,
        span: Option<Span>,
    ) {
        let name = self.symbol(name);
        let ty = self.types.intern(ty);
        let id = HirGlobalId(self.globals.len() as u32);
        self.globals.push(HirGlobal {
            name,
            ty,
            mutable,
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
            let parameter_types: Vec<ScriptType> = parameters
                .iter()
                .map(|parameter| {
                    parameter
                        .ty
                        .as_ref()
                        .and_then(|ty| self.type_from_ast(ty))
                        .unwrap_or(ScriptType::Any)
                })
                .collect();
            if let Some((owner_name, method)) = name.split_once("::") {
                let owner = TypeExpr {
                    kind: TypeExprKind::Named(owner_name.into()),
                    span: *span,
                };
                if let Some(owner) = self.type_from_ast(&owner) {
                    let owner = self.types.intern(owner);
                    let method = self.symbol(method);
                    if self.methods.contains_key(&(owner, method))
                        || self.static_methods.contains_key(&(owner, method))
                    {
                        self.error("method is already implemented for this type", *span);
                    }
                    if parameters
                        .first()
                        .is_some_and(|parameter| parameter.name == "self")
                    {
                        self.methods.insert((owner, method), id);
                    } else {
                        self.static_methods.insert((owner, method), id);
                    }
                } else {
                    self.error("impl refers to an unknown type", *span);
                }
            }
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
                attributes,
                name,
                type_parameters,
                parameters,
                return_type,
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
            let expected_result = self.functions[function_id.0 as usize].result.clone();
            self.return_context
                .push(return_type.as_ref().map(|_| expected_result.clone()));
            let body = if return_type.is_some()
                && !matches!(expected_result, ScriptType::Unit | ScriptType::Never)
            {
                let mut statements = Vec::new();
                for (index, statement) in body.statements.iter().enumerate() {
                    if index + 1 == body.statements.len()
                        && let Stmt::Expr(expression) = statement
                    {
                        let value =
                            self.lower_expression_expected(expression, Some(&expected_result));
                        self.check_assignment(
                            &expected_result,
                            &self.expression_type(value).clone(),
                            expression.span,
                        );
                        statements.push(self.arena.alloc(HirStmt {
                            kind: HirStmtKind::Expr(value),
                            span: expression.span,
                        }) as &HirStmt<'hir>);
                    } else if let Some(statement) = self.lower_statement(statement) {
                        statements.push(statement);
                    }
                }
                self.arena.alloc(HirBlock {
                    statements: self.arena.alloc_slice_copy(&statements),
                    span: body.span,
                })
            } else if return_type.is_none() {
                self.lower_value_block(body, false)
            } else {
                self.lower_block(body, false)
            };
            self.return_context.pop();
            let inferred_result =
                self.checked_callable_result(body, return_type.as_ref().map(|_| &expected_result));
            if return_type.is_some() && expected_result != ScriptType::Unit {
                self.check_assignment(&expected_result, &inferred_result, body.span);
            }
            if return_type.is_none() {
                self.functions[function_id.0 as usize].result = inferred_result;
            }
            if self.functions[function_id.0 as usize].result == ScriptType::Never
                && !self.block_diverges(body)
            {
                self.error("a function declared `Never` must not return; terminate every path with a Never call or an infinite loop", body.span);
            }
            let declaration = &self.functions[function_id.0 as usize];
            let parameters = self.arena.alloc_slice_copy(&lowered_parameters);
            let result = self.types.intern(declaration.result.clone());
            self.lowered_functions.push(HirFunction {
                track_caller: attributes
                    .iter()
                    .any(|attribute| attribute.name == "trackCaller"),
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

    fn block_diverges(&self, block: &HirBlock<'hir>) -> bool {
        block
            .statements
            .iter()
            .any(|statement| match statement.kind {
                HirStmtKind::Expr(value)
                | HirStmtKind::Return(Some(value))
                | HirStmtKind::Let { value, .. }
                | HirStmtKind::Assign { value, .. } => {
                    self.expression_type(value) == &ScriptType::Never
                }
                HirStmtKind::Global {
                    value: Some(value), ..
                } => self.expression_type(value) == &ScriptType::Never,
                HirStmtKind::If {
                    then_block,
                    else_block: Some(else_block),
                    ..
                } => self.block_diverges(then_block) && self.block_diverges(else_block),
                HirStmtKind::While { condition, .. } => {
                    matches!(condition.kind, HirExprKind::Literal(HirLiteral::Bool(true)))
                }
                _ => false,
            })
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

    fn lower_value_block(&mut self, block: &Block, scoped: bool) -> &'hir HirBlock<'hir> {
        if scoped {
            self.scopes.push(BTreeMap::new());
        }
        let mut statements = Vec::new();
        for (index, statement) in block.statements.iter().enumerate() {
            if index + 1 == block.statements.len()
                && let Stmt::Expr(expression) = statement
            {
                let value = self.lower_expression(expression);
                statements.push(self.arena.alloc(HirStmt {
                    kind: HirStmtKind::Expr(value),
                    span: expression.span,
                }) as &HirStmt<'hir>);
            } else if let Some(statement) = self.lower_statement(statement) {
                statements.push(statement);
            }
        }
        if scoped {
            self.scopes.pop();
        }
        self.arena.alloc(HirBlock {
            statements: self.arena.alloc_slice_copy(&statements),
            span: block.span,
        })
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
            Stmt::Return { value, span } => {
                let expected = self.return_context.last().cloned().flatten();
                if self.return_context.is_empty() {
                    self.error("return is only allowed inside a function or closure", *span);
                }
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression_expected(value, expected.as_ref()));
                if let Some(expected) = expected {
                    let actual = value
                        .map(|value| self.expression_type(value).clone())
                        .unwrap_or(ScriptType::Unit);
                    self.check_assignment(&expected, &actual, *span);
                }
                (HirStmtKind::Return(value), *span)
            }
            Stmt::Import { span, .. } => {
                self.error("imports are only allowed at module scope", *span);
                return None;
            }
            Stmt::Enum { .. } | Stmt::TypeAlias { .. } | Stmt::Struct { .. } => return None,
            Stmt::Impl { span, .. } | Stmt::Property { span, .. } | Stmt::Const { span, .. } => {
                self.error("impl declarations are only allowed at module scope", *span);
                return None;
            }
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
                    .and_then(|ty| self.type_from_ast(ty))
                    .or_else(|| {
                        self.numeric_hints
                            .contains(&span.start)
                            .then_some(ScriptType::Float)
                    });
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
                let evidence = (!*mutable && ty == ScriptType::Any)
                    .then(|| self.cast_source_type(value))
                    .filter(stable_cast_evidence);
                let local = self.declare_local(name, ty, *mutable, *span);
                if let Some(evidence) = evidence {
                    self.immutable_any_types.insert(local, evidence);
                }
                if type_annotation.is_none()
                    && matches!(inferred, ScriptType::Int | ScriptType::Float)
                {
                    let dependencies = self.numeric_sources(value);
                    self.inferred_numeric.insert(local);
                    self.numeric_dependencies.insert(span.start, dependencies);
                }
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
                if let ExprKind::Member { object, name } = &target.kind {
                    let object = self.lower_expression(object);
                    let getter = self.symbol(&format!("get#{name}"));
                    let setter = self.symbol(&format!("set#{name}"));
                    if self.methods.contains_key(&(object.ty, getter)) {
                        let Some(setter) = self.methods.get(&(object.ty, setter)).copied() else {
                            self.error(format!("computed property `{name}` is read-only"), *span);
                            return None;
                        };
                        let expected = self.functions[setter.0 as usize].parameters[1].clone();
                        let value = self.lower_expression_expected(value, Some(&expected));
                        self.check_assignment(
                            &expected,
                            &self.expression_type(value).clone(),
                            value.span,
                        );
                        let call = self.accessor_call(setter, &[object, value], *span);
                        return Some(self.arena.alloc(HirStmt {
                            kind: HirStmtKind::Expr(call),
                            span: *span,
                        }));
                    }
                }
                let target = self.lower_place(target)?;
                // Only rebinding is restricted. A field assignment deliberately does
                // not recurse to the root: `let actor = ...; actor.position = ...` is valid.
                let immutable = match target {
                    HirPlace::Local(id) if !self.locals[id.0 as usize].mutable => {
                        Some(self.locals[id.0 as usize].name)
                    }
                    HirPlace::Global(id) if !self.globals[id.0 as usize].mutable => {
                        Some(self.globals[id.0 as usize].name)
                    }
                    _ => None,
                };
                if let Some(name) = immutable {
                    let name = self.symbols.resolve(name).unwrap_or("<unknown>");
                    self.error(format!("cannot reassign immutable binding `{name}`; declare it with `var` (or `global var` for a global binding); `let` still permits object field updates"), *span);
                    return None;
                }
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
                // The existing bare-string statement hook consumes a template.
                // Ordinary expression contexts use eager String interpolation.
                let value = if let ExprKind::String(text) = &expression.kind {
                    self.alloc_expression(
                        HirExprKind::Literal(HirLiteral::String(self.arena.alloc_str(text))),
                        ScriptType::String,
                        expression.span,
                    )
                } else {
                    self.lower_expression(expression)
                };
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
        self.lower_expression_in_context(expression, None)
    }

    fn lower_expression_in_context(
        &mut self,
        expression: &Expr,
        expected_result: Option<&ScriptType>,
    ) -> &'hir HirExpr<'hir> {
        if let Some(value) = self.lower_enum_constructor(expression, None) {
            return value;
        }
        let (kind, ty) = match &expression.kind {
            ExprKind::When { value, arms } => {
                return self.lower_when(value, arms, expected_result, expression.span);
            }
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
            ExprKind::String(value) => return self.lower_string(value, expression.span),
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
                            type_bindings: &[],
                            callee,
                            arguments: &[],
                            function: ResolvedFunction::Builtin(member.builtin),
                        },
                        ty,
                        expression.span,
                    );
                }
                self.error(
                    format!("cannot infer the type of `.{name}`; add an explicit type annotation or use it as an argument with a known parameter type"),
                    expression.span,
                );
                let ty = ScriptType::Never;
                (HirExprKind::Symbol(symbol), ty)
            }
            ExprKind::Not(value) => {
                let value = self.lower_expression(value);
                if !ScriptType::Bool.accepts(self.expression_type(value)) {
                    self.error(
                        format!(
                            "operator `!` expects Bool, got {:?}",
                            self.expression_type(value)
                        ),
                        value.span,
                    );
                }
                let falsy = self.alloc_expression(
                    HirExprKind::Literal(HirLiteral::Bool(false)),
                    ScriptType::Bool,
                    expression.span,
                );
                (
                    HirExprKind::Binary {
                        left: value,
                        op: BinaryOp::Equal,
                        right: falsy,
                    },
                    ScriptType::Bool,
                )
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
                    && let Some(symbol) = self.external_selector(expression)
                    && let Some(signature) = self.external_functions.get(&symbol)
                {
                    let ty = ScriptType::Callable {
                        parameters: signature.parameters.clone(),
                        result: Box::new(signature.result.clone()),
                    };
                    return self.alloc_expression(
                        HirExprKind::Unresolved(symbol),
                        ty,
                        expression.span,
                    );
                }
                if matches!(expression.kind, ExprKind::Member { .. }) {
                    if let Some(method) =
                        self.resolve_static_script_method(object, name, expression.span)
                    {
                        if self
                            .symbols
                            .resolve(self.functions[method.0 as usize].name)
                            .is_some_and(|name| name.contains("::get#"))
                        {
                            return self.accessor_call(method, &[], expression.span);
                        }
                        return self.alloc_expression(
                            HirExprKind::Function(method),
                            self.function_type(method),
                            expression.span,
                        );
                    }
                }
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
                let getter = self.symbol(&format!("get#{name}"));
                if !safe && let Some(getter) = self.methods.get(&(object.ty, getter)).copied() {
                    return self.accessor_call(getter, &[object], expression.span);
                }
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
                return self.lower_elvis(value, fallback, expected_result, expression.span);
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
                let source = self.cast_source_type(value);
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
                if *mode == CastMode::Static
                    && matches!(target, ScriptType::Callable { .. })
                    && matches!(cast_certainty(&source, &target), CastCertainty::Always)
                {
                    // A proven function cast changes only the static view;
                    // it does not wrap or replace the closure object.
                    return self.alloc_expression(value.kind, target, expression.span);
                }
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
                let mut explicit_types = type_arguments
                    .iter()
                    .filter_map(|ty| self.type_from_ast(ty))
                    .collect::<Vec<_>>();
                if let ExprKind::Ident(name) = &syntax_callee.kind {
                    if name.starts_with("__builtin_") && crate::intrinsics::resolve(name).is_none()
                    {
                        self.error(
                            format!("unknown compiler intrinsic `{name}`"),
                            expression.span,
                        );
                    }
                }
                if let ExprKind::Ident(name) = &syntax_callee.kind
                    && let Some(definition) = crate::intrinsics::resolve(name)
                {
                    if arguments.len() != 1
                        || trailing_block.is_some()
                        || !type_arguments.is_empty()
                    {
                        self.error(
                            format!("intrinsic `{name}` requires exactly one argument"),
                            expression.span,
                        );
                        return self.alloc_expression(
                            HirExprKind::Literal(HirLiteral::Unit),
                            ScriptType::Any,
                            expression.span,
                        );
                    }
                    let argument = self.lower_expression_expected(
                        &arguments[0].value,
                        Some(&definition.parameter),
                    );
                    self.check_assignment(
                        &definition.parameter,
                        &self.expression_type(argument).clone(),
                        arguments[0].span,
                    );
                    return self.alloc_expression(
                        HirExprKind::Intrinsic {
                            operation: definition.operation,
                            argument,
                        },
                        definition.result.clone(),
                        expression.span,
                    );
                }
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
                let mut callee = self.lower_expression(syntax_callee);
                let mut function = self.resolve_call(expression);
                if let HirExprKind::Function(id) = callee.kind {
                    function = ResolvedFunction::User(id);
                }
                let mut receiver = None;
                if let HirExprKind::Member { object, member, .. } = callee.kind {
                    // Receiver-qualified native methods do not share a global
                    // method name. Resolve using the statically known owner,
                    // including receivers produced by fluent calls.
                    if let ScriptType::Named(owner) = self.expression_type(object)
                        && let Some(manifest) = self.manifest
                        && let Some(owner_name) = self.symbols.resolve(*owner)
                        && let Some(member_name) = self.symbols.resolve(member)
                        && let Some(builtin) = manifest.resolve_selector(owner_name, member_name)
                        && manifest
                            .signature(builtin)
                            .is_some_and(|s| s.receiver.is_some())
                    {
                        function = ResolvedFunction::Builtin(builtin);
                    }
                    if self.static_methods.contains_key(&(object.ty, member)) {
                        self.error(
                            "static methods must be called on their type, not an instance",
                            syntax_callee.span,
                        );
                    }
                    if let Some(id) = self.methods.get(&(object.ty, member)).copied() {
                        function = ResolvedFunction::User(id);
                        receiver = Some(object);
                        callee = self.alloc_expression(
                            HirExprKind::Function(id),
                            ScriptType::Function,
                            syntax_callee.span,
                        );
                    }
                }
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
                if explicit_types.is_empty()
                    && let Some(expected) = expected_result.filter(|ty| **ty != ScriptType::Any)
                    && let Some((type_parameters, _, result)) = self.generic_signature(function)
                    && !type_parameters.is_empty()
                {
                    let mut substitutions = BTreeMap::new();
                    infer_type_argument(&result, expected, &mut substitutions);
                    if let Some(types) = type_parameters
                        .iter()
                        .map(|parameter| substitutions.get(parameter).cloned())
                        .collect::<Option<Vec<_>>>()
                    {
                        explicit_types = types;
                    }
                }
                let (mut expected_parameters, expected_variadic) = match function {
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
                    _ => match self.expression_type(callee) {
                        ScriptType::Callable { parameters, .. } => (Some(parameters.clone()), None),
                        _ => (None, None),
                    },
                };
                if !explicit_types.is_empty()
                    && let Some((type_parameters, _, _)) = self.generic_signature(function)
                    && let Some(parameters) = &mut expected_parameters
                {
                    let substitutions = type_parameters
                        .iter()
                        .copied()
                        .zip(explicit_types.iter().cloned())
                        .collect();
                    for parameter in parameters {
                        *parameter = substitute_type(parameter, &substitutions);
                    }
                }
                let mut arguments = arguments
                    .iter()
                    .enumerate()
                    .map(|(index, argument)| HirArgument {
                        label: argument.label.as_deref().map(|label| self.symbol(label)),
                        value: self.lower_expression_expected(
                            &argument.value,
                            expected_parameters
                                .as_ref()
                                .and_then(|parameters| {
                                    parameters.get(index + usize::from(receiver.is_some()))
                                })
                                .or(expected_variadic.as_ref()),
                        ),
                        span: argument.span,
                    })
                    .collect::<Vec<_>>();
                if let Some(receiver) = receiver {
                    arguments.insert(
                        0,
                        HirArgument {
                            label: None,
                            value: receiver,
                            span: receiver.span,
                        },
                    );
                }
                if let Some(block) = trailing_block {
                    self.return_context.push(None);
                    let block = self.lower_block(block, true);
                    self.return_context.pop();
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
                let ty = if function == ResolvedFunction::Dynamic {
                    match self.expression_type(callee) {
                        ScriptType::Callable { result, .. } => (**result).clone(),
                        _ => self.call_result(function, arguments, &explicit_types),
                    }
                } else {
                    self.call_result(function, arguments, &explicit_types)
                };
                let bindings = if let Some((type_parameters, parameters, _)) =
                    self.generic_signature(function)
                {
                    if explicit_types.is_empty() {
                        infer_type_arguments(&parameters, arguments, |value| {
                            self.expression_type(value).clone()
                        })
                    } else {
                        type_parameters
                            .iter()
                            .copied()
                            .zip(explicit_types.iter().cloned())
                            .collect()
                    }
                } else {
                    BTreeMap::new()
                };
                let bindings = bindings
                    .into_iter()
                    .map(|(name, ty)| (name, self.types.intern(ty)))
                    .collect::<Vec<_>>();
                (
                    HirExprKind::Call {
                        type_bindings: self.arena.alloc_slice_copy(&bindings),
                        callee,
                        arguments,
                        function,
                    },
                    ty,
                )
            }
            ExprKind::Tuple(values) | ExprKind::List(values) => {
                let syntax_values = values;
                let mut values = values
                    .iter()
                    .map(|value| self.lower_expression(value))
                    .collect::<Vec<_>>();
                if matches!(expression.kind, ExprKind::List(_))
                    && values
                        .iter()
                        .any(|value| self.expression_type(value) == &ScriptType::Float)
                {
                    for (syntax, value) in syntax_values.iter().zip(&mut values) {
                        if self.expression_type(value) == &ScriptType::Int {
                            *value =
                                self.lower_expression_expected(syntax, Some(&ScriptType::Float));
                            self.check_assignment(
                                &ScriptType::Float,
                                &self.expression_type(value).clone(),
                                syntax.span,
                            );
                        }
                    }
                }
                let ty = if matches!(expression.kind, ExprKind::Tuple(_)) {
                    ScriptType::TupleOf(
                        values
                            .iter()
                            .map(|value| self.expression_type(value).clone())
                            .collect(),
                    )
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
                if let ExprKind::TypedStructLiteral { type_name, .. } = &expression.kind {
                    if let Some(expected @ ScriptType::Struct { .. }) =
                        self.instantiate_named_type(type_name, &[], expression.span)
                    {
                        let literal = Expr {
                            kind: ExprKind::StructLiteral(fields.clone()),
                            span: expression.span,
                        };
                        return self.lower_expression_expected(&literal, Some(&expected));
                    }
                }
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
                self.return_context.push(None);
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
                let body = self.lower_value_block(body, false);
                self.return_context.pop();
                self.scopes.pop();
                let result = self.checked_callable_result(body, None);
                let signature = ScriptType::Callable {
                    parameters: parameters
                        .iter()
                        .map(|id| {
                            self.types
                                .get(self.locals[id.0 as usize].ty)
                                .expect("local type exists")
                                .clone()
                        })
                        .collect(),
                    result: Box::new(result),
                };
                (
                    HirExprKind::Lambda {
                        parameters: self.arena.alloc_slice_copy(&parameters),
                        body,
                        return_value: !matches!(
                            self.closure_result(body),
                            ScriptType::Unit | ScriptType::Never
                        ),
                    },
                    signature,
                )
            }
            ExprKind::Block(block) => {
                self.return_context.push(None);
                let block = self.lower_value_block(block, true);
                self.return_context.pop();
                let result = self.checked_callable_result(block, None);
                (
                    HirExprKind::Block(block),
                    ScriptType::Callable {
                        parameters: Vec::new(),
                        result: Box::new(result),
                    },
                )
            }
            ExprKind::Binary { left, op, right } => {
                let left_syntax = left;
                let right_syntax = right;
                let mut left = self.lower_expression(left);
                let (truthy, falsy) = self.condition_refinements(left_syntax);
                self.refinements.push(match op {
                    BinaryOp::And => truthy,
                    BinaryOp::Or => falsy,
                    _ => BTreeMap::new(),
                });
                let mut right = self.lower_expression(right);
                self.refinements.pop();
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    for operand in [left, right] {
                        if !ScriptType::Bool.accepts(self.expression_type(operand)) {
                            self.error(
                                format!(
                                    "logical operator expects Bool, got {:?}",
                                    self.expression_type(operand)
                                ),
                                operand.span,
                            );
                        }
                    }
                }
                if self.expression_type(left) == &ScriptType::Float
                    && self.expression_type(right) == &ScriptType::Int
                {
                    right = self.lower_expression_expected(right_syntax, Some(&ScriptType::Float));
                    self.check_assignment(
                        &ScriptType::Float,
                        &self.expression_type(right).clone(),
                        right.span,
                    );
                } else if self.expression_type(right) == &ScriptType::Float
                    && self.expression_type(left) == &ScriptType::Int
                {
                    left = self.lower_expression_expected(left_syntax, Some(&ScriptType::Float));
                    self.check_assignment(
                        &ScriptType::Float,
                        &self.expression_type(left).clone(),
                        left.span,
                    );
                }
                if matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide
                ) {
                    let left_type = self.expression_type(left);
                    let right_type = self.expression_type(right);
                    let strings = *op == BinaryOp::Add
                        && left_type == &ScriptType::String
                        && right_type == &ScriptType::String;
                    let numeric = |ty: &ScriptType| {
                        // Preserve the existing numeric path for unresolved
                        // host/global signatures; it still checks VM operands.
                        matches!(
                            ty,
                            ScriptType::Int
                                | ScriptType::Float
                                | ScriptType::Never
                                | ScriptType::Any
                        )
                    };
                    if !strings && !(numeric(left_type) && numeric(right_type)) {
                        self.error(
                            format!("arithmetic operator {op:?} cannot accept {left_type:?} and {right_type:?}; use numeric operands, or String + String for concatenation (convert other values explicitly with toString())"),
                            expression.span,
                        );
                    }
                }
                if matches!(
                    op,
                    BinaryOp::Less
                        | BinaryOp::LessEqual
                        | BinaryOp::Greater
                        | BinaryOp::GreaterEqual
                ) {
                    for operand in [left, right] {
                        if !matches!(
                            self.expression_type(operand),
                            ScriptType::Int | ScriptType::Float | ScriptType::Never
                        ) {
                            self.error(
                                format!(
                                    "ordered comparison expects Int or Float, got {:?}",
                                    self.expression_type(operand)
                                ),
                                operand.span,
                            );
                        }
                    }
                }
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
                    let left_type = self.expression_type(left);
                    let right_type = self.expression_type(right);
                    if left_type != &ScriptType::Any
                        && right_type != &ScriptType::Any
                        && !left_type.accepts(right_type)
                        && !right_type.accepts(left_type)
                    {
                        self.error(format!("equality requires compatible operand types, got {left_type:?} and {right_type:?}"), expression.span);
                    }
                }
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
                            type_bindings: &[],
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
        if expected == Some(&ScriptType::Symbol)
            && let ExprKind::Symbol(name) = &expression.kind
        {
            let symbol = self.symbol(name);
            return self.alloc_expression(
                HirExprKind::Symbol(symbol),
                ScriptType::Symbol,
                expression.span,
            );
        }
        if let ExprKind::Elvis { value, fallback } = &expression.kind {
            return self.lower_elvis(value, fallback, expected, expression.span);
        }
        if let ExprKind::When { value, arms } = &expression.kind {
            return self.lower_when(value, arms, expected, expression.span);
        }
        if let Some(value) = self.lower_enum_constructor(expression, expected) {
            return value;
        }
        if matches!(expression.kind, ExprKind::Call { .. }) && {
            let function = self.resolve_call(expression);
            self.generic_signature(function)
                .is_some_and(|(parameters, _, _)| !parameters.is_empty())
        } {
            return self.lower_expression_in_context(expression, expected);
        }
        if let Some(ScriptType::TupleOf(types)) = expected
            && let ExprKind::Tuple(values) = &expression.kind
        {
            if types.len() != values.len() {
                self.error(
                    format!(
                        "tuple expects {} elements, got {}",
                        types.len(),
                        values.len()
                    ),
                    expression.span,
                );
            }
            let values = values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let value = self.lower_expression_expected(value, types.get(index));
                    if let Some(expected) = types.get(index) {
                        self.check_assignment(
                            expected,
                            &self.expression_type(value).clone(),
                            value.span,
                        );
                    }
                    value
                })
                .collect::<Vec<_>>();
            return self.alloc_expression(
                HirExprKind::Tuple(self.arena.alloc_slice_copy(&values)),
                ScriptType::TupleOf(types.clone()),
                expression.span,
            );
        }
        if let Some(ScriptType::Callable {
            parameters: expected_parameters,
            result,
        }) = expected
        {
            let (parameters, body) = match &expression.kind {
                ExprKind::Lambda { parameters, body } => (parameters.as_slice(), body),
                ExprKind::Block(body) => (&[][..], body),
                _ => return self.lower_expression(expression),
            };
            if parameters.len() != expected_parameters.len() {
                self.error(
                    format!(
                        "closure expects {} parameters, got {}",
                        expected_parameters.len(),
                        parameters.len()
                    ),
                    expression.span,
                );
            }
            self.scopes.push(BTreeMap::new());
            let locals = parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let expected = expected_parameters
                        .get(index)
                        .cloned()
                        .unwrap_or(ScriptType::Any);
                    let ty = parameter
                        .ty
                        .as_ref()
                        .and_then(|ty| self.type_from_ast(ty))
                        .unwrap_or_else(|| expected.clone());
                    self.check_assignment(&ty, &expected, parameter.span);
                    self.declare_local(&parameter.name, ty, false, parameter.span)
                })
                .collect::<Vec<_>>();
            let mut statements = Vec::new();
            self.return_context.push(Some((**result).clone()));
            for (index, statement) in body.statements.iter().enumerate() {
                if index + 1 == body.statements.len()
                    && **result != ScriptType::Unit
                    && let Stmt::Expr(value) = statement
                {
                    let value = self.lower_expression_expected(value, Some(result));
                    self.check_assignment(result, &self.expression_type(value).clone(), value.span);
                    statements.push(self.arena.alloc(HirStmt {
                        kind: HirStmtKind::Expr(value),
                        span: value.span,
                    }) as &HirStmt<'hir>);
                } else if let Some(statement) = self.lower_statement(statement) {
                    statements.push(statement);
                }
            }
            let body = self.arena.alloc(HirBlock {
                statements: self.arena.alloc_slice_copy(&statements),
                span: body.span,
            });
            self.return_context.pop();
            if **result != ScriptType::Unit {
                let actual = self.checked_callable_result(body, Some(result));
                self.check_assignment(result, &actual, body.span);
            }
            self.scopes.pop();
            return self.alloc_expression(
                HirExprKind::Lambda {
                    parameters: self.arena.alloc_slice_copy(&locals),
                    body,
                    return_value: !matches!(**result, ScriptType::Unit | ScriptType::Never),
                },
                expected.expect("callable context exists").clone(),
                expression.span,
            );
        }
        if let Some(ScriptType::Union(types)) = expected
            && types.contains(&ScriptType::Float)
            && !types.contains(&ScriptType::Int)
            && matches!(
                expression.kind,
                ExprKind::Number { .. } | ExprKind::UnaryMinus(_)
            )
        {
            return self.lower_expression_expected(expression, Some(&ScriptType::Float));
        }
        if expected == Some(&ScriptType::Float) {
            if let ExprKind::UnaryMinus(inner) = &expression.kind {
                let value = self.lower_expression_expected(inner, expected);
                return self.alloc_expression(
                    HirExprKind::UnaryMinus(value),
                    self.expression_type(value).clone(),
                    expression.span,
                );
            }
            if let ExprKind::Binary { left, op, right } = &expression.kind
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide
                )
            {
                let left = self.lower_expression_expected(left, expected);
                let right = self.lower_expression_expected(right, expected);
                self.check_assignment(
                    &ScriptType::Float,
                    &self.expression_type(left).clone(),
                    left.span,
                );
                self.check_assignment(
                    &ScriptType::Float,
                    &self.expression_type(right).clone(),
                    right.span,
                );
                return self.alloc_expression(
                    HirExprKind::Binary {
                        left,
                        op: *op,
                        right,
                    },
                    ScriptType::Float,
                    expression.span,
                );
            }
            if let ExprKind::Number {
                value,
                unit: NumberUnit::Scalar,
            } = &expression.kind
            {
                return self.alloc_expression(
                    HirExprKind::Literal(HirLiteral::Number {
                        value: *value,
                        unit: NumberUnit::Scalar,
                    }),
                    ScriptType::Float,
                    expression.span,
                );
            }
            let value = self.lower_expression(expression);
            self.float_requirements.extend(self.numeric_sources(value));
            return value;
        }
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
        if let Some(ScriptType::Optional(inner)) = expected
            && matches!(expression.kind, ExprKind::Null)
        {
            return self.alloc_expression(
                HirExprKind::Literal(HirLiteral::Null),
                ScriptType::Optional(inner.clone()),
                expression.span,
            );
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
            // Optional injection happens after contextual inference. Otherwise
            // a literal such as `6` is prematurely fixed to Int when the host
            // expects Float?, and compound literals lose their element context.
            // Null and explicit .some(...) were handled above.
            let value = if let Some(ScriptType::Optional(inner)) = expected {
                self.lower_expression_expected(expression, Some(inner))
            } else {
                self.lower_expression(expression)
            };
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
                    type_bindings: &[],
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
            self.return_context.push(None);
            let block = self.lower_block(block, true);
            self.return_context.pop();
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
                type_bindings: &[],
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
        if let ResolvedFunction::External(_) = function
            && let Some((type_parameters, parameters, _)) = self.generic_signature(function)
            && !type_parameters.is_empty()
        {
            let bindings = if explicit_types.is_empty() {
                infer_type_arguments(&parameters, arguments, |value| {
                    self.expression_type(value).clone()
                })
            } else {
                if explicit_types.len() != type_parameters.len() {
                    self.error("wrong number of generic type arguments", span);
                }
                type_parameters
                    .iter()
                    .copied()
                    .zip(explicit_types.iter().cloned())
                    .collect()
            };
            if parameters.len() != arguments.len() {
                self.error("wrong number of function arguments", span);
            }
            for parameter in type_parameters {
                if !bindings.contains_key(&parameter) {
                    self.error("cannot infer generic parameter; add a result annotation or explicit type arguments", span);
                }
            }
            for (expected, actual) in parameters.iter().zip(arguments) {
                self.check_assignment(
                    &substitute_type(expected, &bindings),
                    &self.expression_type(actual.value).clone(),
                    actual.span,
                );
            }
            return;
        }
        if function == ResolvedFunction::Dynamic && self.expression_type(callee) == &ScriptType::Any
        {
            self.error(
                "cannot call Any; explicitly cast to a function type such as `(Int) -> Int` using `as`, `as?`, or `as!` before calling",
                callee.span,
            );
            return;
        }
        if matches!(
            function,
            ResolvedFunction::Dynamic | ResolvedFunction::External(_)
        ) && let ScriptType::Callable { parameters, .. } = self.expression_type(callee).clone()
        {
            if parameters.len() != arguments.len() {
                self.error(
                    format!(
                        "function expects {} arguments, got {}",
                        parameters.len(),
                        arguments.len()
                    ),
                    span,
                );
            }
            for (expected, actual) in parameters.iter().zip(arguments) {
                self.check_assignment(
                    expected,
                    &self.expression_type(actual.value).clone(),
                    actual.span,
                );
            }
            return;
        }
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
                            "cannot infer generic parameter `{name}` from this call; add a result type annotation, explicit type arguments, or a value whose type determines it"
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
            && !expected.accepts_native_argument(self.expression_type(object))
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
            if !expected.accepts_native_argument(self.expression_type(actual.value)) {
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

    fn accessor_call(
        &mut self,
        function: HirFunctionId,
        values: &[&'hir HirExpr<'hir>],
        span: Span,
    ) -> &'hir HirExpr<'hir> {
        let callee =
            self.alloc_expression(HirExprKind::Function(function), ScriptType::Function, span);
        let arguments = values
            .iter()
            .map(|value| HirArgument {
                label: None,
                value,
                span: value.span,
            })
            .collect::<Vec<_>>();
        let arguments = self.arena.alloc_slice_copy(&arguments);
        let result = self.functions[function.0 as usize].result.clone();
        self.alloc_expression(
            HirExprKind::Call {
                type_bindings: &[],
                callee,
                arguments,
                function: ResolvedFunction::User(function),
            },
            result,
            span,
        )
    }

    fn resolve_static_script_method(
        &mut self,
        object: &Expr,
        method: &str,
        span: Span,
    ) -> Option<HirFunctionId> {
        let ExprKind::Ident(name) = &object.kind else {
            return None;
        };
        let symbol = self.symbol(name);
        // Value bindings take precedence over type names in expressions.
        if self.resolve_local(symbol).is_some() || self.global_names.contains_key(&symbol) {
            return None;
        }
        // A namespace/value name is not necessarily a type. This is a lookup,
        // not a type annotation, so a miss must not emit an unknown-type error.
        let owner = self.instantiate_named_type(name, &[], object.span)?;
        let owner = self.types.intern(owner);
        let method_symbol = self.symbol(method);
        let getter = self.symbol(&format!("get#{method}"));
        if let Some(function) = self.static_methods.get(&(owner, getter)) {
            return Some(*function);
        }
        if let Some(function) = self.static_methods.get(&(owner, method_symbol)) {
            return Some(*function);
        }
        if self.methods.contains_key(&(owner, method_symbol)) {
            self.error(
                format!(
                    "instance method `{method}` requires a receiver; call it on a {name} value"
                ),
                span,
            );
        } else if self.aliases.contains_key(name) {
            self.error(
                format!("type `{name}` has no static method `{method}`"),
                span,
            );
        }
        None
    }

    fn native_callable_type(&self, builtin: BuiltinId) -> ScriptType {
        self.manifest
            .and_then(|manifest| manifest.signature(builtin))
            .map_or(ScriptType::Any, |signature| ScriptType::Callable {
                parameters: signature
                    .receiver
                    .iter()
                    .cloned()
                    .chain(signature.parameters.iter().cloned())
                    .collect(),
                result: Box::new(signature.result.clone()),
            })
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
                self.function_type(function),
                span,
            );
        }
        if let Some(builtin) = self.manifest.and_then(|manifest| manifest.resolve(name)) {
            let ty = self.native_callable_type(builtin);
            return self.alloc_expression(HirExprKind::Builtin(builtin), ty, span);
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
        if let Some(builtin) = self
            .manifest
            .and_then(|manifest| manifest.resolve(&imported))
        {
            let ty = self.native_callable_type(builtin);
            return self.alloc_expression(HirExprKind::Builtin(builtin), ty, span);
        }
        if !self.external_functions.contains_key(&symbol) {
            self.error(
                format!(
                    "unknown identifier `{name}`; declare it or import its definition before use"
                ),
                span,
            );
            return self.alloc_expression(HirExprKind::Unresolved(symbol), ScriptType::Never, span);
        }
        let ty = self
            .external_functions
            .get(&symbol)
            .map_or(ScriptType::Any, |signature| ScriptType::Callable {
                parameters: signature.parameters.clone(),
                result: Box::new(signature.result.clone()),
            });
        self.alloc_expression(HirExprKind::Unresolved(symbol), ty, span)
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
        if let ExprKind::Not(value) = &condition.kind {
            let (truthy, falsy) = self.condition_refinements(value);
            return (falsy, truthy);
        }
        let ExprKind::Binary { left, op, right } = &condition.kind else {
            return (BTreeMap::new(), BTreeMap::new());
        };
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let (mut lt, mut lf) = self.condition_refinements(left);
            let (rt, rf) = self.condition_refinements(right);
            if *op == BinaryOp::And {
                lt.extend(rt);
                // Either operand can make the expression false.
                lf.retain(|id, ty| rf.get(id) == Some(ty));
            } else {
                lt.retain(|id, ty| rt.get(id) == Some(ty));
                lf.extend(rf);
            }
            return (lt, lf);
        }
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

    fn numeric_sources(&self, value: &HirExpr<'hir>) -> BTreeSet<usize> {
        match value.kind {
            HirExprKind::Local(id) if self.inferred_numeric.contains(&id) => {
                BTreeSet::from([self.locals[id.0 as usize].span.start])
            }
            HirExprKind::UnaryMinus(value) => self.numeric_sources(value),
            HirExprKind::Binary {
                left,
                right,
                op: BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide,
            } => {
                let mut sources = self.numeric_sources(left);
                sources.extend(self.numeric_sources(right));
                sources
            }
            _ => BTreeSet::new(),
        }
    }

    fn cast_source_type(&self, value: &HirExpr<'_>) -> ScriptType {
        if self.expression_type(value) == &ScriptType::Any {
            if let HirExprKind::Local(id) = value.kind
                && let Some(ty) = self.immutable_any_types.get(&id)
            {
                return ty.clone();
            }
            if let HirExprKind::Cast {
                value,
                target: ScriptType::Any,
                ..
            } = value.kind
            {
                return self.cast_source_type(value);
            }
        }
        self.expression_type(value).clone()
    }

    fn check_assignment(&mut self, expected: &ScriptType, actual: &ScriptType, span: Span) {
        if !expected.accepts(actual) {
            let help = if expected == &ScriptType::Int && actual == &ScriptType::Float {
                "; use `.toInt()` for an explicit numeric conversion"
            } else if expected == &ScriptType::Float && actual == &ScriptType::Int {
                "; use `.toFloat()` for an explicit numeric conversion"
            } else if actual == &ScriptType::Any {
                "; narrow Any with `as?` or validate it with `as!`"
            } else {
                ""
            };
            self.error(format!("expected {expected:?}, got {actual:?}{help}"), span);
        }
    }

    fn resolve_call(&mut self, expression: &Expr) -> ResolvedFunction {
        let ExprKind::Call { callee, .. } = &expression.kind else {
            return ResolvedFunction::Dynamic;
        };
        if matches!(callee.kind, ExprKind::Member { .. })
            && let Some(symbol) = self.external_selector(callee)
        {
            return ResolvedFunction::External(symbol);
        }
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
        if let ResolvedFunction::External(_) = function
            && let Some((type_parameters, parameters, result)) = self.generic_signature(function)
            && !type_parameters.is_empty()
        {
            let bindings = if explicit_types.is_empty() {
                infer_type_arguments(&parameters, arguments, |value| {
                    self.expression_type(value).clone()
                })
            } else {
                type_parameters
                    .into_iter()
                    .zip(explicit_types.iter().cloned())
                    .collect()
            };
            return substitute_type(&result, &bindings);
        }
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
            ResolvedFunction::External(symbol) => self
                .external_functions
                .get(&symbol)
                .map_or(ScriptType::Any, |signature| signature.result.clone()),
            ResolvedFunction::Dynamic => ScriptType::Any,
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

    fn generic_signature(
        &self,
        function: ResolvedFunction,
    ) -> Option<(Vec<SymbolId>, Vec<ScriptType>, ScriptType)> {
        match function {
            ResolvedFunction::User(id) => self.functions.get(id.0 as usize).map(|f| {
                (
                    f.type_parameters.clone(),
                    f.parameters.clone(),
                    f.result.clone(),
                )
            }),
            ResolvedFunction::External(symbol) => self.external_functions.get(&symbol).map(|f| {
                (
                    self.external_type_parameters
                        .get(&symbol)
                        .cloned()
                        .unwrap_or_default(),
                    f.parameters.clone(),
                    f.result.clone(),
                )
            }),
            _ => None,
        }
    }

    fn external_selector(&self, expression: &Expr) -> Option<SymbolId> {
        let name = flatten_selector(expression)?;
        let root = self.symbols.get(name.split('.').next()?)?;
        if self.resolve_local(root).is_some() || self.global_names.contains_key(&root) {
            return None;
        }
        let symbol = self.symbols.get(&name)?;
        self.external_functions
            .contains_key(&symbol)
            .then_some(symbol)
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
            TypeExprKind::Unit => Some(ScriptType::Unit),
            TypeExprKind::Tuple(values) => Some(ScriptType::TupleOf(
                values
                    .iter()
                    .map(|ty| self.type_from_ast(ty))
                    .collect::<Option<_>>()?,
            )),
            TypeExprKind::Function { parameters, result } => Some(ScriptType::Callable {
                parameters: parameters
                    .iter()
                    .map(|ty| self.type_from_ast(ty))
                    .collect::<Option<_>>()?,
                result: Box::new(self.type_from_ast(result)?),
            }),
            TypeExprKind::Named(name) => {
                if let Some(parameter) = self
                    .type_parameters
                    .iter()
                    .rev()
                    .find_map(|parameters| parameters.get(name))
                {
                    return Some(parameter.clone());
                }
                self.require_named_type(name, &[], ty.span)
            }
            TypeExprKind::Applied { name, arguments } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.type_from_ast(argument))
                    .collect::<Option<Vec<_>>>()?;
                self.require_named_type(name, &arguments, ty.span)
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

    fn require_named_type(
        &mut self,
        name: &str,
        arguments: &[ScriptType],
        span: Span,
    ) -> Option<ScriptType> {
        let previous_errors = self.errors.len();
        let resolved = self.instantiate_named_type(name, arguments, span);
        if resolved.is_none() && self.errors.len() == previous_errors {
            self.error(
                format!("unknown type `{name}`; declare or import the type before using it"),
                span,
            );
        }
        resolved
    }

    fn instantiate_named_type(
        &mut self,
        name: &str,
        arguments: &[ScriptType],
        span: Span,
    ) -> Option<ScriptType> {
        if let Some((parameters, variants)) = self.enums.get(name).cloned() {
            if parameters.len() != arguments.len() {
                self.error(
                    format!(
                        "enum `{name}` expects {} type arguments; provide an explicit type context",
                        parameters.len()
                    ),
                    span,
                );
                return None;
            }
            if self.type_expansions.iter().any(|v| v == name) {
                self.error("recursive enum payloads are not supported yet", span);
                return None;
            }
            self.type_expansions.push(name.to_string());
            self.type_parameters.push(
                parameters
                    .into_iter()
                    .zip(arguments.iter().cloned())
                    .collect(),
            );
            let mut payloads = BTreeMap::new();
            for variant in variants {
                let fields = variant
                    .fields
                    .iter()
                    .filter_map(|field| self.type_from_ast(field))
                    .collect();
                payloads.insert(variant.name, fields);
            }
            self.type_parameters.pop();
            self.type_expansions.pop();
            // Preserve the native Option ABI while the optional bytecode
            // operations are migrated to ordinary enum operations. The schema
            // itself is owned by std, just like every other enum declaration.
            if name == "Optional" {
                return Some(ScriptType::Optional(Box::new(arguments[0].clone())));
            }
            return Some(ScriptType::Enum {
                name: self.symbol(name),
                arguments: arguments.to_vec(),
                variants: payloads,
            });
        }
        let builtin = match name {
            "Any" => Some(ScriptType::Any),
            "Never" => Some(ScriptType::Never),
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
        if matches!(name, "List" | "Binding") {
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
        let resolved = self
            .manifest
            .and_then(|manifest| manifest.symbols().find(name))
            .map(ScriptType::Named);
        resolved
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
        let expression = self.arena.alloc(HirExpr { kind, ty, span });
        if self.types.get(ty) == Some(&ScriptType::Never)
            && matches!(kind, HirExprKind::Call { .. })
        {
            self.arena.alloc(HirExpr {
                kind: HirExprKind::GuardNever(expression),
                ty,
                span,
            })
        } else {
            expression
        }
    }

    fn function_type(&self, function: HirFunctionId) -> ScriptType {
        let declaration = &self.functions[function.0 as usize];
        ScriptType::Callable {
            parameters: declaration.parameters.clone(),
            result: Box::new(declaration.result.clone()),
        }
    }

    fn closure_result(&self, body: &HirBlock<'hir>) -> ScriptType {
        let mut results = Vec::new();
        self.collect_returns(body, &mut results);
        if let Some(result) = results.into_iter().find(|ty| *ty != ScriptType::Never) {
            return result;
        }
        if self.block_diverges(body) {
            return ScriptType::Never;
        }
        body.statements
            .last()
            .and_then(|statement| match statement.kind {
                HirStmtKind::Expr(value) => Some(self.expression_type(value).clone()),
                _ => None,
            })
            .unwrap_or(ScriptType::Unit)
    }

    // Returns are local to this callable: never descend into expression closures.
    // The boolean records whether control can reach the end of this block.
    fn collect_returns(&self, body: &HirBlock<'hir>, results: &mut Vec<ScriptType>) -> bool {
        for statement in body.statements {
            match statement.kind {
                HirStmtKind::Return(value) => {
                    results.push(
                        value
                            .map(|v| self.expression_type(v).clone())
                            .unwrap_or(ScriptType::Unit),
                    );
                    return false;
                }
                HirStmtKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    let then_falls = self.collect_returns(then_block, results);
                    let else_falls =
                        else_block.map_or(true, |block| self.collect_returns(block, results));
                    if !then_falls && !else_falls {
                        return false;
                    }
                }
                HirStmtKind::While { condition, body } => {
                    self.collect_returns(body, results);
                    if matches!(condition.kind, HirExprKind::Literal(HirLiteral::Bool(true))) {
                        return false;
                    }
                }
                HirStmtKind::Expr(value) if self.expression_type(value) == &ScriptType::Never => {
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    fn checked_callable_result(
        &mut self,
        body: &HirBlock<'hir>,
        expected: Option<&ScriptType>,
    ) -> ScriptType {
        let mut results = Vec::new();
        if self.collect_returns(body, &mut results) {
            let tail = body
                .statements
                .last()
                .and_then(|s| match s.kind {
                    HirStmtKind::Expr(value) => Some(self.expression_type(value).clone()),
                    _ => None,
                })
                .unwrap_or(ScriptType::Unit);
            results.push(if expected == Some(&ScriptType::Unit) {
                ScriptType::Unit
            } else {
                tail
            });
        }
        let result = expected.cloned().unwrap_or_else(|| {
            results
                .iter()
                .find(|ty| **ty != ScriptType::Never)
                .cloned()
                .unwrap_or(ScriptType::Never)
        });
        for actual in results {
            self.check_assignment(&result, &actual, body.span);
        }
        result
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
        BinaryOp::Add if left == &ScriptType::String && right == &ScriptType::String => {
            ScriptType::String
        }
        BinaryOp::And
        | BinaryOp::Or
        | BinaryOp::Equal
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
        (Never, other) | (other, Never) => other,
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

// Reference-backed containers can be mutated through aliases, even under let.
// Do not preserve their element/field types as flow evidence without alias analysis.
fn stable_cast_evidence(ty: &ScriptType) -> bool {
    matches!(
        ty,
        ScriptType::Unit
            | ScriptType::Bool
            | ScriptType::Int
            | ScriptType::Float
            | ScriptType::Percent
            | ScriptType::String
            | ScriptType::TextTemplate
            | ScriptType::Symbol
            | ScriptType::Selector
            | ScriptType::Callable { .. }
    )
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
        // Both directions are explicitly requested by the author here.
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
        (
            ScriptType::Callable { parameters, result },
            ScriptType::Callable {
                parameters: actuals,
                result: actual_result,
            },
        ) => {
            for (parameter, actual) in parameters.iter().zip(actuals) {
                infer_type_argument(parameter, actual, substitutions);
            }
            infer_type_argument(result, actual_result, substitutions);
        }
        (ScriptType::TupleOf(parameters), ScriptType::TupleOf(actuals)) => {
            for (parameter, actual) in parameters.iter().zip(actuals) {
                infer_type_argument(parameter, actual, substitutions);
            }
        }
        (
            ScriptType::Enum {
                name, arguments, ..
            },
            ScriptType::Enum {
                name: actual_name,
                arguments: actuals,
                ..
            },
        ) if name == actual_name => {
            for (p, a) in arguments.iter().zip(actuals) {
                infer_type_argument(p, a, substitutions);
            }
        }
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

pub(crate) fn substitute_type(
    ty: &ScriptType,
    substitutions: &BTreeMap<SymbolId, ScriptType>,
) -> ScriptType {
    match ty {
        ScriptType::Callable { parameters, result } => ScriptType::Callable {
            parameters: parameters
                .iter()
                .map(|ty| substitute_type(ty, substitutions))
                .collect(),
            result: Box::new(substitute_type(result, substitutions)),
        },
        ScriptType::TupleOf(values) => ScriptType::TupleOf(
            values
                .iter()
                .map(|ty| substitute_type(ty, substitutions))
                .collect(),
        ),
        ScriptType::TypeParameter(parameter) => substitutions
            .get(parameter)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
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
        ScriptType::Enum {
            name,
            arguments,
            variants,
        } => ScriptType::Enum {
            name: *name,
            arguments: arguments
                .iter()
                .map(|ty| substitute_type(ty, substitutions))
                .collect(),
            variants: variants
                .iter()
                .map(|(n, fields)| {
                    (
                        n.clone(),
                        fields
                            .iter()
                            .map(|ty| substitute_type(ty, substitutions))
                            .collect(),
                    )
                })
                .collect(),
        },
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
        .iter()
        .map(statement_span)
        .map(|span| span.end)
        .max()
        .unwrap_or_default()
}

fn statement_span(statement: &Stmt) -> Span {
    match statement {
        Stmt::Return { span, .. } => *span,
        Stmt::Import { span, .. }
        | Stmt::TypeAlias { span, .. }
        | Stmt::Struct { span, .. }
        | Stmt::Enum { span, .. }
        | Stmt::Impl { span, .. }
        | Stmt::Property { span, .. }
        | Stmt::Const { span, .. }
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
    fn unknown_names_and_contextless_members_have_specific_diagnostics() {
        for (source, expected) in [
            ("let value = missing", "unknown identifier `missing`"),
            ("missing()", "unknown identifier `missing`"),
            ("let value: Missing = 1", "unknown type `Missing`"),
            ("fn greet(value: Greeting) {}", "unknown type `Greeting`"),
            ("let value = .abc", "cannot infer the type of `.abc`"),
            (
                "let value = .abc\nfn use(value: Choice) {}\nenum Choice { abc }\nuse(value)",
                "cannot infer the type of `.abc`",
            ),
            (
                r#"let value = "${missing}""#,
                "unknown identifier `missing`",
            ),
        ] {
            let syntax = parse_program(source).expect("syntax");
            let arena = HirArena::new();
            let errors = lower_to_hir(&arena, &syntax, None).expect_err("must fail statically");
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "{source}: {errors:?}"
            );
        }
        let syntax = parse_program(
            r#"
            enum Choice { abc, other }
            fn accept(value: Choice) {}
            let explicit: Choice = .abc
            accept(explicit)
            accept(.abc)
            let raw: TextTemplate = "${missing}"
        "#,
        )
        .expect("syntax");
        let arena = HirArena::new();
        lower_to_hir(&arena, &syntax, None)
            .expect("explicit and parameter contexts determine variants");
    }

    #[test]
    fn arithmetic_rejects_non_numeric_operands_except_string_addition() {
        for source in [
            r#"let value = "a" + 1"#,
            r#"let value = 1 + "a""#,
            r#"let value = "a" - "b""#,
            r#"let value = "a" * "b""#,
            r#"let value = "a" / "b""#,
            r#"let value = true + false"#,
            r#"let a: Any = "a"; let value = a + "b""#,
            r#"let a: String? = "a"; let value = a + "b""#,
        ] {
            let syntax = parse_program(source).expect("syntax parses");
            let arena = HirArena::new();
            let errors =
                lower_to_hir(&arena, &syntax, None).expect_err("invalid arithmetic must fail");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("arithmetic operator")),
                "{source}: {errors:?}"
            );
        }
    }

    #[test]
    fn standard_enums_require_payload_types_and_exhaustive_matches() {
        for source in [
            "let value: Result = .success(1)",
            "let value: Result<Int, String> = .error(1)",
            "let value: Optional<Int> = .some(\"alice\")",
            "let value: Optional<Int> = .missing",
            "let value: Int? = null\nwhen value { .some(n) -> n }",
            "let value: Int? = null\nvalue ?: \"alice\"",
            "enum Optional { custom }\nlet value: Optional = .custom",
        ] {
            let syntax = parse_program(source).expect("syntax parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_err(), "{source}");
        }
    }

    #[test]
    fn enum_construction_and_when_are_strictly_typed() {
        for (tail, message) in [
            ("let p: Packet<Int> = .data(\"wrong\")", "expected"),
            ("let p: Packet<Int> = .data()", "payload"),
            ("let p: Packet = .empty", "type arguments"),
            ("let p: Packet<Int> = .missing", "variant"),
            (
                "let p: Packet<Int> = .empty\nwhen p { .empty -> 1 }",
                "non-exhaustive",
            ),
            (
                "let p: Packet<Int> = .empty\nwhen p { .empty -> 1\n.empty -> 2\n.data(x) -> x }",
                "duplicate",
            ),
            (
                "let p: Packet<Int> = .empty\nwhen p { .empty -> 1\n.data(x) -> \"bad\" }",
                "same type",
            ),
            (
                "let p: Packet<Int> = .empty\nwhen p { .empty -> 1\n.data(x,y) -> x }",
                "bindings",
            ),
        ] {
            let syntax = parse_program(&format!("enum Packet<T> {{ data(T), empty }}\n{tail}"))
                .expect("parse");
            let arena = HirArena::new();
            let errors = lower_to_hir(&arena, &syntax, None).expect_err("must reject");
            assert!(
                errors.iter().any(|e| e.message.contains(message)),
                "{tail}: {errors:?}"
            );
        }
    }

    #[test]
    fn nominal_struct_declarations_support_generics_and_methods() {
        let source = r#"
            struct Player<T> { name: T, score: Int }
            struct Counter { score: Int }
            impl Counter { fn increment(self) { self.score += 1 } }
            let player: Player<String?> = .{ name: null, score: 12 }
            let counter = Counter.{ score: 1 }
            counter.increment()
            player.name = "Alice"
        "#;
        let syntax = parse_program(source).expect("parse nominal structs");
        assert!(matches!(syntax.statements[0], Stmt::Struct { .. }));
        let arena = HirArena::new();
        lower_to_hir(&arena, &syntax, None).expect("generic records and receiver lowering");
        for source in [
            "struct Alice { value: Int }\nstruct Bob { value: Int }\nlet a = Alice.{value:1}\nlet b: Bob = a",
            "struct Player<T> { value: T }\nlet p: Player = .{value:1}",
            "struct Player { value: Int }\nlet p = Player.{value:\"wrong\"}",
        ] {
            let syntax = parse_program(source).expect("parse invalid types");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_err(), "{source}");
        }
    }

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
    fn fixed_bindings_allow_field_updates_but_not_rebinding() {
        for source in [
            "let alice = .{ stats: .{ score: 1 } }\nalice.stats.score += 1",
            "global let alice = .{ stats: .{ score: 1 } }\nalice.stats.score = 2",
            "var score = 1\nscore += 1\nscore = 3",
            "global var score: Int\nscore = 1\nscore += 1",
        ] {
            let syntax = parse_program(source).expect("binding syntax parses");
            let arena = HirArena::new();
            lower_to_hir(&arena, &syntax, None).expect("valid binding operation lowers");
        }
        for source in [
            "let score = 1\nscore = 2",
            "let score = 1\nscore += 1",
            "global let score = 1\nscore = 2",
            "global let score = 1\nscore += 1",
            "let alice = .{ score: 1 }\nalice = .{ score: 2 }",
        ] {
            let syntax = parse_program(source).expect("binding syntax parses");
            let arena = HirArena::new();
            let errors = lower_to_hir(&arena, &syntax, None)
                .expect_err("fixed binding cannot be reassigned");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("cannot reassign immutable binding"))
            );
        }
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
    fn float_constraints_propagate_back_through_inferred_bindings() {
        let syntax =
            parse_program("let a = 1\nlet alias = a\nlet b: Float = alias").expect("parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("inference succeeds");
        for local in hir.locals.iter() {
            assert_eq!(hir.types.get(local.ty), Some(&ScriptType::Float));
        }
    }

    #[test]
    fn explicit_int_annotation_is_not_reinferred() {
        let syntax = parse_program("let a: Int = 1\nlet b: Float = a").expect("parses");
        let arena = HirArena::new();
        let errors =
            lower_to_hir(&arena, &syntax, None).expect_err("implicit widening is forbidden");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains(".toFloat()"))
        );
    }

    #[test]
    fn float_context_infers_literals_but_not_typed_int_values() {
        for source in [
            "let x: Float = 1\nlet y: Float = -2",
            "fn accept(value: Float) {}\naccept(1)",
            "fn value() -> Float { -1 }",
            "let n: Int = 1\nlet x: Float = n.toFloat()",
            "let n: Int = 1\nfn accept(value: Float) {}\naccept(n.toFloat())",
        ] {
            let syntax = parse_program(source).expect("parses");
            let arena = HirArena::new();
            lower_to_hir(&arena, &syntax, None)
                .expect("contextual literals or explicit conversion");
        }
        for source in [
            "let n: Int = 1\nfn accept(value: Float) {}\naccept(n)",
            "fn value(n: Int) -> Float { n }",
            "let n: Int = 1\nlet x: Float = n + 2",
            "let n: Int = 1\nlet f: Float = 2\nlet result = n + f",
            "let n: Int = 1\nlet values = [n, 2.5]",
            "fn value() -> Int { 1.5 }",
        ] {
            let syntax = parse_program(source).expect("parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_err(), "{source}");
        }
    }

    #[test]
    fn optional_context_reaches_literals_without_widening_typed_ints() {
        for source in [
            "let x: Float? = 6\nlet y: Float? = -6",
            "fn accept(value: Float?) {}\naccept(6)\naccept(-6)\naccept(null)\naccept(.some(6))",
            "fn value() -> Float? { 6 }",
            "let values: List<Float?> = [1, -2, null]",
            "let pair: (Float?, Float?) = (1, -2)",
            "let values: List<Float>? = [1, 2]",
            "let x: Float? = null\nlet y: Float? = x",
        ] {
            let syntax = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            lower_to_hir(&arena, &syntax, None)
                .unwrap_or_else(|errors| panic!("{source}: {errors:?}"));
        }
        for source in [
            "let n: Int = 6\nlet x: Float? = n",
            "fn accept(value: Float?) {}\nlet n: Int = 6\naccept(n)",
            "fn value(n: Int) -> Float? { n }",
        ] {
            let syntax = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_err(), "{source}");
        }
    }

    #[test]
    fn narrowing_and_any_require_explicit_conversion() {
        for source in [
            "let a = 1.5\nlet b: Int = a",
            "let a: Any = 1\nlet b: Int = a",
        ] {
            let syntax = parse_program(source).expect("parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_err());
        }
        assert!(ScriptType::String.accepts(&ScriptType::Never));
        assert!(ScriptType::Any.accepts(&ScriptType::String));
        assert!(!ScriptType::String.accepts(&ScriptType::Any));
    }

    #[test]
    fn immutable_any_evidence_is_available_only_to_explicit_casts() {
        for source in [
            "let a: Int = 1\nlet b: Any = a\nlet c: Int = b as Int",
            "let a: Any = 1\nlet b: Any = a\nlet c = b as Int",
            "let a = 1 as Any\nlet b = a as Int",
            "fn convert(value: Any) -> Int { value as! Int }",
            "fn convert(value: Any) -> Int? { value as? Int }",
        ] {
            let syntax = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &syntax, None).is_ok(), "{source}");
        }
        for (source, diagnostic) in [
            ("let a: Any = 1\nlet b: Int = a", "expected Int"),
            ("let a: Any = 1\nlet b = a as String", "cannot cast"),
            (
                "fn convert(value: Any) -> Int { value as Int }",
                "cannot prove",
            ),
            (
                "var a: Any = 1\na = \"alice\"\nlet b = a as Int",
                "cannot prove",
            ),
            (
                "let a: Any = { value: Int -> value }\na(1)",
                "cannot call Any",
            ),
            (
                "let a: Any = { value: Int -> value }\nlet b = a as (Int) -> Int\nb(\"alice\")",
                "expected Int",
            ),
        ] {
            let syntax = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            let errors =
                lower_to_hir(&arena, &syntax, None).expect_err("invalid Any use is rejected");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains(diagnostic)),
                "{source}: {errors:?}"
            );
        }
    }

    #[test]
    fn never_functions_cannot_fall_through() {
        for (source, valid) in [
            ("fn stop() -> Never { unreachable() }", true),
            ("fn stop() -> Never { todo() }", true),
            ("fn stop() -> Never { 1 }", false),
        ] {
            let syntax = parse_program(source).expect("parses");
            let arena = HirArena::new();
            assert_eq!(
                lower_to_hir(&arena, &syntax, None).is_ok(),
                valid,
                "{source}"
            );
        }
    }

    #[test]
    fn methods_and_intrinsics_have_checked_signatures() {
        for (source, message) in [
            (
                "type Player = .{}\nimpl Player { fn invalid(n: Int, self) {} }",
                "self",
            ),
            ("impl Missing { fn invalid(self) {} }", "unknown type"),
            (
                "type Player = .{}\nimpl Player { fn run(self, n: Int) {} }\nlet p = Player.{}\np.run(\"alice\")",
                "argument expects",
            ),
            ("__builtin_f2i(\"alice\")", "expected Float"),
            ("__builtin_unknown(1)", "unknown compiler intrinsic"),
            ("fn __builtin_f2i() {}", "reserved"),
        ] {
            let syntax = parse_program(source).expect("parses");
            let arena = HirArena::new();
            let errors =
                lower_to_hir(&arena, &syntax, None).expect_err("invalid declaration or call");
            assert!(
                errors.iter().any(|error| error.message.contains(message)),
                "{source}: {errors:?}"
            );
        }
    }

    #[test]
    fn static_and_instance_method_calls_are_distinguished() {
        for (tail, expected) in [
            ("Player.value()", "requires a receiver"),
            (
                "let player = Player.{}\nplayer.name()",
                "static methods must be called on their type",
            ),
            ("Player.name(1)", "arguments"),
            ("Player.add(\"alice\")", "argument expects"),
            ("Player.missing()", "no static method"),
            (
                "let Player = Player.{}\nPlayer.name()",
                "static methods must be called on their type",
            ),
        ] {
            let source = format!(
                "type Player = .{{}}\nimpl Player {{ fn name() -> String {{ \"Player\" }} fn value(self) -> Int {{ 1 }} fn add(n: Int) -> Int {{ n }} }}\n{tail}"
            );
            let syntax = parse_program(&source).expect("parses");
            let arena = HirArena::new();
            let errors = lower_to_hir(&arena, &syntax, None).expect_err("invalid method call");
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "{tail}: {errors:?}"
            );
        }
    }

    #[test]
    fn generic_calls_infer_type_parameters_from_result_context() {
        for source in [
            "fn obtain<T>() -> T { todo() }\ntype FormResult = .{ a: Int }\nlet result: FormResult = obtain()",
            "fn obtain<T>() -> List<T> { todo() }\nlet result: List<String> = obtain()",
            "fn obtain<T>() -> T { todo() }\nfn result() -> String { obtain() }",
            "fn identity<T>(value: T) -> T { value }\nlet result: Float = identity(1)",
            "fn identity<T>(value: T) -> T { value }\nlet result = identity<String>(\"alice\")",
        ] {
            let program = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            lower_to_hir(&arena, &program, None)
                .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        }
        for source in [
            "fn obtain<T>() -> T { todo() }\nlet result = obtain()",
            "fn identity<T>(value: T) -> T { value }\nlet result: String = identity(1)",
            "fn identity<T>(value: T) -> T { value }\nlet value: Int = 1\nlet result: Float = identity(value)",
        ] {
            let program = parse_program(source).expect("source parses");
            let arena = HirArena::new();
            assert!(lower_to_hir(&arena, &program, None).is_err(), "{source}");
        }
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
        let syntax = parse_program("type Loop<T> = Loop<T>\nglobal var value: Loop<String>")
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
