//! A small deterministic bytecode VM for HKS.
//!
//! The VM never accesses an ECS world. Builtin calls are yielded as data and
//! resumed by the embedding engine with a value.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ast::{BinaryOp, Block, Expr, ExprKind, NumberUnit, Program, Stmt},
    hir::{StatementValue, lower_statement},
    span::Span,
    symbol::{SymbolId, SymbolManifest},
};

pub const BYTECODE_VERSION: u16 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BuiltinId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptType {
    Any,
    Unit,
    Bool,
    Int,
    Number,
    Percent,
    String,
    Symbol,
    Selector,
    Task,
    Named(SymbolId),
    Union(Vec<ScriptType>),
    Nullable(Box<ScriptType>),
    Tuple,
    List(Box<ScriptType>),
    Record(BTreeMap<String, ScriptType>),
    Map,
}

impl ScriptType {
    fn accepts(&self, actual: &Self) -> bool {
        self == &Self::Any
            || actual == &Self::Any
            || self == actual
            || matches!(self, Self::Union(types) if types.iter().any(|expected| expected.accepts(actual)))
            || matches!(self, Self::Nullable(inner) if inner.accepts(actual))
            || matches!((self, actual), (Self::List(expected), Self::List(actual)) if expected.accepts(actual))
            || matches!((self, actual), (Self::Record(expected), Self::Record(actual))
                if expected.len() == actual.len()
                    && expected.iter().all(|(name, expected)|
                        actual.get(name).is_some_and(|actual| expected.accepts(actual))))
            || matches!((self, actual), (Self::Number, Self::Int))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub receiver: Option<ScriptType>,
    pub parameters: Vec<ScriptType>,
    pub result: ScriptType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StaticMemberKind {
    Method,
    Getter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticMember {
    pub owner: SymbolId,
    pub name: SymbolId,
    pub builtin: BuiltinId,
    pub kind: StaticMemberKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinManifest {
    hash: u64,
    names: BTreeMap<String, BuiltinId>,
    selectors: BTreeMap<(String, String), BuiltinId>,
    operators: BTreeMap<String, BuiltinId>,
    #[serde(default)]
    symbols: SymbolManifest,
    #[serde(default)]
    signatures: BTreeMap<BuiltinId, FunctionSignature>,
    #[serde(default)]
    static_members: Vec<StaticMember>,
    /// Embedding-owned globals. Source code may read and mutate their fixed fields,
    /// but cannot redeclare the root object or change its schema.
    #[serde(default)]
    globals: BTreeMap<String, ScriptType>,
}

impl BuiltinManifest {
    pub fn new(entries: impl IntoIterator<Item = (impl Into<String>, BuiltinId)>) -> Self {
        let names = entries
            .into_iter()
            .map(|(name, id)| (name.into(), id))
            .collect::<BTreeMap<_, _>>();
        Self::with_selectors(names, BTreeMap::new())
    }

    pub fn with_selectors(
        names: BTreeMap<String, BuiltinId>,
        selectors: BTreeMap<(String, String), BuiltinId>,
    ) -> Self {
        Self::with_operators(names, selectors, BTreeMap::new())
    }

    pub fn with_operators(
        names: BTreeMap<String, BuiltinId>,
        selectors: BTreeMap<(String, String), BuiltinId>,
        operators: BTreeMap<String, BuiltinId>,
    ) -> Self {
        let hash = names
            .iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash, (name, id)| {
                name.bytes()
                    .chain(id.0.to_le_bytes())
                    .fold(hash, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
                    })
            });
        let hash = selectors
            .iter()
            .fold(hash, |hash, ((selector, method), id)| {
                selector
                    .bytes()
                    .chain([b'.'])
                    .chain(method.bytes())
                    .chain(id.0.to_le_bytes())
                    .fold(hash, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
                    })
            });
        let hash = operators.iter().fold(hash, |hash, (operator, id)| {
            operator
                .bytes()
                .chain(id.0.to_le_bytes())
                .fold(hash, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
                })
        });
        Self {
            hash,
            names,
            selectors,
            operators,
            symbols: SymbolManifest::default(),
            signatures: BTreeMap::new(),
            static_members: Vec::new(),
            globals: BTreeMap::new(),
        }
    }

    pub fn with_type_metadata(
        mut self,
        symbols: SymbolManifest,
        signatures: BTreeMap<BuiltinId, FunctionSignature>,
        static_members: Vec<StaticMember>,
    ) -> Self {
        self.symbols = symbols;
        self.signatures = signatures;
        self.static_members = static_members;
        self.rehash();
        self
    }

    pub fn with_globals(mut self, globals: BTreeMap<String, ScriptType>) -> Self {
        self.globals = globals;
        self.rehash();
        self
    }

    pub fn globals(&self) -> &BTreeMap<String, ScriptType> {
        &self.globals
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    pub fn resolve(&self, name: &str) -> Option<BuiltinId> {
        self.names.get(name).copied()
    }

    pub fn resolve_selector(&self, selector: &str, method: &str) -> Option<BuiltinId> {
        self.selectors
            .get(&(selector.to_string(), method.to_string()))
            .copied()
    }

    pub fn resolve_operator(&self, operator: &str) -> Option<BuiltinId> {
        self.operators.get(operator).copied()
    }

    pub fn symbols(&self) -> &SymbolManifest {
        &self.symbols
    }

    pub fn signature(&self, builtin: BuiltinId) -> Option<&FunctionSignature> {
        self.signatures.get(&builtin)
    }

    pub fn resolve_static_method(&self, name: &str) -> Result<&StaticMember, &'static str> {
        self.resolve_static(name, StaticMemberKind::Method)
    }

    pub fn resolve_getter(&self, name: &str) -> Result<&StaticMember, &'static str> {
        self.resolve_static(name, StaticMemberKind::Getter)
    }

    fn resolve_static(
        &self,
        name: &str,
        kind: StaticMemberKind,
    ) -> Result<&StaticMember, &'static str> {
        let Some(name) = self.symbols.find(name) else {
            return Err("unknown");
        };
        let mut matches = self
            .static_members
            .iter()
            .filter(|member| member.name == name && member.kind == kind);
        let Some(member) = matches.next() else {
            return Err("unknown");
        };
        if matches.next().is_some() {
            return Err("ambiguous");
        }
        Ok(member)
    }

    fn rehash(&mut self) {
        let mut hash = Self::with_operators(
            self.names.clone(),
            self.selectors.clone(),
            self.operators.clone(),
        )
        .hash;
        for symbol in self.symbols.symbols() {
            hash = symbol.bytes().fold(hash, hash_byte);
            hash = hash_byte(hash, 0xff);
        }
        for (builtin, signature) in &self.signatures {
            hash = builtin.0.to_le_bytes().into_iter().fold(hash, hash_byte);
            hash = format!("{signature:?}").bytes().fold(hash, hash_byte);
        }
        for member in &self.static_members {
            hash = format!("{member:?}").bytes().fold(hash, hash_byte);
        }
        for (name, ty) in &self.globals {
            hash = name.bytes().fold(hash, hash_byte);
            hash = hash_byte(hash, 0xfe);
            hash = format!("{ty:?}").bytes().fold(hash, hash_byte);
        }
        self.hash = hash;
    }
}

fn hash_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Uninitialized,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(String),
    Selector(String),
    Typed {
        type_id: SymbolId,
        value: Box<Value>,
    },
    Handle {
        type_id: u32,
        id: u64,
    },
    Task(u64),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

enum MemberPathError {
    Unknown(String),
    NotRecord,
}

