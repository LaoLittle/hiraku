//! A small deterministic bytecode VM for HKS.
//!
//! The VM never accesses an ECS world. Builtin calls are yielded as data and
//! resumed by the embedding engine with a value.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Expr, ExprKind, Program, Span, Stmt};

pub const BYTECODE_VERSION: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BuiltinId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinManifest {
    hash: u64,
    names: BTreeMap<String, BuiltinId>,
}

impl BuiltinManifest {
    pub fn new(entries: impl IntoIterator<Item = (impl Into<String>, BuiltinId)>) -> Self {
        let names = entries
            .into_iter()
            .map(|(name, id)| (name.into(), id))
            .collect::<BTreeMap<_, _>>();
        let hash = names
            .iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash, (name, id)| {
                name.bytes()
                    .chain(id.0.to_le_bytes())
                    .fold(hash, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
                    })
            });
        Self { hash, names }
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    pub fn resolve(&self, name: &str) -> Option<BuiltinId> {
        self.names.get(name).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(String),
    Handle { type_id: u32, id: u64 },
    Task(u64),
    Tuple(Vec<Value>),
    Map(BTreeMap<String, Value>),
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
    MakeTuple(usize),
    MakeMap(Vec<String>),
    Negate,
    Equal,
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
    Return,
    StatementCommit,
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
    let mut declaration_errors = Vec::new();
    for statement in &program.statements {
        if let Stmt::Function { name, span, .. } = statement {
            if function_names.contains_key(name) {
                declaration_errors.push(CompileError {
                    message: format!("function `{name}` is defined more than once"),
                    span: span.clone(),
                });
            } else {
                function_names.insert(name.clone(), FunctionId(function_names.len() as u32));
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
}

impl Compiler<'_> {
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
            for (index, statement) in body.statements.iter().enumerate() {
                let is_last = index + 1 == body.statements.len();
                if is_last && let Stmt::Expr(expression) = statement {
                    self.expression(expression);
                    if self.manifest.is_some() {
                        self.instructions.push(Instruction::StatementCommit);
                    }
                    self.instructions.push(Instruction::Return);
                } else {
                    self.statement(statement);
                }
            }
            if !matches!(self.instructions.last(), Some(Instruction::Return)) {
                self.instructions.push(Instruction::Constant(Value::Null));
                self.instructions.push(Instruction::Return);
            }
            let instructions = std::mem::replace(&mut self.instructions, parent);
            self.functions.push(FunctionTemplate {
                name,
                parameters,
                instructions,
            });
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Function { span, .. } => self.errors.push(CompileError {
                message: "nested function definitions are not supported".to_string(),
                span: span.clone(),
            }),
            Stmt::Let { name, value, .. } => {
                self.expression(value);
                self.instructions
                    .push(Instruction::StoreLocal(name.clone()));
                if self.manifest.is_some() {
                    self.instructions.push(Instruction::StatementCommit);
                }
            }
            Stmt::Expr(expression) => {
                self.expression(expression);
                self.instructions.push(Instruction::Pop);
                if self.manifest.is_some() {
                    self.instructions.push(Instruction::StatementCommit);
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
            ExprKind::Ident(name) => self.instructions.push(Instruction::LoadLocal(name.clone())),
            ExprKind::Symbol(name) => self
                .instructions
                .push(Instruction::Constant(Value::Symbol(name.clone()))),
            ExprKind::Bool(value) => self
                .instructions
                .push(Instruction::Constant(Value::Bool(*value))),
            ExprKind::Number { value, unit } => {
                self.instructions.push(Instruction::Constant(match unit {
                    super::NumberUnit::Scalar => Value::Number(*value),
                    super::NumberUnit::Percent => Value::Percent(*value),
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
            ExprKind::Map(fields) => {
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
                    for argument in arguments {
                        if argument.label.is_some() {
                            self.errors.push(CompileError {
                                message: "user functions do not accept named arguments".to_string(),
                                span: argument.span.clone(),
                            });
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
                    if let Some(name) = flatten_callee(callee)
                        && let Some(builtin) = manifest.resolve(&name)
                    {
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
                    if let ExprKind::Member { object, name } = &callee.kind
                        && let Some(builtin) = manifest.resolve(name)
                    {
                        self.expression(object);
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
            ExprKind::Member { .. } | ExprKind::Block(_) => self.errors.push(CompileError {
                message: "expression is not a value".to_string(),
                span: expression.span.clone(),
            }),
            ExprKind::Binary { left, op, right } => {
                self.expression(left);
                self.expression(right);
                match op {
                    super::BinaryOp::Equal => self.instructions.push(Instruction::Equal),
                }
            }
        }
    }

    fn compile_task(&mut self, block: &super::Block, mode: TaskMode) -> u32 {
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

fn flatten_callee(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member { object, name } => Some(format!("{}.{}", flatten_callee(object)?, name)),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinCall {
    pub builtin: BuiltinId,
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
    StatementCommit,
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
            call_frames: Vec::new(),
            status: VmStatus::Ready,
        })
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
                Instruction::MakeTuple(count) => {
                    let values = self.pop_count(count)?;
                    self.stack.push(Value::Tuple(values));
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
                Instruction::CallBuiltin {
                    builtin,
                    labels,
                    has_receiver,
                } => {
                    let values = self.pop_count(labels.len() + usize::from(has_receiver))?;
                    let labels = if has_receiver {
                        std::iter::once(None).chain(labels).collect()
                    } else {
                        labels
                    };
                    self.status = VmStatus::WaitingForHost;
                    return Ok(Some(VmEvent::Call(BuiltinCall {
                        builtin,
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
                Instruction::StatementCommit => return Ok(Some(VmEvent::StatementCommit)),
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
    StatementCommit { task: u64 },
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
        })
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
            Instruction::MakeTuple(count) => {
                let values = self.pop_task_count(task_id, count)?;
                self.task_mut(task_id)?.stack.push(Value::Tuple(values));
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
            Instruction::CallBuiltin {
                builtin,
                labels,
                has_receiver,
            } => {
                let values =
                    self.pop_task_count(task_id, labels.len() + usize::from(has_receiver))?;
                let labels = if has_receiver {
                    std::iter::once(None).chain(labels).collect()
                } else {
                    labels
                };
                self.task_mut(task_id)?.status = TaskStatus::WaitingForHost;
                return Ok(Some(TaskEvent::Call {
                    task: task_id,
                    call: BuiltinCall {
                        builtin,
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
            Instruction::StatementCommit => {
                return Ok(Some(TaskEvent::StatementCommit { task: task_id }));
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
    use crate::hks::parse_program;

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
        assert_eq!(restored.step().unwrap(), Some(VmEvent::StatementCommit));
        assert_eq!(restored.step().unwrap(), Some(VmEvent::StatementCommit));
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
        assert_eq!(call.arguments[0].value, Value::Handle { type_id: 7, id: 9 });
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
        assert_eq!(restored.step().unwrap(), Some(VmEvent::StatementCommit));
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn yields_named_builtin_calls_and_restores_waiting_state() {
        let program =
            parse_program("let result = camera.zoom(1.2, at: .center, duration: 1)").unwrap();
        let manifest = BuiltinManifest::new([("camera.zoom", BuiltinId(10))]);
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
        assert_eq!(restored.step().unwrap(), Some(VmEvent::StatementCommit));
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn preserves_percent_tuple_arguments_for_typed_builtins() {
        let program = parse_program("camera.zoom(1.2, at: (20%, 30%))").unwrap();
        let manifest = BuiltinManifest::new([("camera.zoom", BuiltinId(10))]);
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
        let manifest = BuiltinManifest::new([("camera.zoom", BuiltinId(10))]);
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
        assert_eq!(restored.step().unwrap(), Some(VmEvent::StatementCommit));
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
            Some(TaskEvent::StatementCommit { task })
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
            Some(TaskEvent::StatementCommit { task })
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
            Some(TaskEvent::StatementCommit { task })
        );
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::StatementCommit { task })
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
            Some(TaskEvent::StatementCommit { task: first })
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
            Some(TaskEvent::StatementCommit { task: second })
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
}
