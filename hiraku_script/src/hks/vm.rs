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
    stack: Vec<Value>,
    locals: BTreeMap<String, Value>,
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
    pub next_task_id: u64,
    pub tasks: BTreeMap<u64, TaskSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub template: u32,
    pub pc: usize,
    pub stack: Vec<Value>,
    pub locals: BTreeMap<String, Value>,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskEvent {
    Call { task: u64, call: BuiltinCall },
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
                stack: Vec::new(),
                locals: BTreeMap::new(),
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
                            stack: task.stack.clone(),
                            locals: task.locals.clone(),
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
        let mut scheduler = Self::new(bytecode)?;
        scheduler.next_task_id = snapshot.next_task_id;
        for (id, task) in snapshot.tasks {
            let template = scheduler.template(task.template)?;
            if task.pc > template.instructions.len() {
                return Err(TaskSchedulerError::InvalidProgramCounter(task.pc));
            }
            scheduler.tasks.insert(
                id,
                ScheduledTask {
                    template: task.template,
                    pc: task.pc,
                    stack: task.stack,
                    locals: task.locals,
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
        if template.mode == TaskMode::Parallel {
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

        let instruction = {
            let task = self
                .tasks
                .get_mut(&task_id)
                .ok_or(TaskSchedulerError::UnknownTask(task_id))?;
            let instruction = template
                .instructions
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
            Instruction::Call { callee, labels } => {
                let values = self.pop_task_count(task_id, labels.len())?;
                self.task_mut(task_id)?.status = TaskStatus::WaitingForHost;
                return Ok(Some(TaskEvent::Call {
                    task: task_id,
                    call: BuiltinCall {
                        callee,
                        arguments: labels
                            .into_iter()
                            .zip(values)
                            .map(|(label, value)| CallArgument { label, value })
                            .collect(),
                    },
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
    InvalidProgramCounter(usize),
    StackUnderflow,
    UnknownLocal(String),
    UnknownTask(u64),
    UnknownTemplate(u32),
    NotWaitingForHost,
    TypeMismatch(&'static str),
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

    #[test]
    fn scheduler_runs_sequence_tasks_and_restores_waiting_state() {
        let program = parse_program("let handle = seq { first(); second() }").unwrap();
        let bytecode = compile(&program, 42).unwrap();
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
        assert_eq!(call.callee, "first");

        let snapshot = scheduler.snapshot();
        let mut restored = TaskScheduler::restore(bytecode, snapshot).unwrap();
        restored.resume(task, Value::Null).unwrap();
        let Some(TaskEvent::Call {
            task: yielded,
            call,
        }) = restored.step().unwrap()
        else {
            panic!("expected second call");
        };
        assert_eq!(yielded, task);
        assert_eq!(call.callee, "second");
        restored.resume(task, Value::Null).unwrap();
        assert_eq!(
            restored.step().unwrap(),
            Some(TaskEvent::Completed {
                task,
                value: Value::Null,
            })
        );
    }

    #[test]
    fn scheduler_starts_parallel_children_in_task_id_order() {
        let program = parse_program("let handles = par { first(); second() }").unwrap();
        let bytecode = compile(&program, 42).unwrap();
        let mut scheduler = TaskScheduler::new(bytecode).unwrap();
        let parent = scheduler.spawn(2).unwrap();

        let Some(TaskEvent::Call { task: first, call }) = scheduler.step().unwrap() else {
            panic!("expected first child call");
        };
        assert_eq!(call.callee, "first");
        scheduler.resume(first, Value::Null).unwrap();
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
        assert_eq!(call.callee, "second");
        scheduler.resume(second, Value::Null).unwrap();
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