fn set_member_path(
    value: &mut Value,
    path: &[String],
    new_value: Value,
) -> Result<(), MemberPathError> {
    let Some((name, remainder)) = path.split_first() else {
        return Err(MemberPathError::NotRecord);
    };
    let fields = match value {
        Value::Map(fields) => fields,
        Value::Typed { value, .. } => match value.as_mut() {
            Value::Map(fields) => fields,
            _ => return Err(MemberPathError::NotRecord),
        },
        _ => return Err(MemberPathError::NotRecord),
    };
    let field = fields
        .get_mut(name)
        .ok_or_else(|| MemberPathError::Unknown(name.clone()))?;
    if remainder.is_empty() {
        *field = new_value;
        Ok(())
    } else {
        set_member_path(field, remainder, new_value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub version: u16,
    pub source_hash: u64,
    #[serde(default)]
    pub builtin_manifest_hash: u64,
    pub instructions: Vec<Instruction>,
    #[serde(default)]
    pub functions: Vec<FunctionTemplate>,
    pub tasks: Vec<TaskTemplate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionTemplate {
    pub name: String,
    pub parameters: Vec<String>,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskTemplate {
    pub mode: TaskMode,
    pub instructions: Vec<Instruction>,
    /// Parallel templates reference one sequence template per direct block statement.
    pub children: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskMode {
    Sequence,
    Parallel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    Constant(Value),
    LoadLocal(String),
    StoreLocal(String),
    StoreLocalMember {
        root: String,
        path: Vec<String>,
    },
    LoadGlobal(String),
    StoreGlobal(String),
    StoreGlobalMember {
        root: String,
        path: Vec<String>,
    },
    MakeTuple(usize),
    MakeList(usize),
    MakeMap(Vec<String>),
    Negate,
    Equal,
    GetMember {
        name: String,
        safe: bool,
    },
    AssertNonNull,
    CallBuiltin {
        builtin: BuiltinId,
        labels: Vec<Option<String>>,
        has_receiver: bool,
    },
    CallFunction {
        function: FunctionId,
        argument_count: usize,
    },
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfNotNull(usize),
    Return,
    Statement(StatementValue),
    SpawnTask {
        task: u32,
    },
    Pop,
    Halt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub span: Span,
}

pub fn compile(program: &Program, source_hash: u64) -> Result<Bytecode, Vec<CompileError>> {
    compile_inner(program, source_hash, None)
}

pub fn compile_with_manifest(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
) -> Result<Bytecode, Vec<CompileError>> {
    compile_inner(program, source_hash, Some(manifest))
}

fn compile_inner(
    program: &Program,
    source_hash: u64,
    manifest: Option<&BuiltinManifest>,
) -> Result<Bytecode, Vec<CompileError>> {
    let mut function_names = BTreeMap::new();
    let mut function_signatures = BTreeMap::new();
    let mut type_aliases = BTreeMap::new();
    let mut global_types = manifest
        .map(|manifest| manifest.globals().clone())
        .unwrap_or_default();
    let mut declaration_errors = Vec::new();
    for statement in &program.statements {
        if let Stmt::TypeAlias { name, ty, span } = statement {
            if type_aliases.contains_key(name) {
                declaration_errors.push(CompileError {
                    message: format!("type `{name}` is defined more than once"),
                    span: span.clone(),
                });
                continue;
            }
            match script_type_from_ast(ty, &type_aliases, manifest) {
                Some(ty) => {
                    type_aliases.insert(name.clone(), ty);
                }
                None => declaration_errors.push(CompileError {
                    message: format!("type `{name}` refers to an unknown type"),
                    span: ty.span,
                }),
            }
        }
    }
    for statement in &program.statements {
        if let Stmt::Function {
            name,
            parameters,
            return_type,
            body,
            span,
        } = statement
        {
            if function_names.contains_key(name) {
                declaration_errors.push(CompileError {
                    message: format!("function `{name}` is defined more than once"),
                    span: span.clone(),
                });
            } else {
                function_names.insert(name.clone(), FunctionId(function_names.len() as u32));
                let parameter_types = parameters
                    .iter()
                    .map(|parameter| {
                        parameter
                            .ty
                            .as_ref()
                            .and_then(|ty| script_type_from_ast(ty, &type_aliases, manifest))
                            .or_else(|| parameter.ty.is_none().then_some(ScriptType::Any))
                    })
                    .collect::<Option<Vec<_>>>();
                let result = return_type
                    .as_ref()
                    .and_then(|ty| script_type_from_ast(ty, &type_aliases, manifest));
                if let Some(parameter_types) = parameter_types
                    && (return_type.is_none() || result.is_some())
                {
                    let parameter_locals = parameters
                        .iter()
                        .zip(&parameter_types)
                        .map(|(parameter, ty)| (parameter.name.clone(), ty.clone()))
                        .collect::<BTreeMap<_, _>>();
                    let inferred_result = body
                        .statements
                        .last()
                        .and_then(|statement| match statement {
                            Stmt::Expr(expression) => Some(expression),
                            _ => None,
                        })
                        .map(|expression| {
                            infer_expression_type(
                                manifest,
                                &type_aliases,
                                &parameter_locals,
                                expression,
                            )
                        })
                        .unwrap_or(ScriptType::Unit);
                    let result = result.unwrap_or(inferred_result);
                    function_signatures.insert(
                        name.clone(),
                        FunctionSignature {
                            receiver: None,
                            parameters: parameter_types,
                            result,
                        },
                    );
                } else {
                    declaration_errors.push(CompileError {
                        message: format!("function `{name}` uses an unknown type"),
                        span: span.clone(),
                    });
                }
            }
        }
        if let Stmt::Global {
            name,
            type_annotation,
            value,
            span,
        } = statement
        {
            if manifest.is_some_and(|manifest| manifest.globals().contains_key(name)) {
                declaration_errors.push(CompileError {
                    message: format!("embedding-owned global `{name}` cannot be redeclared"),
                    span: span.clone(),
                });
            } else if global_types.contains_key(name) {
                declaration_errors.push(CompileError {
                    message: format!("global `{name}` is defined more than once"),
                    span: span.clone(),
                });
            } else {
                let ty = type_annotation
                    .as_ref()
                    .and_then(|ty| script_type_from_ast(ty, &type_aliases, manifest))
                    .or_else(|| {
                        value.as_ref().map(|value| {
                            infer_expression_type(manifest, &type_aliases, &BTreeMap::new(), value)
                        })
                    })
                    .unwrap_or(ScriptType::Any);
                global_types.insert(name.clone(), ty);
            }
        }
    }
    let mut compiler = Compiler {
        instructions: Vec::new(),
        functions: Vec::new(),
        tasks: Vec::new(),
        errors: declaration_errors,
        manifest,
        function_names,
        function_signatures,
        local_types: global_types.clone(),
        local_bindings: BTreeSet::new(),
        global_types,
        type_aliases,
    };
    compiler.compile_functions(program);
    for statement in &program.statements {
        if !matches!(statement, Stmt::Function { .. }) {
            compiler.statement(statement);
        }
    }
    compiler.instructions.push(Instruction::Halt);
    if compiler.errors.is_empty() {
        Ok(Bytecode {
            version: BYTECODE_VERSION,
            source_hash,
            builtin_manifest_hash: manifest.map(BuiltinManifest::hash).unwrap_or_default(),
            instructions: compiler.instructions,
            functions: compiler.functions,
            tasks: compiler.tasks,
        })
    } else {
        Err(compiler.errors)
    }
}

struct Compiler<'a> {
    instructions: Vec<Instruction>,
    functions: Vec<FunctionTemplate>,
    tasks: Vec<TaskTemplate>,
    errors: Vec<CompileError>,
    manifest: Option<&'a BuiltinManifest>,
    function_names: BTreeMap<String, FunctionId>,
    function_signatures: BTreeMap<String, FunctionSignature>,
    local_types: BTreeMap<String, ScriptType>,
    local_bindings: BTreeSet<String>,
    global_types: BTreeMap<String, ScriptType>,
    type_aliases: BTreeMap<String, ScriptType>,
}

impl Compiler<'_> {
    fn infer_type(&self, expression: &Expr) -> ScriptType {
        if let ExprKind::Call { callee, .. } = &expression.kind
            && let ExprKind::Ident(name) = &callee.kind
            && let Some(signature) = self.function_signatures.get(name)
        {
            return signature.result.clone();
        }
        infer_expression_type(
            self.manifest,
            &self.type_aliases,
            &self.local_types,
            expression,
        )
    }

    fn expression_matches(&self, expected: &ScriptType, expression: &Expr) -> bool {
        match (&expression.kind, expected) {
            (ExprKind::Null, ScriptType::Nullable(_) | ScriptType::Any) => true,
            (ExprKind::Null, _) => false,
            (
                ExprKind::Map(fields) | ExprKind::TypedMap { fields, .. },
                ScriptType::Record(types),
            ) => {
                fields.len() == types.len()
                    && fields.iter().all(|field| {
                        types
                            .get(&field.name)
                            .is_some_and(|expected| self.expression_matches(expected, &field.value))
                    })
            }
            (ExprKind::List(values), ScriptType::List(element)) => values
                .iter()
                .all(|value| self.expression_matches(element, value)),
            (_, _) => expected.accepts(&self.infer_type(expression)),
        }
    }

    fn reject_untyped_null(&mut self, binding: &str, kind: &str, expression: &Expr) {
        let Some(path) = uncontextual_null_path(expression) else {
            return;
        };
        let target = if path.is_empty() {
            binding.to_string()
        } else {
            format!("{binding}.{}", path.join("."))
        };
        self.errors.push(CompileError {
            message: format!(
                "cannot infer a type for null at `{target}`; add an explicit type (`{kind} {binding}: Type = ...`) or use a typed record constructor (`{kind} {binding} = Type.{{ ... }}`)"
            ),
            span: expression.span,
        });
    }

    fn compile_functions(&mut self, program: &Program) {
        let declarations = program
            .statements
            .iter()
            .filter_map(|statement| match statement {
                Stmt::Function {
                    name,
                    parameters,
                    body,
                    ..
                } => Some((name.clone(), parameters.clone(), body.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (name, parameters, body) in declarations {
            let parent = std::mem::take(&mut self.instructions);
            let parent_types = std::mem::take(&mut self.local_types);
            let parent_bindings = std::mem::take(&mut self.local_bindings);
            self.local_types = self.global_types.clone();
            let signature =
                self.function_signatures
                    .get(&name)
                    .cloned()
                    .unwrap_or(FunctionSignature {
                        receiver: None,
                        parameters: vec![ScriptType::Any; parameters.len()],
                        result: ScriptType::Any,
                    });
            self.local_types.extend(
                parameters
                    .iter()
                    .zip(&signature.parameters)
                    .map(|(parameter, ty)| (parameter.name.clone(), ty.clone())),
            );
            self.local_bindings
                .extend(parameters.iter().map(|parameter| parameter.name.clone()));
            for (index, statement) in body.statements.iter().enumerate() {
                let is_last = index + 1 == body.statements.len();
                if is_last && let Stmt::Expr(expression) = statement {
                    if matches!(expression.kind, ExprKind::Null)
                        && !matches!(signature.result, ScriptType::Nullable(_))
                    {
                        self.errors.push(CompileError {
                            message: format!(
                                "function `{name}` returns null without a nullable return type; declare an explicit `Type?` return type"
                            ),
                            span: expression.span,
                        });
                    }
                    let actual = self.infer_type(expression);
                    if signature.result != ScriptType::Any
                        && !self.expression_matches(&signature.result, expression)
                    {
                        self.errors.push(CompileError {
                            message: format!(
                                "function `{name}` returns {:?}, got {actual:?}",
                                signature.result
                            ),
                            span: expression.span,
                        });
                    }
                    if let ExprKind::String(value) = &expression.kind {
                        self.instructions
                            .push(Instruction::Statement(StatementValue::String(
                                value.clone(),
                            )));
                        self.instructions
                            .push(Instruction::Constant(Value::String(value.clone())));
                    } else {
                        self.expression(expression);
                        self.instructions
                            .push(Instruction::Statement(StatementValue::Commit));
                    }
                    self.instructions.push(Instruction::Return);
                } else {
                    self.statement(statement);
                }
            }
            if !matches!(self.instructions.last(), Some(Instruction::Return)) {
                if signature.result != ScriptType::Any
                    && !signature.result.accepts(&ScriptType::Unit)
                {
                    self.errors.push(CompileError {
                        message: format!(
                            "function `{name}` may return Unit, expected {:?}",
                            signature.result
                        ),
                        span: body.span,
                    });
                }
                self.instructions.push(Instruction::Constant(Value::Null));
                self.instructions.push(Instruction::Return);
            }
            let instructions = std::mem::replace(&mut self.instructions, parent);
            self.local_types = parent_types;
            self.local_bindings = parent_bindings;
            self.functions.push(FunctionTemplate {
                name,
                parameters: parameters
                    .into_iter()
                    .map(|parameter| parameter.name)
                    .collect(),
                instructions,
            });
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        match statement {
            Stmt::TypeAlias { .. } => {}
            Stmt::Function { span, .. } => self.errors.push(CompileError {
                message: "nested function definitions are not supported".to_string(),
                span: span.clone(),
            }),
            Stmt::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                if type_annotation.is_none() {
                    self.reject_untyped_null(name, "let", value);
                }
                let value_type = self.infer_type(value);
                let declared_type = type_annotation
                    .as_ref()
                    .and_then(|ty| script_type_from_ast(ty, &self.type_aliases, self.manifest));
                if type_annotation.is_some() && declared_type.is_none() {
                    self.errors.push(CompileError {
                        message: format!("local `{name}` uses an unknown type"),
                        span: type_annotation.as_ref().expect("checked above").span,
                    });
                }
                if let Some(expected) = &declared_type
                    && !self.expression_matches(expected, value)
                {
                    self.errors.push(CompileError {
                        message: format!("local `{name}` expects {expected:?}, got {value_type:?}"),
                        span: value.span,
                    });
                }
                self.expression(value);
                self.instructions
                    .push(Instruction::StoreLocal(name.clone()));
                self.local_types
                    .insert(name.clone(), declared_type.unwrap_or(value_type));
                self.local_bindings.insert(name.clone());
                self.instructions
                    .push(Instruction::Statement(StatementValue::Commit));
            }
            Stmt::Global {
                name,
                type_annotation,
                value,
                span,
            } => {
                let expected = self
                    .global_types
                    .get(name)
                    .cloned()
                    .unwrap_or(ScriptType::Any);
                if let Some(value) = value {
                    if type_annotation.is_none() {
                        self.reject_untyped_null(name, "global", value);
                    }
                    let actual = self.infer_type(value);
                    if !self.expression_matches(&expected, value) {
                        self.errors.push(CompileError {
                            message: format!(
                                "global `{name}` expects {expected:?}, got {actual:?}"
                            ),
                            span: value.span,
                        });
                    }
                    self.expression(value);
                } else {
                    self.instructions
                        .push(Instruction::Constant(Value::Uninitialized));
                }
                self.instructions
                    .push(Instruction::StoreGlobal(name.clone()));
                self.instructions
                    .push(Instruction::Statement(StatementValue::Commit));
                let _ = span;
            }
            Stmt::Assign { target, value, .. } => {
                let actual = self.infer_type(value);
                if let Some((root, path)) = assignment_member_path(target) {
                    let is_local = self.local_bindings.contains(&root);
                    let root_type = if is_local {
                        self.local_types.get(&root)
                    } else {
                        self.global_types.get(&root)
                    };
                    let Some(root_type) = root_type else {
                        self.errors.push(CompileError {
                            message: format!("unknown assignment root `{root}`"),
                            span: target.span,
                        });
                        return;
                    };
                    let Some(expected) = record_path_type(root_type, &path) else {
                        self.errors.push(CompileError {
                            message: format!("unknown or non-record field `{}`", path.join(".")),
                            span: target.span,
                        });
                        return;
                    };
                    if !self.expression_matches(expected, value) {
                        self.errors.push(CompileError {
                            message: format!(
                                "`{root}.{}` expects {expected:?}, got {actual:?}",
                                path.join(".")
                            ),
                            span: value.span,
                        });
                    }
                    self.expression(value);
                    if is_local {
                        self.instructions
                            .push(Instruction::StoreLocalMember { root, path });
                    } else {
                        self.instructions
                            .push(Instruction::StoreGlobalMember { root, path });
                    }
                    self.instructions
                        .push(Instruction::Statement(StatementValue::Commit));
                    return;
                }
                let ExprKind::Ident(name) = &target.kind else {
                    self.errors.push(CompileError {
                        message: "assignment target must be a variable or global record field"
                            .to_string(),
                        span: target.span,
                    });
                    return;
                };
                let is_local = self.local_bindings.contains(name);
                let expected = if is_local {
                    self.local_types.get(name)
                } else {
                    self.global_types.get(name)
                };
                if let Some(expected) = expected
                    && !self.expression_matches(expected, value)
                {
                    self.errors.push(CompileError {
                        message: format!("`{name}` expects {expected:?}, got {actual:?}"),
                        span: value.span,
                    });
                }
                self.expression(value);
                if is_local {
                    self.instructions
                        .push(Instruction::StoreLocal(name.clone()));
                } else {
                    self.instructions
                        .push(Instruction::StoreGlobal(name.clone()));
                }
                self.instructions
                    .push(Instruction::Statement(StatementValue::Commit));
            }
            Stmt::Expr(expression) => {
                if matches!(expression.kind, ExprKind::Null) {
                    self.errors.push(CompileError {
                        message: "standalone null has no type; use it only where an explicit nullable type is expected"
                            .to_string(),
                        span: expression.span,
                    });
                }
                let statement_value = lower_statement(statement);
                if matches!(statement_value, StatementValue::String(_)) {
                    self.instructions
                        .push(Instruction::Statement(statement_value));
                } else {
                    self.expression(expression);
                    self.instructions.push(Instruction::Pop);
                    self.instructions
                        .push(Instruction::Statement(statement_value));
                }
            }
            Stmt::If {
                condition,
                then_block,
                else_block,
                ..
            } => {
                self.expression(condition);
                let branch = self.instructions.len();
                self.instructions.push(Instruction::JumpIfFalse(usize::MAX));
                for statement in &then_block.statements {
                    self.statement(statement);
                }
                if let Some(else_block) = else_block {
                    let end_jump = self.instructions.len();
                    self.instructions.push(Instruction::Jump(usize::MAX));
                    let else_start = self.instructions.len();
                    self.instructions[branch] = Instruction::JumpIfFalse(else_start);
                    for statement in &else_block.statements {
                        self.statement(statement);
                    }
                    let end = self.instructions.len();
                    self.instructions[end_jump] = Instruction::Jump(end);
                } else {
                    let end = self.instructions.len();
                    self.instructions[branch] = Instruction::JumpIfFalse(end);
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                let start = self.instructions.len();
                self.expression(condition);
                let exit = self.instructions.len();
                self.instructions.push(Instruction::JumpIfFalse(usize::MAX));
                for statement in &body.statements {
                    self.statement(statement);
                }
                self.instructions.push(Instruction::Jump(start));
                let end = self.instructions.len();
                self.instructions[exit] = Instruction::JumpIfFalse(end);
            }
        }
    }

    fn expression(&mut self, expression: &Expr) {
        match &expression.kind {
            ExprKind::Null => self.instructions.push(Instruction::Constant(Value::Null)),
            ExprKind::Ellipsis => self
                .instructions
                .push(Instruction::Constant(Value::Ellipsis)),
            ExprKind::Ident(name) => {
                if self.global_types.contains_key(name) {
                    self.instructions
                        .push(Instruction::LoadGlobal(name.clone()));
                } else {
                    self.instructions.push(Instruction::LoadLocal(name.clone()));
                }
            }
            ExprKind::Symbol(name) => {
                if let Some(manifest) = self.manifest {
                    match manifest.resolve_getter(name) {
                        Ok(member) => {
                            self.instructions.push(Instruction::CallBuiltin {
                                builtin: member.builtin,
                                labels: Vec::new(),
                                has_receiver: false,
                            });
                            return;
                        }
                        Err("ambiguous") => {
                            self.errors.push(CompileError {
                                message: format!("getter `.{name}` is ambiguous"),
                                span: expression.span,
                            });
                            return;
                        }
                        Err(_) => {}
                    }
                }
                self.instructions
                    .push(Instruction::Constant(Value::Symbol(name.clone())));
            }
            ExprKind::Bool(value) => self
                .instructions
                .push(Instruction::Constant(Value::Bool(*value))),
            ExprKind::Number { value, unit } => {
                self.instructions.push(Instruction::Constant(match unit {
                    NumberUnit::Scalar => Value::Number(*value),
                    NumberUnit::Percent => Value::Percent(*value),
                }))
            }
            ExprKind::String(value) => self
                .instructions
                .push(Instruction::Constant(Value::String(value.clone()))),
            ExprKind::UnaryMinus(value) => {
                self.expression(value);
                self.instructions.push(Instruction::Negate);
            }
            ExprKind::Tuple(values) => {
                for value in values {
                    self.expression(value);
                }
                self.instructions.push(Instruction::MakeTuple(values.len()));
            }
            ExprKind::List(values) => {
                for value in values {
                    self.expression(value);
                }
                self.instructions.push(Instruction::MakeList(values.len()));
            }
            ExprKind::Map(fields) => {
                for field in fields {
                    self.expression(&field.value);
                }
                self.instructions.push(Instruction::MakeMap(
                    fields.iter().map(|field| field.name.clone()).collect(),
                ));
            }
            ExprKind::TypedMap { type_name, fields } => {
                match self.type_aliases.get(type_name).cloned() {
                    Some(ScriptType::Record(expected_fields)) => {
                        for field in fields {
                            match expected_fields.get(&field.name) {
                                Some(expected) => {
                                    let actual = self.infer_type(&field.value);
                                    if !self.expression_matches(expected, &field.value) {
                                        self.errors.push(CompileError {
                                            message: format!(
                                                "`{type_name}.{}` expects {expected:?}, got {actual:?}",
                                                field.name
                                            ),
                                            span: field.value.span,
                                        });
                                    }
                                }
                                None => self.errors.push(CompileError {
                                    message: format!(
                                        "type `{type_name}` has no field `{}`",
                                        field.name
                                    ),
                                    span: field.span,
                                }),
                            }
                        }
                        for name in expected_fields.keys() {
                            if !fields.iter().any(|field| &field.name == name) {
                                self.errors.push(CompileError {
                                    message: format!(
                                        "typed record `{type_name}` is missing field `{name}`"
                                    ),
                                    span: expression.span,
                                });
                            }
                        }
                    }
                    Some(_) => self.errors.push(CompileError {
                        message: format!("type `{type_name}` is not a record type"),
                        span: expression.span,
                    }),
                    None => self.errors.push(CompileError {
                        message: format!("unknown record type `{type_name}`"),
                        span: expression.span,
                    }),
                }
                for field in fields {
                    self.expression(&field.value);
                }
                self.instructions.push(Instruction::MakeMap(
                    fields.iter().map(|field| field.name.clone()).collect(),
                ));
            }
            ExprKind::Call {
                callee,
                arguments,
                trailing_block,
            } => {
                if let Some(block) = trailing_block {
                    let Some(callee) = flatten_callee(callee) else {
                        self.errors.push(CompileError {
                            message: "task call target must be an identifier".to_string(),
                            span: callee.span.clone(),
                        });
                        return;
                    };
                    if !arguments.is_empty() || !matches!(callee.as_str(), "seq" | "par") {
                        self.errors.push(CompileError {
                            message: "trailing blocks are only supported by seq and par"
                                .to_string(),
                            span: expression.span.clone(),
                        });
                        return;
                    }
                    let mode = if callee == "seq" {
                        TaskMode::Sequence
                    } else {
                        TaskMode::Parallel
                    };
                    let task = self.compile_task(block, mode);
                    self.instructions.push(Instruction::SpawnTask { task });
                    return;
                }
                if let ExprKind::Ident(name) = &callee.kind
                    && let Some(function) = self.function_names.get(name).copied()
                {
                    let signature = self.function_signatures.get(name).cloned();
                    if let Some(signature) = &signature
                        && signature.parameters.len() != arguments.len()
                    {
                        self.errors.push(CompileError {
                            message: format!(
                                "function `{name}` expects {} arguments, got {}",
                                signature.parameters.len(),
                                arguments.len()
                            ),
                            span: expression.span,
                        });
                    }
                    for (index, argument) in arguments.iter().enumerate() {
                        if argument.label.is_some() {
                            self.errors.push(CompileError {
                                message: "user functions do not accept named arguments".to_string(),
                                span: argument.span.clone(),
                            });
                        }
                        if let Some(expected) = signature
                            .as_ref()
                            .and_then(|signature| signature.parameters.get(index))
                        {
                            let actual = self.infer_type(&argument.value);
                            if !self.expression_matches(expected, &argument.value) {
                                self.errors.push(CompileError {
                                    message: format!(
                                        "function `{name}` argument {} expects {expected:?}, got {actual:?}",
                                        index + 1
                                    ),
                                    span: argument.span,
                                });
                            }
                        }
                        self.expression(&argument.value);
                    }
                    self.instructions.push(Instruction::CallFunction {
                        function,
                        argument_count: arguments.len(),
                    });
                    return;
                }
                if let Some(manifest) = self.manifest {
                    if let ExprKind::Symbol(name) = &callee.kind {
                        match manifest.resolve_static_method(name) {
                            Ok(member) => {
                                self.check_signature(member.builtin, arguments, callee.span);
                                for argument in arguments {
                                    self.expression(&argument.value);
                                }
                                self.instructions.push(Instruction::CallBuiltin {
                                    builtin: member.builtin,
                                    labels: arguments
                                        .iter()
                                        .map(|argument| argument.label.clone())
                                        .collect(),
                                    has_receiver: false,
                                });
                                return;
                            }
                            Err("ambiguous") => {
                                self.errors.push(CompileError {
                                    message: format!("static method `.{name}` is ambiguous"),
                                    span: callee.span,
                                });
                                return;
                            }
                            Err(_) => {}
                        }
                    }
                    if let ExprKind::Ident(name) = &callee.kind
                        && let Some(builtin) = manifest.resolve(&name)
                    {
                        self.check_signature(builtin, arguments, callee.span);
                        for argument in arguments {
                            self.expression(&argument.value);
                        }
                        self.instructions.push(Instruction::CallBuiltin {
                            builtin,
                            labels: arguments
                                .iter()
                                .map(|argument| argument.label.clone())
                                .collect(),
                            has_receiver: false,
                        });
                        return;
                    }
                    if let ExprKind::Member { object, name } = &callee.kind {
                        let selector = match &object.kind {
                            ExprKind::Ident(selector) => manifest
                                .resolve_selector(selector, name)
                                .map(|builtin| (builtin, Some(selector.clone()))),
                            _ => None,
                        };
                        let method = selector
                            .map(|(builtin, selector)| (builtin, selector))
                            .or_else(|| manifest.resolve(name).map(|builtin| (builtin, None)));
                        if let Some((builtin, selector)) = method {
                            self.check_method_signature(builtin, object, arguments, callee.span);
                            if let Some(selector) = selector {
                                self.instructions
                                    .push(Instruction::Constant(Value::Selector(selector)));
                            } else {
                                self.expression(object);
                            }
                            for argument in arguments {
                                self.expression(&argument.value);
                            }
                            self.instructions.push(Instruction::CallBuiltin {
                                builtin,
                                labels: arguments
                                    .iter()
                                    .map(|argument| argument.label.clone())
                                    .collect(),
                                has_receiver: true,
                            });
                            return;
                        }
                    }
                    self.errors.push(CompileError {
                        message: "call is not registered in the builtin manifest".to_string(),
                        span: callee.span.clone(),
                    });
                    return;
                }
                self.errors.push(CompileError {
                    message: "builtin calls require a BuiltinManifest".to_string(),
                    span: callee.span.clone(),
                });
            }
            ExprKind::Member { object, name } => {
                self.expression(object);
                self.instructions.push(Instruction::GetMember {
                    name: name.clone(),
                    safe: false,
                });
            }
            ExprKind::SafeMember { object, name } => {
                self.expression(object);
                self.instructions.push(Instruction::GetMember {
                    name: name.clone(),
                    safe: true,
                });
            }
            ExprKind::NonNull(value) => {
                self.expression(value);
                self.instructions.push(Instruction::AssertNonNull);
            }
            ExprKind::Elvis { value, fallback } => {
                self.expression(value);
                let jump = self.instructions.len();
                self.instructions
                    .push(Instruction::JumpIfNotNull(usize::MAX));
                self.instructions.push(Instruction::Pop);
                self.expression(fallback);
                let end = self.instructions.len();
                self.instructions[jump] = Instruction::JumpIfNotNull(end);
            }
            ExprKind::Block(_) => self.errors.push(CompileError {
                message: "block expression is not a value".to_string(),
                span: expression.span.clone(),
            }),
            ExprKind::Binary { left, op, right } => {
                self.expression(left);
                self.expression(right);
                match op {
                    BinaryOp::Equal => self.instructions.push(Instruction::Equal),
                    BinaryOp::Colon => {
                        let Some(builtin) = self
                            .manifest
                            .and_then(|manifest| manifest.resolve_operator(":"))
                        else {
                            self.errors.push(CompileError {
                                message: "operator `:` is not registered in the builtin manifest"
                                    .to_string(),
                                span: expression.span,
                            });
                            return;
                        };
                        self.instructions.push(Instruction::CallBuiltin {
                            builtin,
                            labels: vec![None, None],
                            has_receiver: false,
                        });
                    }
                }
            }
        }
    }

    fn compile_task(&mut self, block: &Block, mode: TaskMode) -> u32 {
        if mode == TaskMode::Parallel {
            let children = block
                .statements
                .iter()
                .map(|statement| self.compile_statement_task(statement))
                .collect();
            let task = self.tasks.len() as u32;
            self.tasks.push(TaskTemplate {
                mode,
                instructions: Vec::new(),
                children,
            });
            return task;
        }
        let parent = std::mem::take(&mut self.instructions);
        for statement in &block.statements {
            self.statement(statement);
        }
        self.instructions.push(Instruction::Halt);
        let instructions = std::mem::replace(&mut self.instructions, parent);
        let task = self.tasks.len() as u32;
        self.tasks.push(TaskTemplate {
            mode,
            instructions,
            children: Vec::new(),
        });
        task
    }

    fn check_signature(&mut self, builtin: BuiltinId, arguments: &[crate::Argument], span: Span) {
        let Some(signature) = self
            .manifest
            .and_then(|manifest| manifest.signature(builtin))
            .cloned()
        else {
            return;
        };
        if signature.receiver.is_some() {
            self.errors.push(CompileError {
                message: "method requires a receiver".to_string(),
                span,
            });
            return;
        }
        if signature.parameters.len() != arguments.len() {
            self.errors.push(CompileError {
                message: format!(
                    "expected {} arguments, got {}",
                    signature.parameters.len(),
                    arguments.len()
                ),
                span,
            });
            return;
        }
        for (index, (expected, argument)) in signature.parameters.iter().zip(arguments).enumerate()
        {
            let actual = self.infer_type(&argument.value);
            if !self.expression_matches(expected, &argument.value) {
                self.errors.push(CompileError {
                    message: format!(
                        "argument {} expects {expected:?}, got {actual:?}",
                        index + 1
                    ),
                    span: argument.span,
                });
            }
        }
    }

    fn check_method_signature(
        &mut self,
        builtin: BuiltinId,
        receiver: &Expr,
        arguments: &[crate::Argument],
        span: Span,
    ) {
        let Some(signature) = self
            .manifest
            .and_then(|manifest| manifest.signature(builtin))
            .cloned()
        else {
            return;
        };
        if let Some(expected) = &signature.receiver {
            let actual = self.infer_type(receiver);
            if !self.expression_matches(expected, receiver) {
                self.errors.push(CompileError {
                    message: format!("receiver expects {expected:?}, got {actual:?}"),
                    span: receiver.span,
                });
            }
        }
        if signature.parameters.len() != arguments.len() {
            self.errors.push(CompileError {
                message: format!(
                    "expected {} arguments, got {}",
                    signature.parameters.len(),
                    arguments.len()
                ),
                span,
            });
            return;
        }
        for (index, (expected, argument)) in signature.parameters.iter().zip(arguments).enumerate()
        {
            let actual = self.infer_type(&argument.value);
            if !self.expression_matches(expected, &argument.value) {
                self.errors.push(CompileError {
                    message: format!(
                        "argument {} expects {expected:?}, got {actual:?}",
                        index + 1
                    ),
                    span: argument.span,
                });
            }
        }
    }

    fn compile_statement_task(&mut self, statement: &Stmt) -> u32 {
        let parent = std::mem::take(&mut self.instructions);
        self.statement(statement);
        self.instructions.push(Instruction::Halt);
        let instructions = std::mem::replace(&mut self.instructions, parent);
        let task = self.tasks.len() as u32;
        self.tasks.push(TaskTemplate {
            mode: TaskMode::Sequence,
            instructions,
            children: Vec::new(),
        });
        task
    }
}

fn infer_expression_type(
    manifest: Option<&BuiltinManifest>,
    type_aliases: &BTreeMap<String, ScriptType>,
    locals: &BTreeMap<String, ScriptType>,
    expression: &Expr,
) -> ScriptType {
    match &expression.kind {
        ExprKind::Null => ScriptType::Any,
        ExprKind::Ellipsis => ScriptType::Any,
        ExprKind::Bool(_) | ExprKind::Binary { .. } => ScriptType::Bool,
        ExprKind::Number {
            value,
            unit: NumberUnit::Scalar,
        } => {
            if value.fract() == 0.0 {
                ScriptType::Int
            } else {
                ScriptType::Number
            }
        }
        ExprKind::UnaryMinus(value) => infer_expression_type(manifest, type_aliases, locals, value),
        ExprKind::Number {
            unit: NumberUnit::Percent,
            ..
        } => ScriptType::Percent,
        ExprKind::String(_) => ScriptType::String,
        ExprKind::Symbol(name) => manifest
            .and_then(|manifest| manifest.resolve_getter(name).ok())
            .and_then(|member| manifest.and_then(|manifest| manifest.signature(member.builtin)))
            .map(|signature| signature.result.clone())
            .unwrap_or(ScriptType::Symbol),
        ExprKind::Tuple(_) => ScriptType::Tuple,
        ExprKind::List(values) => {
            let mut values = values.iter();
            let element = values
                .next()
                .map(|value| infer_expression_type(manifest, type_aliases, locals, value))
                .unwrap_or(ScriptType::Any);
            if values.all(|value| {
                element.accepts(&infer_expression_type(
                    manifest,
                    type_aliases,
                    locals,
                    value,
                ))
            }) {
                ScriptType::List(Box::new(element))
            } else {
                ScriptType::List(Box::new(ScriptType::Any))
            }
        }
        ExprKind::Map(fields) => ScriptType::Record(
            fields
                .iter()
                .map(|field| {
                    (
                        field.name.clone(),
                        infer_expression_type(manifest, type_aliases, locals, &field.value),
                    )
                })
                .collect(),
        ),
        ExprKind::TypedMap { type_name, .. } => type_aliases
            .get(type_name)
            .cloned()
            .unwrap_or(ScriptType::Any),
        ExprKind::Call { callee, .. } => {
            let builtin = manifest.and_then(|manifest| match &callee.kind {
                ExprKind::Ident(name) => manifest.resolve(name),
                ExprKind::Symbol(name) => manifest
                    .resolve_static_method(name)
                    .ok()
                    .map(|member| member.builtin),
                ExprKind::Member { name, .. } => manifest.resolve(name),
                _ => None,
            });
            builtin
                .and_then(|builtin| manifest.and_then(|manifest| manifest.signature(builtin)))
                .map(|signature| signature.result.clone())
                .unwrap_or(ScriptType::Any)
        }
        ExprKind::Ident(name) => locals.get(name).cloned().unwrap_or(ScriptType::Any),
        ExprKind::Member { object, name } => {
            match infer_expression_type(manifest, type_aliases, locals, object) {
                ScriptType::Record(fields) => fields.get(name).cloned().unwrap_or(ScriptType::Any),
                ScriptType::Nullable(inner) => match *inner {
                    ScriptType::Record(fields) => {
                        fields.get(name).cloned().unwrap_or(ScriptType::Any)
                    }
                    _ => ScriptType::Any,
                },
                _ => ScriptType::Any,
            }
        }
        ExprKind::SafeMember { object, name } => {
            let member = match infer_expression_type(manifest, type_aliases, locals, object) {
                ScriptType::Record(fields) => fields.get(name).cloned().unwrap_or(ScriptType::Any),
                ScriptType::Nullable(inner) => match *inner {
                    ScriptType::Record(fields) => {
                        fields.get(name).cloned().unwrap_or(ScriptType::Any)
                    }
                    _ => ScriptType::Any,
                },
                _ => ScriptType::Any,
            };
            ScriptType::Nullable(Box::new(member))
        }
        ExprKind::Elvis { value, fallback } => {
            let value_type = infer_expression_type(manifest, type_aliases, locals, value);
            let fallback = infer_expression_type(manifest, type_aliases, locals, fallback);
            match (&value.kind, value_type) {
                (ExprKind::Null, _) => fallback,
                (_, ScriptType::Nullable(inner)) if inner.accepts(&fallback) => *inner,
                (_, other) => other,
            }
        }
        ExprKind::NonNull(value) => {
            match infer_expression_type(manifest, type_aliases, locals, value) {
                ScriptType::Nullable(inner) => *inner,
                other => other,
            }
        }
        ExprKind::Block(_) => ScriptType::Any,
    }
}

fn uncontextual_null_path(expression: &Expr) -> Option<Vec<String>> {
    match &expression.kind {
        ExprKind::Null => Some(Vec::new()),
        ExprKind::Map(fields) => fields.iter().find_map(|field| {
            uncontextual_null_path(&field.value).map(|mut path| {
                path.insert(0, field.name.clone());
                path
            })
        }),
        ExprKind::List(values) | ExprKind::Tuple(values) => {
            values.iter().enumerate().find_map(|(index, value)| {
                uncontextual_null_path(value).map(|mut path| {
                    path.insert(0, format!("[{index}]"));
                    path
                })
            })
        }
        // A typed constructor supplies context to every field. Other
        // expressions such as Elvis and equality give `null` their own
        // operator context and therefore do not infer a Null type either.
        ExprKind::TypedMap { .. } => None,
        _ => None,
    }
}

fn script_type_from_ast(
    ty: &crate::TypeExpr,
    aliases: &BTreeMap<String, ScriptType>,
    manifest: Option<&BuiltinManifest>,
) -> Option<ScriptType> {
    match &ty.kind {
        crate::TypeExprKind::Named(name) => match name.as_str() {
            "Any" => Some(ScriptType::Any),
            "Unit" => Some(ScriptType::Unit),
            "Bool" => Some(ScriptType::Bool),
            "Int" => Some(ScriptType::Int),
            "Float" | "Number" => Some(ScriptType::Number),
            "String" => Some(ScriptType::String),
            _ => aliases.get(name).cloned().or_else(|| {
                manifest
                    .and_then(|manifest| manifest.symbols().find(name))
                    .map(ScriptType::Named)
            }),
        },
        crate::TypeExprKind::Nullable(inner) => Some(ScriptType::Nullable(Box::new(
            script_type_from_ast(inner, aliases, manifest)?,
        ))),
        crate::TypeExprKind::List(element) => Some(ScriptType::List(Box::new(
            script_type_from_ast(element, aliases, manifest)?,
        ))),
        crate::TypeExprKind::Record(fields) => Some(ScriptType::Record(
            fields
                .iter()
                .map(|field| {
                    Some((
                        field.name.clone(),
                        script_type_from_ast(&field.ty, aliases, manifest)?,
                    ))
                })
                .collect::<Option<BTreeMap<_, _>>>()?,
        )),
    }
}

fn flatten_callee(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => Some(format!("{}.{}", flatten_callee(object)?, name)),
        _ => None,
    }
}

fn assignment_member_path(expression: &Expr) -> Option<(String, Vec<String>)> {
    fn collect(expression: &Expr, path: &mut Vec<String>) -> Option<String> {
        match &expression.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Member { object, name } => {
                let root = collect(object, path)?;
                path.push(name.clone());
                Some(root)
            }
            _ => None,
        }
    }
    let mut path = Vec::new();
    let root = collect(expression, &mut path)?;
    (!path.is_empty()).then_some((root, path))
}

fn record_path_type<'a>(mut ty: &'a ScriptType, path: &[String]) -> Option<&'a ScriptType> {
    for name in path {
        let ScriptType::Record(fields) = ty else {
            return None;
        };
        ty = fields.get(name)?;
    }
    Some(ty)
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinCall {
    pub builtin: BuiltinId,
    pub receiver: Option<Value>,
    pub arguments: Vec<CallArgument>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallArgument {
    pub label: Option<String>,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRequest {
    pub task: u32,
    pub template: TaskTemplate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VmEvent {
    Call(BuiltinCall),
    Statement(StatementValue),
    SpawnTask(TaskRequest),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmStatus {
    Ready,
    WaitingForHost,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VmSnapshot {
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub pc: usize,
    #[serde(default)]
    pub current_function: Option<FunctionId>,
    pub stack: Vec<Value>,
    pub locals: BTreeMap<String, Value>,
    #[serde(default)]
    pub globals: BTreeMap<String, Value>,
    #[serde(default)]
    pub call_frames: Vec<CallFrame>,
    pub status: VmStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallFrame {
    pub function: Option<FunctionId>,
    pub pc: usize,
    pub locals: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vm {
    bytecode: Bytecode,
    pc: usize,
    current_function: Option<FunctionId>,
    stack: Vec<Value>,
    locals: BTreeMap<String, Value>,
    globals: BTreeMap<String, Value>,
    call_frames: Vec<CallFrame>,
    status: VmStatus,
}

impl Vm {
    pub fn new(bytecode: Bytecode) -> Result<Self, VmError> {
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        Ok(Self {
            bytecode,
            pc: 0,
            current_function: None,
            stack: Vec::new(),
            locals: BTreeMap::new(),
            globals: BTreeMap::new(),
            call_frames: Vec::new(),
            status: VmStatus::Ready,
        })
    }

    /// Seeds top-level locals when an embedding executes separately compiled
    /// statement chunks. Values remain VM-owned and are still checked by native
    /// argument conversion when used as call receivers or arguments.
    pub fn set_locals(&mut self, locals: BTreeMap<String, Value>) {
        self.locals = locals;
    }

    pub fn locals(&self) -> &BTreeMap<String, Value> {
        &self.locals
    }

    pub fn set_globals(&mut self, globals: BTreeMap<String, Value>) {
        self.globals = globals;
    }

    pub fn globals(&self) -> &BTreeMap<String, Value> {
        &self.globals
    }

    pub fn status(&self) -> &VmStatus {
        &self.status
    }

    pub fn eval_template(
        &mut self,
        template: &str,
    ) -> Result<String, crate::template::TemplateError> {
        crate::template::eval_template(template, &mut self.globals)
    }

    pub fn step(&mut self) -> Result<Option<VmEvent>, VmError> {
        if !matches!(self.status, VmStatus::Ready) {
            return Ok(None);
        }
        loop {
            let instruction = self
                .current_instructions()?
                .get(self.pc)
                .cloned()
                .ok_or(VmError::InvalidProgramCounter(self.pc))?;
            self.pc += 1;
            match instruction {
                Instruction::Constant(value) => self.stack.push(value),
                Instruction::LoadLocal(name) => self.stack.push(
                    self.locals
                        .get(&name)
                        .cloned()
                        .ok_or(VmError::UnknownLocal(name))?,
                ),
                Instruction::StoreLocal(name) => {
                    let value = self.pop()?;
                    self.locals.insert(name, value);
                }
                Instruction::StoreLocalMember { root, path } => {
                    let new_value = self.pop()?;
                    let value = self
                        .locals
                        .get_mut(&root)
                        .ok_or_else(|| VmError::UnknownLocal(root.clone()))?;
                    set_member_path(value, &path, new_value).map_err(|error| match error {
                        MemberPathError::Unknown(name) => VmError::UnknownMember(name),
                        MemberPathError::NotRecord => {
                            VmError::TypeMismatch("member receiver is not a record")
                        }
                    })?;
                }
                Instruction::LoadGlobal(name) => {
                    let value = self
                        .globals
                        .get(&name)
                        .cloned()
                        .ok_or_else(|| VmError::UnknownGlobal(name.clone()))?;
                    if value == Value::Uninitialized {
                        return Err(VmError::UninitializedGlobal(name));
                    }
                    self.stack.push(value);
                }
                Instruction::StoreGlobal(name) => {
                    let value = self.pop()?;
                    self.globals.insert(name, value);
                }
                Instruction::StoreGlobalMember { root, path } => {
                    let new_value = self.pop()?;
                    let value = self
                        .globals
                        .get_mut(&root)
                        .ok_or_else(|| VmError::UnknownGlobal(root.clone()))?;
                    set_member_path(value, &path, new_value).map_err(|error| match error {
                        MemberPathError::Unknown(name) => VmError::UnknownMember(name),
                        MemberPathError::NotRecord => {
                            VmError::TypeMismatch("member receiver is not a record")
                        }
                    })?;
                }
                Instruction::MakeTuple(count) => {
                    let values = self.pop_count(count)?;
                    self.stack.push(Value::Tuple(values));
                }
                Instruction::MakeList(count) => {
                    let values = self.pop_count(count)?;
                    self.stack.push(Value::List(values));
                }
                Instruction::MakeMap(keys) => {
                    let values = self.pop_count(keys.len())?;
                    self.stack
                        .push(Value::Map(keys.into_iter().zip(values).collect()));
                }
                Instruction::Negate => {
                    let Value::Number(value) = self.pop()? else {
                        return Err(VmError::TypeMismatch("cannot negate a non-number"));
                    };
                    self.stack.push(Value::Number(-value));
                }
                Instruction::Equal => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Bool(left == right));
                }
                Instruction::GetMember { name, safe } => {
                    let value = self.pop()?;
                    if value == Value::Null && safe {
                        self.stack.push(Value::Null);
                        continue;
                    }
                    let value = match value {
                        Value::Map(mut fields) => fields
                            .remove(&name)
                            .ok_or_else(|| VmError::UnknownMember(name.clone()))?,
                        Value::Typed { value, .. } => match *value {
                            Value::Map(mut fields) => fields
                                .remove(&name)
                                .ok_or_else(|| VmError::UnknownMember(name.clone()))?,
                            _ => {
                                return Err(VmError::TypeMismatch(
                                    "member receiver is not a record",
                                ));
                            }
                        },
                        Value::Null => return Err(VmError::NullMemberAccess(name)),
                        _ => return Err(VmError::TypeMismatch("member receiver is not a record")),
                    };
                    self.stack.push(value);
                }
                Instruction::AssertNonNull => {
                    if self.stack.last() == Some(&Value::Null) {
                        return Err(VmError::NullAssertion);
                    }
                }
                Instruction::CallBuiltin {
                    builtin,
                    labels,
                    has_receiver,
                } => {
                    let mut values = self.pop_count(labels.len() + usize::from(has_receiver))?;
                    let receiver = if has_receiver {
                        Some(values.remove(0))
                    } else {
                        None
                    };
                    self.status = VmStatus::WaitingForHost;
                    return Ok(Some(VmEvent::Call(BuiltinCall {
                        builtin,
                        receiver,
                        arguments: labels
                            .into_iter()
                            .zip(values)
                            .map(|(label, value)| CallArgument { label, value })
                            .collect(),
                    })));
                }
                Instruction::CallFunction {
                    function,
                    argument_count,
                } => {
                    let template = self
                        .bytecode
                        .functions
                        .get(function.0 as usize)
                        .ok_or(VmError::UnknownFunction(function))?;
                    if template.parameters.len() != argument_count {
                        return Err(VmError::FunctionArity {
                            function,
                            expected: template.parameters.len(),
                            actual: argument_count,
                        });
                    }
                    let parameters = template.parameters.clone();
                    let values = self.pop_count(argument_count)?;
                    self.call_frames.push(CallFrame {
                        function: self.current_function,
                        pc: self.pc,
                        locals: std::mem::take(&mut self.locals),
                    });
                    self.current_function = Some(function);
                    self.pc = 0;
                    self.locals = parameters.into_iter().zip(values).collect();
                }
                Instruction::Jump(target) => self.pc = target,
                Instruction::JumpIfFalse(target) => {
                    let Value::Bool(condition) = self.pop()? else {
                        return Err(VmError::TypeMismatch("condition must be bool"));
                    };
                    if !condition {
                        self.pc = target;
                    }
                }
                Instruction::JumpIfNotNull(target) => {
                    if self.stack.last().is_some_and(|value| value != &Value::Null) {
                        self.pc = target;
                    }
                }
                Instruction::Return => {
                    let value = self.pop()?;
                    let frame = self
                        .call_frames
                        .pop()
                        .ok_or(VmError::ReturnOutsideFunction)?;
                    self.current_function = frame.function;
                    self.pc = frame.pc;
                    self.locals = frame.locals;
                    self.stack.push(value);
                }
                Instruction::Statement(value) => return Ok(Some(VmEvent::Statement(value))),
                Instruction::SpawnTask { task } => {
                    let template = self
                        .bytecode
                        .tasks
                        .get(task as usize)
                        .cloned()
                        .ok_or(VmError::UnknownTask(task))?;
                    self.status = VmStatus::WaitingForHost;
                    return Ok(Some(VmEvent::SpawnTask(TaskRequest { task, template })));
                }
                Instruction::Pop => {
                    self.pop()?;
                }
                Instruction::Halt => {
                    self.status = VmStatus::Completed;
                    return Ok(Some(VmEvent::Completed(
                        self.stack.pop().unwrap_or(Value::Null),
                    )));
                }
            }
        }
    }

    /// Supplies the result of the most recent yielded host request.
    pub fn resume(&mut self, value: Value) -> Result<(), VmError> {
        if !matches!(self.status, VmStatus::WaitingForHost) {
            return Err(VmError::NotWaitingForHost);
        }
        self.stack.push(value);
        self.status = VmStatus::Ready;
        Ok(())
    }

    pub fn resume_builtin(&mut self, value: Value) -> Result<(), VmError> {
        self.resume(value)
    }

    pub fn snapshot(&self) -> VmSnapshot {
        VmSnapshot {
            source_hash: self.bytecode.source_hash,
            builtin_manifest_hash: self.bytecode.builtin_manifest_hash,
            pc: self.pc,
            current_function: self.current_function,
            stack: self.stack.clone(),
            locals: self.locals.clone(),
            globals: self.globals.clone(),
            call_frames: self.call_frames.clone(),
            status: self.status.clone(),
        }
    }

    pub fn restore(bytecode: Bytecode, snapshot: VmSnapshot) -> Result<Self, VmError> {
        if bytecode.source_hash != snapshot.source_hash {
            return Err(VmError::SourceHashMismatch);
        }
        if bytecode.builtin_manifest_hash != snapshot.builtin_manifest_hash {
            return Err(VmError::BuiltinManifestMismatch);
        }
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        let instruction_length = snapshot
            .current_function
            .map(|function| {
                bytecode
                    .functions
                    .get(function.0 as usize)
                    .map(|template| template.instructions.len())
                    .ok_or(VmError::UnknownFunction(function))
            })
            .transpose()?
            .unwrap_or(bytecode.instructions.len());
        if snapshot.pc > instruction_length {
            return Err(VmError::InvalidProgramCounter(snapshot.pc));
        }
        Ok(Self {
            bytecode,
            pc: snapshot.pc,
            current_function: snapshot.current_function,
            stack: snapshot.stack,
            locals: snapshot.locals,
            globals: snapshot.globals,
            call_frames: snapshot.call_frames,
            status: snapshot.status,
        })
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn current_instructions(&self) -> Result<&[Instruction], VmError> {
        match self.current_function {
            Some(function) => self
                .bytecode
                .functions
                .get(function.0 as usize)
                .map(|template| template.instructions.as_slice())
                .ok_or(VmError::UnknownFunction(function)),
            None => Ok(&self.bytecode.instructions),
        }
    }

    fn pop_count(&mut self, count: usize) -> Result<Vec<Value>, VmError> {
        if self.stack.len() < count {
            return Err(VmError::StackUnderflow);
        }
        let start = self.stack.len() - count;
        Ok(self.stack.split_off(start))
    }
}

/// Deterministic executor for task templates emitted by `seq` and `par`.
///
/// The scheduler has no knowledge of builtins. It yields calls to its host and
/// accepts a value when that host completes the operation.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskScheduler {
    bytecode: Bytecode,
    next_task_id: u64,
    tasks: BTreeMap<u64, ScheduledTask>,
    globals: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ScheduledTask {
    template: u32,
    pc: usize,
    current_function: Option<FunctionId>,
    stack: Vec<Value>,
    locals: BTreeMap<String, Value>,
    call_frames: Vec<CallFrame>,
    status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Ready,
    WaitingForHost,
    WaitingForChildren(Vec<u64>),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSchedulerSnapshot {
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub next_task_id: u64,
    pub tasks: BTreeMap<u64, TaskSnapshot>,
    #[serde(default)]
    pub globals: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub template: u32,
    pub pc: usize,
    #[serde(default)]
    pub current_function: Option<FunctionId>,
    pub stack: Vec<Value>,
    pub locals: BTreeMap<String, Value>,
    #[serde(default)]
    pub call_frames: Vec<CallFrame>,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskEvent {
    Call { task: u64, call: BuiltinCall },
    Statement { task: u64, value: StatementValue },
    Completed { task: u64, value: Value },
}

impl TaskScheduler {
    pub fn new(bytecode: Bytecode) -> Result<Self, TaskSchedulerError> {
        if bytecode.version != BYTECODE_VERSION {
            return Err(TaskSchedulerError::UnsupportedBytecode(bytecode.version));
        }
        Ok(Self {
            bytecode,
            next_task_id: 1,
            tasks: BTreeMap::new(),
            globals: BTreeMap::new(),
        })
    }

    pub fn set_globals(&mut self, globals: BTreeMap<String, Value>) {
        self.globals = globals;
    }

    pub fn globals(&self) -> &BTreeMap<String, Value> {
        &self.globals
    }

    pub fn eval_template(
        &mut self,
        template: &str,
    ) -> Result<String, crate::template::TemplateError> {
        crate::template::eval_template(template, &mut self.globals)
    }

    /// Starts a task template and returns its deterministic handle.
    pub fn spawn(&mut self, template: u32) -> Result<u64, TaskSchedulerError> {
        self.template(template)?;
        let task = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            task,
            ScheduledTask {
                template,
                pc: 0,
                current_function: None,
                stack: Vec::new(),
                locals: BTreeMap::new(),
                call_frames: Vec::new(),
                status: TaskStatus::Ready,
            },
        );
        Ok(task)
    }

    /// Drives one task until it needs a host result or completes.
    pub fn step(&mut self) -> Result<Option<TaskEvent>, TaskSchedulerError> {
        loop {
            if let Some(event) = self.settle_completed_children()? {
                return Ok(Some(event));
            }
            let Some(task) = self.tasks.iter().find_map(|(task, state)| {
                matches!(state.status, TaskStatus::Ready).then_some(*task)
            }) else {
                return Ok(None);
            };
            if let Some(event) = self.step_task(task)? {
                return Ok(Some(event));
            }
        }
    }

    pub fn resume(&mut self, task: u64, value: Value) -> Result<(), TaskSchedulerError> {
        let task = self
            .tasks
            .get_mut(&task)
            .ok_or(TaskSchedulerError::UnknownTask(task))?;
        if !matches!(task.status, TaskStatus::WaitingForHost) {
            return Err(TaskSchedulerError::NotWaitingForHost);
        }
        task.stack.push(value);
        task.status = TaskStatus::Ready;
        Ok(())
    }

    pub fn status(&self, task: u64) -> Option<&TaskStatus> {
        self.tasks.get(&task).map(|task| &task.status)
    }

    pub fn snapshot(&self) -> TaskSchedulerSnapshot {
        TaskSchedulerSnapshot {
            source_hash: self.bytecode.source_hash,
            builtin_manifest_hash: self.bytecode.builtin_manifest_hash,
            next_task_id: self.next_task_id,
            tasks: self
                .tasks
                .iter()
                .map(|(id, task)| {
                    (
                        *id,
                        TaskSnapshot {
                            template: task.template,
                            pc: task.pc,
                            current_function: task.current_function,
                            stack: task.stack.clone(),
                            locals: task.locals.clone(),
                            call_frames: task.call_frames.clone(),
                            status: task.status.clone(),
                        },
                    )
                })
                .collect(),
            globals: self.globals.clone(),
        }
    }

    pub fn restore(
        bytecode: Bytecode,
        snapshot: TaskSchedulerSnapshot,
    ) -> Result<Self, TaskSchedulerError> {
        if bytecode.source_hash != snapshot.source_hash {
            return Err(TaskSchedulerError::SourceHashMismatch);
        }
        if bytecode.builtin_manifest_hash != snapshot.builtin_manifest_hash {
            return Err(TaskSchedulerError::BuiltinManifestMismatch);
        }
        let mut scheduler = Self::new(bytecode)?;
        scheduler.next_task_id = snapshot.next_task_id;
        scheduler.globals = snapshot.globals;
        for (id, task) in snapshot.tasks {
            scheduler.template(task.template)?;
            let instruction_length = task
                .current_function
                .map(|function| {
                    scheduler
                        .bytecode
                        .functions
                        .get(function.0 as usize)
                        .map(|function| function.instructions.len())
                        .ok_or(TaskSchedulerError::UnknownFunction(function))
                })
                .transpose()?
                .unwrap_or_else(|| {
                    scheduler.bytecode.tasks[task.template as usize]
                        .instructions
                        .len()
                });
            if task.pc > instruction_length {
                return Err(TaskSchedulerError::InvalidProgramCounter(task.pc));
            }
            scheduler.tasks.insert(
                id,
                ScheduledTask {
                    template: task.template,
                    pc: task.pc,
                    current_function: task.current_function,
                    stack: task.stack,
                    locals: task.locals,
                    call_frames: task.call_frames,
                    status: task.status,
                },
            );
        }
        Ok(scheduler)
    }

    fn step_task(&mut self, task_id: u64) -> Result<Option<TaskEvent>, TaskSchedulerError> {
        let template_id = self
            .tasks
            .get(&task_id)
            .ok_or(TaskSchedulerError::UnknownTask(task_id))?
            .template;
        let template = self.template(template_id)?.clone();
        let at_template_root = self
            .tasks
            .get(&task_id)
            .is_some_and(|task| task.current_function.is_none() && task.pc == 0);
        if template.mode == TaskMode::Parallel && at_template_root {
            let children = template
                .children
                .iter()
                .map(|template| self.spawn(*template))
                .collect::<Result<Vec<_>, _>>()?;
            self.tasks
                .get_mut(&task_id)
                .ok_or(TaskSchedulerError::UnknownTask(task_id))?
                .status = TaskStatus::WaitingForChildren(children);
            return Ok(None);
        }

        let current_function = self
            .tasks
            .get(&task_id)
            .ok_or(TaskSchedulerError::UnknownTask(task_id))?
            .current_function;
        let instructions = match current_function {
            Some(function) => self
                .bytecode
                .functions
                .get(function.0 as usize)
                .map(|function| function.instructions.clone())
                .ok_or(TaskSchedulerError::UnknownFunction(function))?,
            None => template.instructions.clone(),
        };
        let instruction = {
            let task = self
                .tasks
                .get_mut(&task_id)
                .ok_or(TaskSchedulerError::UnknownTask(task_id))?;
            let instruction = instructions
                .get(task.pc)
                .cloned()
                .ok_or(TaskSchedulerError::InvalidProgramCounter(task.pc))?;
            task.pc += 1;
            instruction
        };
        match instruction {
            Instruction::Constant(value) => self.task_mut(task_id)?.stack.push(value),
            Instruction::LoadLocal(name) => {
                let value = self
                    .task_mut(task_id)?
                    .locals
                    .get(&name)
                    .cloned()
                    .ok_or(TaskSchedulerError::UnknownLocal(name))?;
                self.task_mut(task_id)?.stack.push(value);
            }
            Instruction::StoreLocal(name) => {
                let value = self.pop_task(task_id)?;
                self.task_mut(task_id)?.locals.insert(name, value);
            }
            Instruction::StoreLocalMember { root, path } => {
                let new_value = self.pop_task(task_id)?;
                let value = self
                    .task_mut(task_id)?
                    .locals
                    .get_mut(&root)
                    .ok_or_else(|| TaskSchedulerError::UnknownLocal(root.clone()))?;
                set_member_path(value, &path, new_value).map_err(|error| match error {
                    MemberPathError::Unknown(name) => TaskSchedulerError::UnknownMember(name),
                    MemberPathError::NotRecord => {
                        TaskSchedulerError::TypeMismatch("member receiver is not a record")
                    }
                })?;
            }
            Instruction::LoadGlobal(name) => {
                let value = self
                    .globals
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| TaskSchedulerError::UnknownGlobal(name.clone()))?;
                if value == Value::Uninitialized {
                    return Err(TaskSchedulerError::UninitializedGlobal(name));
                }
                self.task_mut(task_id)?.stack.push(value);
            }
            Instruction::StoreGlobal(name) => {
                let value = self.pop_task(task_id)?;
                self.globals.insert(name, value);
            }
            Instruction::StoreGlobalMember { root, path } => {
                let new_value = self.pop_task(task_id)?;
                let value = self
                    .globals
                    .get_mut(&root)
                    .ok_or_else(|| TaskSchedulerError::UnknownGlobal(root.clone()))?;
                set_member_path(value, &path, new_value).map_err(|error| match error {
                    MemberPathError::Unknown(name) => TaskSchedulerError::UnknownMember(name),
                    MemberPathError::NotRecord => {
                        TaskSchedulerError::TypeMismatch("member receiver is not a record")
                    }
                })?;
            }
            Instruction::MakeTuple(count) => {
                let values = self.pop_task_count(task_id, count)?;
                self.task_mut(task_id)?.stack.push(Value::Tuple(values));
            }
            Instruction::MakeList(count) => {
                let values = self.pop_task_count(task_id, count)?;
                self.task_mut(task_id)?.stack.push(Value::List(values));
            }
            Instruction::MakeMap(keys) => {
                let values = self.pop_task_count(task_id, keys.len())?;
                self.task_mut(task_id)?
                    .stack
                    .push(Value::Map(keys.into_iter().zip(values).collect()));
            }
            Instruction::Negate => {
                let Value::Number(value) = self.pop_task(task_id)? else {
                    return Err(TaskSchedulerError::TypeMismatch(
                        "cannot negate a non-number",
                    ));
                };
                self.task_mut(task_id)?.stack.push(Value::Number(-value));
            }
            Instruction::Equal => {
                let right = self.pop_task(task_id)?;
                let left = self.pop_task(task_id)?;
                self.task_mut(task_id)?
                    .stack
                    .push(Value::Bool(left == right));
            }
            Instruction::GetMember { name, safe } => {
                let value = self.pop_task(task_id)?;
                if value == Value::Null && safe {
                    self.task_mut(task_id)?.stack.push(Value::Null);
                    return Ok(None);
                }
                let value = match value {
                    Value::Map(mut fields) => fields
                        .remove(&name)
                        .ok_or_else(|| TaskSchedulerError::UnknownMember(name.clone()))?,
                    Value::Typed { value, .. } => match *value {
                        Value::Map(mut fields) => fields
                            .remove(&name)
                            .ok_or_else(|| TaskSchedulerError::UnknownMember(name.clone()))?,
                        _ => {
                            return Err(TaskSchedulerError::TypeMismatch(
                                "member receiver is not a record",
                            ));
                        }
                    },
                    Value::Null => return Err(TaskSchedulerError::NullMemberAccess(name)),
                    _ => {
                        return Err(TaskSchedulerError::TypeMismatch(
                            "member receiver is not a record",
                        ));
                    }
                };
                self.task_mut(task_id)?.stack.push(value);
            }
            Instruction::AssertNonNull => {
                if self.task_mut(task_id)?.stack.last() == Some(&Value::Null) {
                    return Err(TaskSchedulerError::NullAssertion);
                }
            }
            Instruction::CallBuiltin {
                builtin,
                labels,
                has_receiver,
            } => {
                let mut values =
                    self.pop_task_count(task_id, labels.len() + usize::from(has_receiver))?;
                let receiver = if has_receiver {
                    Some(values.remove(0))
                } else {
                    None
                };
                self.task_mut(task_id)?.status = TaskStatus::WaitingForHost;
                return Ok(Some(TaskEvent::Call {
                    task: task_id,
                    call: BuiltinCall {
                        builtin,
                        receiver,
                        arguments: labels
                            .into_iter()
                            .zip(values)
                            .map(|(label, value)| CallArgument { label, value })
                            .collect(),
                    },
                }));
            }
            Instruction::CallFunction {
                function,
                argument_count,
            } => {
                let parameters = self
                    .bytecode
                    .functions
                    .get(function.0 as usize)
                    .ok_or(TaskSchedulerError::UnknownFunction(function))?
                    .parameters
                    .clone();
                if parameters.len() != argument_count {
                    return Err(TaskSchedulerError::FunctionArity {
                        function,
                        expected: parameters.len(),
                        actual: argument_count,
                    });
                }
                let values = self.pop_task_count(task_id, argument_count)?;
                let task = self.task_mut(task_id)?;
                task.call_frames.push(CallFrame {
                    function: task.current_function,
                    pc: task.pc,
                    locals: std::mem::take(&mut task.locals),
                });
                task.current_function = Some(function);
                task.pc = 0;
                task.locals = parameters.into_iter().zip(values).collect();
            }
            Instruction::Return => {
                let value = self.pop_task(task_id)?;
                let task = self.task_mut(task_id)?;
                let frame = task
                    .call_frames
                    .pop()
                    .ok_or(TaskSchedulerError::ReturnOutsideFunction)?;
                task.current_function = frame.function;
                task.pc = frame.pc;
                task.locals = frame.locals;
                task.stack.push(value);
            }
            Instruction::Jump(target) => self.task_mut(task_id)?.pc = target,
            Instruction::JumpIfFalse(target) => {
                let Value::Bool(condition) = self.pop_task(task_id)? else {
                    return Err(TaskSchedulerError::TypeMismatch("condition must be bool"));
                };
                if !condition {
                    self.task_mut(task_id)?.pc = target;
                }
            }
            Instruction::JumpIfNotNull(target) => {
                if self
                    .task_mut(task_id)?
                    .stack
                    .last()
                    .is_some_and(|value| value != &Value::Null)
                {
                    self.task_mut(task_id)?.pc = target;
                }
            }
            Instruction::Statement(value) => {
                return Ok(Some(TaskEvent::Statement {
                    task: task_id,
                    value,
                }));
            }
            Instruction::SpawnTask { task } => {
                let child = self.spawn(task)?;
                self.task_mut(task_id)?.stack.push(Value::Task(child));
            }
            Instruction::Pop => {
                self.pop_task(task_id)?;
            }
            Instruction::Halt => {
                let value = self.task_mut(task_id)?.stack.pop().unwrap_or(Value::Null);
                self.task_mut(task_id)?.status = TaskStatus::Completed(value.clone());
                return Ok(Some(TaskEvent::Completed {
                    task: task_id,
                    value,
                }));
            }
        }
        Ok(None)
    }

    fn settle_completed_children(&mut self) -> Result<Option<TaskEvent>, TaskSchedulerError> {
        let waiting = self
            .tasks
            .iter()
            .filter_map(|(id, task)| match &task.status {
                TaskStatus::WaitingForChildren(children) => Some((*id, children.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (task_id, children) in waiting {
            let mut values = Vec::with_capacity(children.len());
            for child in children {
                let child = self
                    .tasks
                    .get(&child)
                    .ok_or(TaskSchedulerError::UnknownTask(child))?;
                let TaskStatus::Completed(value) = &child.status else {
                    values.clear();
                    break;
                };
                values.push(value.clone());
            }
            if !values.is_empty()
                || matches!(self.status(task_id), Some(TaskStatus::WaitingForChildren(children)) if children.is_empty())
            {
                let value = Value::Tuple(values);
                self.task_mut(task_id)?.status = TaskStatus::Completed(value.clone());
                return Ok(Some(TaskEvent::Completed {
                    task: task_id,
                    value,
                }));
            }
        }
        Ok(None)
    }

    fn template(&self, id: u32) -> Result<&TaskTemplate, TaskSchedulerError> {
        self.bytecode
            .tasks
            .get(id as usize)
            .ok_or(TaskSchedulerError::UnknownTemplate(id))
    }

    fn task_mut(&mut self, id: u64) -> Result<&mut ScheduledTask, TaskSchedulerError> {
        self.tasks
            .get_mut(&id)
            .ok_or(TaskSchedulerError::UnknownTask(id))
    }

    fn pop_task(&mut self, id: u64) -> Result<Value, TaskSchedulerError> {
        self.task_mut(id)?
            .stack
            .pop()
            .ok_or(TaskSchedulerError::StackUnderflow)
    }

    fn pop_task_count(&mut self, id: u64, count: usize) -> Result<Vec<Value>, TaskSchedulerError> {
        let task = self.task_mut(id)?;
        if task.stack.len() < count {
            return Err(TaskSchedulerError::StackUnderflow);
        }
        let start = task.stack.len() - count;
        Ok(task.stack.split_off(start))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskSchedulerError {
    UnsupportedBytecode(u16),
    SourceHashMismatch,
    BuiltinManifestMismatch,
    InvalidProgramCounter(usize),
    StackUnderflow,
    UnknownLocal(String),
    UnknownGlobal(String),
    UninitializedGlobal(String),
    UnknownMember(String),
    NullMemberAccess(String),
    NullAssertion,
    UnknownTask(u64),
    UnknownTemplate(u32),
    NotWaitingForHost,
    TypeMismatch(&'static str),
    UnknownFunction(FunctionId),
    FunctionArity {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    ReturnOutsideFunction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    UnsupportedBytecode(u16),
    InvalidProgramCounter(usize),
    SourceHashMismatch,
    BuiltinManifestMismatch,
    StackUnderflow,
    UnknownLocal(String),
    UnknownGlobal(String),
    UninitializedGlobal(String),
    UnknownMember(String),
    NullMemberAccess(String),
    NullAssertion,
    UnknownTask(u32),
    UnknownFunction(FunctionId),
    FunctionArity {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    ReturnOutsideFunction,
    NotWaitingForHost,
    TypeMismatch(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_program;

    fn camera_manifest() -> BuiltinManifest {
        BuiltinManifest::with_selectors(
            BTreeMap::new(),
            BTreeMap::from([(("camera".to_string(), "zoom".to_string()), BuiltinId(10))]),
        )
    }

    #[test]
    fn calls_user_functions_and_restores_call_frames() {
        let program = parse_program(
            r#"
                fn relay(value) {
                    nativeEcho(value)
                }
                relay("hello")
            "#,
        )
        .unwrap();
        let manifest = BuiltinManifest::new([("nativeEcho", BuiltinId(7))]);
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        assert_eq!(bytecode.functions.len(), 1);

        let mut vm = Vm::new(bytecode.clone()).unwrap();
        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!("expected native call from user function")
        };
        assert_eq!(call.arguments[0].value, Value::String("hello".to_string()));
        let snapshot = vm.snapshot();
        assert_eq!(snapshot.current_function, Some(FunctionId(0)));
        assert_eq!(snapshot.call_frames.len(), 1);

        let mut restored = Vm::restore(bytecode, snapshot).unwrap();
        restored
            .resume_builtin(Value::String("hello".to_string()))
            .unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn compiles_generic_equality_and_control_flow() {
        let program = parse_program(
            r#"
                if "route" == "gallery" {
                    log("yes")
                } else {
                    log("no")
                }
            "#,
        )
        .unwrap();
        let manifest = BuiltinManifest::new([("log", BuiltinId(1))]);
        let bytecode = compile_with_manifest(&program, 1, &manifest).unwrap();
        let mut vm = Vm::new(bytecode).unwrap();
        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!("expected selected branch call")
        };
        assert_eq!(call.arguments[0].value, Value::String("no".to_string()));
    }

    #[test]
    fn registered_fluent_calls_use_handles_and_commit_after_the_statement() {
        let program = parse_program(r#"char("Alice").e("eyes").e("face")"#).unwrap();
        let manifest = BuiltinManifest::new([("char", BuiltinId(1)), ("e", BuiltinId(2))]);
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        assert_eq!(bytecode.builtin_manifest_hash, manifest.hash());

        let mut vm = Vm::new(bytecode.clone()).unwrap();
        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!()
        };
        assert_eq!(call.builtin, BuiltinId(1));
        vm.resume_builtin(Value::Handle { type_id: 7, id: 9 })
            .unwrap();

        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!()
        };
        assert_eq!(call.builtin, BuiltinId(2));
        assert_eq!(call.receiver, Some(Value::Handle { type_id: 7, id: 9 }));
        assert_eq!(call.arguments[0].value, Value::String("eyes".to_string()));
        vm.resume_builtin(Value::Handle { type_id: 7, id: 9 })
            .unwrap();

        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).unwrap();
        let Some(VmEvent::Call(call)) = restored.step().unwrap() else {
            panic!()
        };
        assert_eq!(call.builtin, BuiltinId(2));
        restored
            .resume_builtin(Value::Handle { type_id: 7, id: 9 })
            .unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn yields_named_builtin_calls_and_restores_waiting_state() {
        let program =
            parse_program("let result = camera.zoom(1.2, at: .center, duration: 1)").unwrap();
        let manifest = camera_manifest();
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        let mut vm = Vm::new(bytecode.clone()).unwrap();

        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!("expected camera call");
        };
        assert_eq!(call.builtin, BuiltinId(10));
        assert_eq!(call.arguments[1].label.as_deref(), Some("at"));
        assert_eq!(call.arguments[1].value, Value::Symbol("center".to_string()));

        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).unwrap();
        restored.resume_builtin(Value::Null).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn preserves_percent_tuple_arguments_for_typed_builtins() {
        let program = parse_program("camera.zoom(1.2, at: (20%, 30%))").unwrap();
        let manifest = camera_manifest();
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        let mut vm = Vm::new(bytecode).unwrap();
        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!("expected camera call");
        };
        assert_eq!(
            call.arguments[1].value,
            Value::Tuple(vec![Value::Percent(20.0), Value::Percent(30.0)])
        );
    }

    #[test]
    fn compiles_seq_as_a_host_spawned_task_template() {
        let program = parse_program("let handle = seq { camera.zoom(1.2) }").unwrap();
        let manifest = camera_manifest();
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        assert_eq!(bytecode.tasks.len(), 1);
        assert_eq!(bytecode.tasks[0].mode, TaskMode::Sequence);

        let mut vm = Vm::new(bytecode.clone()).unwrap();
        let Some(VmEvent::SpawnTask(request)) = vm.step().unwrap() else {
            panic!("expected task spawn");
        };
        assert_eq!(request.task, 0);
        assert_eq!(request.template, bytecode.tasks[0]);

        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).unwrap();
        restored.resume(Value::Task(7)).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn compiles_par_with_one_child_task_per_statement() {
        let program = parse_program("let handles = par { first(); second() }").unwrap();
        let manifest = BuiltinManifest::new([("first", BuiltinId(1)), ("second", BuiltinId(2))]);
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        assert_eq!(bytecode.tasks.len(), 3);
        assert_eq!(bytecode.tasks[2].mode, TaskMode::Parallel);
        assert_eq!(bytecode.tasks[2].children, vec![0, 1]);
    }

    #[test]
    fn scheduler_runs_sequence_tasks_and_restores_waiting_state() {
        let program = parse_program("let handle = seq { first(); second() }").unwrap();
        let manifest = BuiltinManifest::new([("first", BuiltinId(1)), ("second", BuiltinId(2))]);
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        let mut scheduler = TaskScheduler::new(bytecode.clone()).unwrap();
        let task = scheduler.spawn(0).unwrap();

        let Some(TaskEvent::Call {
            task: yielded,
            call,
        }) = scheduler.step().unwrap()
        else {
            panic!("expected first call");
        };
        assert_eq!(yielded, task);
        assert_eq!(call.builtin, BuiltinId(1));

        let snapshot = scheduler.snapshot();
        let mut restored = TaskScheduler::restore(bytecode, snapshot).unwrap();
        restored.resume(task, Value::Null).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Statement {
                task,
                value: StatementValue::Commit
            })
        );
        let Some(TaskEvent::Call {
            task: yielded,
            call,
        }) = restored.step().unwrap()
        else {
            panic!("expected second call");
        };
        assert_eq!(yielded, task);
        assert_eq!(call.builtin, BuiltinId(2));
        restored.resume(task, Value::Null).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Statement {
                task,
                value: StatementValue::Commit
            })
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Completed {
                task,
                value: Value::Null,
            })
        );
    }

    #[test]
    fn scheduler_restores_user_function_call_frames() {
        let program = parse_program(
            r#"
                fn relay(value) { nativeEcho(value) }
                seq { relay("task") }
            "#,
        )
        .unwrap();
        let manifest = BuiltinManifest::new([("nativeEcho", BuiltinId(9))]);
        let bytecode = compile_with_manifest(&program, 55, &manifest).unwrap();
        let mut scheduler = TaskScheduler::new(bytecode.clone()).unwrap();
        let task = scheduler.spawn(0).unwrap();
        let Some(TaskEvent::Call { call, .. }) = scheduler.step().unwrap() else {
            panic!("expected call from task function")
        };
        assert_eq!(call.arguments[0].value, Value::String("task".to_string()));
        let snapshot = scheduler.snapshot();
        assert_eq!(snapshot.tasks[&task].current_function, Some(FunctionId(0)));

        let mut restored = TaskScheduler::restore(bytecode, snapshot).unwrap();
        restored
            .resume(task, Value::String("task".to_string()))
            .unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Statement {
                task,
                value: StatementValue::Commit
            })
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Statement {
                task,
                value: StatementValue::Commit
            })
        );
        assert!(matches!(
            restored.step().unwrap(),
            Some(TaskEvent::Completed { task: completed, .. }) if completed == task
        ));
    }

    #[test]
    fn scheduler_starts_parallel_children_in_task_id_order() {
        let program = parse_program("let handles = par { first(); second() }").unwrap();
        let manifest = BuiltinManifest::new([("first", BuiltinId(1)), ("second", BuiltinId(2))]);
        let bytecode = compile_with_manifest(&program, 42, &manifest).unwrap();
        let mut scheduler = TaskScheduler::new(bytecode).unwrap();
        let parent = scheduler.spawn(2).unwrap();

        let Some(TaskEvent::Call { task: first, call }) = scheduler.step().unwrap() else {
            panic!("expected first child call");
        };
        assert_eq!(call.builtin, BuiltinId(1));
        scheduler.resume(first, Value::Null).unwrap();
        assert_eq!(
            scheduler.step().unwrap(),
            Some(TaskEvent::Statement {
                task: first,
                value: StatementValue::Commit
            })
        );
        assert_eq!(
            scheduler.step().unwrap(),
            Some(TaskEvent::Completed {
                task: first,
                value: Value::Null,
            })
        );

        let Some(TaskEvent::Call { task: second, call }) = scheduler.step().unwrap() else {
            panic!("expected second child call");
        };
        assert_eq!(call.builtin, BuiltinId(2));
        scheduler.resume(second, Value::Null).unwrap();
        assert_eq!(
            scheduler.step().unwrap(),
            Some(TaskEvent::Statement {
                task: second,
                value: StatementValue::Commit
            })
        );
        assert_eq!(
            scheduler.step().unwrap(),
            Some(TaskEvent::Completed {
                task: second,
                value: Value::Null,
            })
        );
        assert_eq!(
            scheduler.step().unwrap(),
            Some(TaskEvent::Completed {
                task: parent,
                value: Value::Tuple(vec![Value::Null, Value::Null]),
            })
        );
    }

    #[test]
    fn globals_support_lazy_initialization_and_snapshot_restore() {
        let program = parse_program(
            r#"
                global name: String
                name = "Alice"
                nativeEcho(name)
            "#,
        )
        .expect("global program must parse");
        let manifest = BuiltinManifest::new([("nativeEcho", BuiltinId(12))]);
        let bytecode = compile_with_manifest(&program, 70, &manifest)
            .expect("lazy global assignment must compile");
        let mut vm = Vm::new(bytecode.clone()).expect("VM must initialize");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        let snapshot = vm.snapshot();
        let mut vm = Vm::restore(bytecode, snapshot).expect("global snapshot must restore");
        let Some(VmEvent::Call(call)) = vm.step().expect("VM must advance") else {
            panic!("expected native call")
        };
        assert_eq!(call.arguments[0].value, Value::String("Alice".to_string()));
    }

    #[test]
    fn reading_an_uninitialized_global_is_a_runtime_error() {
        let program =
            parse_program("global name: String\nnativeEcho(name)").expect("lazy global must parse");
        let bytecode = compile_with_manifest(
            &program,
            71,
            &BuiltinManifest::new([("nativeEcho", BuiltinId(12))]),
        )
        .expect("lazy global declaration must compile");
        let mut vm = Vm::new(bytecode).expect("VM must initialize");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert_eq!(
            vm.step(),
            Err(VmError::UninitializedGlobal("name".to_string()))
        );
    }

    #[test]
    fn embedding_globals_have_a_fixed_mutable_schema() {
        let settings_type = ScriptType::Record(BTreeMap::from([(
            "bgmVolume".to_string(),
            ScriptType::Number,
        )]));
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new())
            .with_globals(BTreeMap::from([("settings".to_string(), settings_type)]));
        let program = parse_program("settings.bgmVolume = 0.5")
            .expect("settings field assignment must parse");
        let bytecode = compile_with_manifest(&program, 72, &manifest)
            .expect("known settings field must type-check");
        let mut vm = Vm::new(bytecode).expect("VM must initialize");
        vm.set_globals(BTreeMap::from([(
            "settings".to_string(),
            Value::Map(BTreeMap::from([(
                "bgmVolume".to_string(),
                Value::Number(1.0),
            )])),
        )]));
        assert_eq!(
            vm.step().expect("assignment must run"),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            vm.globals()["settings"],
            Value::Map(BTreeMap::from([(
                "bgmVolume".to_string(),
                Value::Number(0.5),
            )]))
        );

        let invalid = parse_program("settings.extra = 1").expect("syntax must parse");
        let errors = compile_with_manifest(&invalid, 73, &manifest)
            .expect_err("unknown settings fields must be compile errors");
        assert!(errors[0].message.contains("unknown or non-record field"));
    }

    #[test]
    fn typed_nullable_global_fields_accept_later_non_null_values() {
        let program = parse_program(
            r#"
                type Player = .{ name: String?, health: Float }
                global player: Player = .{ name: null, health: 0.0 }
                player.name = "Player"
            "#,
        )
        .expect("typed global record must parse");
        let bytecode = compile(&program, 74).expect("nullable field assignment must compile");
        let mut vm = Vm::new(bytecode).expect("VM must initialize");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert_eq!(
            vm.globals()["player"],
            Value::Map(BTreeMap::from([
                ("health".to_string(), Value::Number(0.0)),
                ("name".to_string(), Value::String("Player".to_string())),
            ]))
        );
    }

    #[test]
    fn typed_record_constructor_supplies_the_inferred_global_type() {
        let program = parse_program(
            r#"
                type Player = .{ name: String?, health: Float }
                global player = Player.{ name: null, health: 0.0 }
                player.name = "Player"
            "#,
        )
        .expect("typed record constructor must parse");
        compile(&program, 76).expect("typed constructor must preserve nullable field types");
    }

    #[test]
    fn untyped_null_field_error_suggests_both_explicit_forms() {
        let program = parse_program(
            r#"
                global player = .{ name: null }
                player.name = "Player"
            "#,
        )
        .expect("implicit record must parse");
        let errors = compile(&program, 77).expect_err("untyped null must be rejected");
        assert!(errors[0].message.contains("cannot infer a type for null"));
        assert!(errors[0].message.contains("global player: Type"));
        assert!(errors[0].message.contains("global player = Type.{ ... }"));
    }

    #[test]
    fn standalone_null_cannot_define_a_binding_type() {
        for source in ["global value = null", "let value = null", "null"] {
            let program = parse_program(source).expect("null example must parse");
            let errors = compile(&program, 78).expect_err("standalone null must not have a type");
            assert!(errors.iter().any(|error| error.message.contains("null")));
        }
    }

    #[test]
    fn local_record_fields_are_written_back_to_the_binding() {
        let program = parse_program(
            r#"
                let player = .{
                    name: "before",
                    stats: .{ health: 1.0 }
                }
                player.name = "after"
                player.stats.health = 2.0
            "#,
        )
        .expect("local member assignments must parse");
        let bytecode = compile(&program, 75).expect("local member assignments must compile");
        assert!(bytecode.instructions.iter().any(|instruction| matches!(
            instruction,
            Instruction::StoreLocalMember { root, path }
                if root == "player" && path == &["stats", "health"]
        )));
        let mut vm = Vm::new(bytecode).expect("VM must initialize");
        for _ in 0..3 {
            assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        }
        assert_eq!(
            vm.locals()["player"],
            Value::Map(BTreeMap::from([
                ("name".to_string(), Value::String("after".to_string())),
                (
                    "stats".to_string(),
                    Value::Map(BTreeMap::from([
                        ("health".to_string(), Value::Number(2.0),)
                    ])),
                ),
            ]))
        );
    }

    #[test]
    fn type_aliases_and_user_function_signatures_are_checked() {
        let valid = parse_program(
            r#"
                type Player = .{ name: String, health: Int }
                fn health(player: Player) -> Int { player.health }
                global player: Player = .{ name: "Alice", health: 123 }
                health(player)
            "#,
        )
        .expect("typed program must parse");
        compile(&valid, 74).expect("matching alias and function types must compile");

        let invalid = parse_program(
            r#"
                type Player = .{ name: String, health: Int }
                fn health(player: Player) -> Int { player.health }
                health("Alice")
            "#,
        )
        .expect("invalid typed call must still parse");
        let errors = compile(&invalid, 75).expect_err("typed arguments must be enforced");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("argument 1"))
        );
    }

    #[test]
    fn nullable_globals_elvis_and_non_null_assertions_are_checked() {
        let invalid = parse_program("global name: String = null").expect("syntax must parse");
        assert!(compile(&invalid, 72).is_err());

        let valid = parse_program(
            r#"
                global name: String? = null
                let shown: String = name ?: "fallback"
                nativeEcho(shown)
            "#,
        )
        .expect("nullable program must parse");
        let bytecode = compile_with_manifest(
            &valid,
            73,
            &BuiltinManifest::new([("nativeEcho", BuiltinId(12))]),
        )
        .expect("elvis must unwrap nullable String");
        let mut vm = Vm::new(bytecode).expect("VM must initialize");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        let Some(VmEvent::Call(call)) = vm.step().expect("VM must advance") else {
            panic!("expected native call")
        };
        assert_eq!(
            call.arguments[0].value,
            Value::String("fallback".to_string())
        );
    }
}
