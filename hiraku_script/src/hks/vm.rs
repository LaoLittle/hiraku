//! A small deterministic bytecode VM for HKS.
//!
//! The VM never accesses an ECS world. Builtin calls are yielded as data and
//! resumed by the embedding engine with a value.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Expr, ExprKind, Program, Span, Stmt};

pub const BYTECODE_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(String),
    Task(u64),
    Tuple(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub version: u16,
    pub source_hash: u64,
    pub instructions: Vec<Instruction>,
    pub tasks: Vec<TaskTemplate>,
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
    Call {
        callee: String,
        labels: Vec<Option<String>>,
    },
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
    let mut compiler = Compiler {
        instructions: Vec::new(),
        tasks: Vec::new(),
        errors: Vec::new(),
    };
    for statement in &program.statements {
        compiler.statement(statement);
    }
    compiler.instructions.push(Instruction::Halt);
    if compiler.errors.is_empty() {
        Ok(Bytecode {
            version: BYTECODE_VERSION,
            source_hash,
            instructions: compiler.instructions,
            tasks: compiler.tasks,
        })
    } else {
        Err(compiler.errors)
    }
}

struct Compiler {
    instructions: Vec<Instruction>,
    tasks: Vec<TaskTemplate>,
    errors: Vec<CompileError>,
}

impl Compiler {
    fn statement(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let { name, value, .. } => {
                self.expression(value);
                self.instructions
                    .push(Instruction::StoreLocal(name.clone()));
            }
            Stmt::Expr(expression) => {
                self.expression(expression);
                self.instructions.push(Instruction::Pop);
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
                let Some(callee) = flatten_callee(callee) else {
                    self.errors.push(CompileError {
                        message: "call target must be an identifier or member path".to_string(),
                        span: callee.span.clone(),
                    });
                    return;
                };
                if let Some(block) = trailing_block {
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
                for argument in arguments {
                    self.expression(&argument.value);
                }
                self.instructions.push(Instruction::Call {
                    callee,
                    labels: arguments
                        .iter()
                        .map(|argument| argument.label.clone())
                        .collect(),
                });
            }
            ExprKind::Member { .. } | ExprKind::Block(_) => self.errors.push(CompileError {
                message: "expression is not a value".to_string(),
                span: expression.span.clone(),
            }),
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
    pub callee: String,
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
    pub pc: usize,
    pub stack: Vec<Value>,
    pub locals: BTreeMap<String, Value>,
    pub status: VmStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vm {
    bytecode: Bytecode,
    pc: usize,
    stack: Vec<Value>,
    locals: BTreeMap<String, Value>,
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
            stack: Vec::new(),
            locals: BTreeMap::new(),
            status: VmStatus::Ready,
        })
    }

    pub fn step(&mut self) -> Result<Option<VmEvent>, VmError> {
        if !matches!(self.status, VmStatus::Ready) {
            return Ok(None);
        }
        loop {
            let instruction = self
                .bytecode
                .instructions
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
                Instruction::Call { callee, labels } => {
                    let values = self.pop_count(labels.len())?;
                    self.status = VmStatus::WaitingForHost;
                    return Ok(Some(VmEvent::Call(BuiltinCall {
                        callee,
                        arguments: labels
                            .into_iter()
                            .zip(values)
                            .map(|(label, value)| CallArgument { label, value })
                            .collect(),
                    })));
                }
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
            pc: self.pc,
            stack: self.stack.clone(),
            locals: self.locals.clone(),
            status: self.status.clone(),
        }
    }

    pub fn restore(bytecode: Bytecode, snapshot: VmSnapshot) -> Result<Self, VmError> {
        if bytecode.source_hash != snapshot.source_hash {
            return Err(VmError::SourceHashMismatch);
        }
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        if snapshot.pc > bytecode.instructions.len() {
            return Err(VmError::InvalidProgramCounter(snapshot.pc));
        }
        Ok(Self {
            bytecode,
            pc: snapshot.pc,
            stack: snapshot.stack,
            locals: snapshot.locals,
            status: snapshot.status,
        })
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn pop_count(&mut self, count: usize) -> Result<Vec<Value>, VmError> {
        if self.stack.len() < count {
            return Err(VmError::StackUnderflow);
        }
        let start = self.stack.len() - count;
        Ok(self.stack.split_off(start))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    UnsupportedBytecode(u16),
    InvalidProgramCounter(usize),
    SourceHashMismatch,
    StackUnderflow,
    UnknownLocal(String),
    UnknownTask(u32),
    NotWaitingForHost,
    TypeMismatch(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hks::parse_program;

    #[test]
    fn yields_named_builtin_calls_and_restores_waiting_state() {
        let program =
            parse_program("let result = camera.zoom(1.2, at: .center, duration: 1)").unwrap();
        let bytecode = compile(&program, 42).unwrap();
        let mut vm = Vm::new(bytecode.clone()).unwrap();

        let Some(VmEvent::Call(call)) = vm.step().unwrap() else {
            panic!("expected camera call");
        };
        assert_eq!(call.callee, "camera.zoom");
        assert_eq!(call.arguments[1].label.as_deref(), Some("at"));
        assert_eq!(call.arguments[1].value, Value::Symbol("center".to_string()));

        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).unwrap();
        restored.resume_builtin(Value::Null).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn preserves_percent_tuple_arguments_for_typed_builtins() {
        let program = parse_program("camera.zoom(1.2, at: (20%, 30%))").unwrap();
        let bytecode = compile(&program, 42).unwrap();
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
        let bytecode = compile(&program, 42).unwrap();
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
            Some(VmEvent::Completed(Value::Null))
        );
    }

    #[test]
    fn compiles_par_with_one_child_task_per_statement() {
        let program = parse_program("let handles = par { first(); second() }").unwrap();
        let bytecode = compile(&program, 42).unwrap();
        assert_eq!(bytecode.tasks.len(), 3);
        assert_eq!(bytecode.tasks[2].mode, TaskMode::Parallel);
        assert_eq!(bytecode.tasks[2].children, vec![0, 1]);
    }
}
