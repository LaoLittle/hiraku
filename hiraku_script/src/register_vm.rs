//! Experimental executable register backend.
//!
//! This backend intentionally lives beside the production stack VM until task
//! regions, user call frames and engine save migration are complete.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    HirArena, MirConstant, MirInstruction, MirTerminator, Program, Register, RegisterFrame,
    ResolvedFunction, Span, StatementValue, SymbolId, SymbolManifest, allocate_registers,
    lower_hir_to_mir, lower_to_hir,
    vm::{BuiltinCall, BuiltinManifest, CallArgument, Value},
};

pub const REGISTER_BYTECODE_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterBytecode {
    pub version: u16,
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub symbols: SymbolManifest,
    pub globals: Vec<SymbolId>,
    pub local_count: u32,
    pub register_count: u16,
    pub instructions: Vec<RegisterInstruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RegisterInstruction {
    Constant {
        dst: Register,
        value: RegisterConstant,
    },
    LoadLocal {
        dst: Register,
        local: u32,
    },
    StoreLocal {
        local: u32,
        src: Register,
    },
    LoadGlobal {
        dst: Register,
        global: u32,
    },
    StoreGlobal {
        global: u32,
        src: Register,
    },
    GetMember {
        dst: Register,
        object: Register,
        member: SymbolId,
        safe: bool,
    },
    SetMember {
        dst: Register,
        object: Register,
        member: SymbolId,
        value: Register,
    },
    UnaryMinus {
        dst: Register,
        value: Register,
    },
    Binary {
        dst: Register,
        op: crate::BinaryOp,
        left: Register,
        right: Register,
    },
    MakeTuple {
        dst: Register,
        values: Vec<Register>,
    },
    MakeList {
        dst: Register,
        values: Vec<Register>,
    },
    MakeMap {
        dst: Register,
        fields: Vec<(SymbolId, Register)>,
    },
    CallBuiltin {
        dst: Register,
        builtin: crate::vm::BuiltinId,
        receiver: Option<Register>,
        arguments: Vec<(Option<SymbolId>, Register)>,
    },
    AssertNonNull {
        dst: Register,
        value: Register,
    },
    SelectNonNull {
        dst: Register,
        value: Register,
        fallback: Register,
    },
    Statement {
        value: Register,
    },
    Jump(usize),
    Branch {
        condition: Register,
        then_target: usize,
        else_target: usize,
    },
    Halt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RegisterConstant {
    Null,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(SymbolId),
    Selector(SymbolId),
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterCompileError {
    pub message: String,
    pub span: Option<Span>,
}

pub fn compile_register_with_manifest(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
) -> Result<RegisterBytecode, Vec<RegisterCompileError>> {
    let arena = HirArena::new();
    let hir = lower_to_hir(&arena, program, Some(manifest)).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| RegisterCompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    if !hir.functions.is_empty() {
        return Err(vec![RegisterCompileError {
            message: "register bytecode user call frames are not implemented yet".into(),
            span: hir.functions.first().map(|function| function.span),
        }]);
    }
    let mir = lower_hir_to_mir(&hir).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| RegisterCompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    let allocation = allocate_registers(&mir.entry).map_err(|error| {
        vec![RegisterCompileError {
            message: format!("register allocation failed: {error:?}"),
            span: None,
        }]
    })?;
    let instructions = emit_function(&mir.entry, &allocation)?;
    Ok(RegisterBytecode {
        version: REGISTER_BYTECODE_VERSION,
        source_hash,
        builtin_manifest_hash: manifest.hash(),
        symbols: hir.symbols,
        globals: hir.globals.iter().map(|global| global.name).collect(),
        local_count: hir.locals.len() as u32,
        register_count: allocation.register_count,
        instructions,
    })
}

fn emit_function(
    function: &crate::MirFunction,
    allocation: &crate::RegisterAllocation,
) -> Result<Vec<RegisterInstruction>, Vec<RegisterCompileError>> {
    let mut starts = Vec::with_capacity(function.blocks.len());
    let mut offset = 0usize;
    for block in &function.blocks {
        starts.push(offset);
        offset += block.instructions.len() + 1;
    }
    let register = |virtual_register| {
        allocation
            .register_for(virtual_register)
            .expect("MIR virtual register was allocated")
    };
    let mut output = Vec::with_capacity(offset);
    for block in &function.blocks {
        for instruction in &block.instructions {
            output.push(match instruction {
                MirInstruction::Constant { dst, value } => RegisterInstruction::Constant {
                    dst: register(*dst),
                    value: constant(value.clone()),
                },
                MirInstruction::LoadLocal { dst, local } => RegisterInstruction::LoadLocal {
                    dst: register(*dst),
                    local: local.0,
                },
                MirInstruction::StoreLocal { local, src } => RegisterInstruction::StoreLocal {
                    local: local.0,
                    src: register(*src),
                },
                MirInstruction::LoadGlobal { dst, global } => RegisterInstruction::LoadGlobal {
                    dst: register(*dst),
                    global: global.0,
                },
                MirInstruction::StoreGlobal { global, src } => RegisterInstruction::StoreGlobal {
                    global: global.0,
                    src: register(*src),
                },
                MirInstruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                } => RegisterInstruction::GetMember {
                    dst: register(*dst),
                    object: register(*object),
                    member: *member,
                    safe: *safe,
                },
                MirInstruction::SetMember {
                    dst,
                    object,
                    member,
                    value,
                } => RegisterInstruction::SetMember {
                    dst: register(*dst),
                    object: register(*object),
                    member: *member,
                    value: register(*value),
                },
                MirInstruction::UnaryMinus { dst, value } => RegisterInstruction::UnaryMinus {
                    dst: register(*dst),
                    value: register(*value),
                },
                MirInstruction::Binary {
                    dst,
                    op,
                    left,
                    right,
                } => RegisterInstruction::Binary {
                    dst: register(*dst),
                    op: *op,
                    left: register(*left),
                    right: register(*right),
                },
                MirInstruction::MakeTuple { dst, values } => RegisterInstruction::MakeTuple {
                    dst: register(*dst),
                    values: values.iter().map(|value| register(*value)).collect(),
                },
                MirInstruction::MakeList { dst, values } => RegisterInstruction::MakeList {
                    dst: register(*dst),
                    values: values.iter().map(|value| register(*value)).collect(),
                },
                MirInstruction::MakeMap { dst, fields } => RegisterInstruction::MakeMap {
                    dst: register(*dst),
                    fields: fields
                        .iter()
                        .map(|(name, value)| (*name, register(*value)))
                        .collect(),
                },
                MirInstruction::Call {
                    dst,
                    function: ResolvedFunction::Builtin(builtin),
                    receiver,
                    dynamic_callee: None,
                    arguments,
                } => RegisterInstruction::CallBuiltin {
                    dst: register(*dst),
                    builtin: *builtin,
                    receiver: receiver.map(register),
                    arguments: arguments
                        .iter()
                        .map(|(label, value)| (*label, register(*value)))
                        .collect(),
                },
                MirInstruction::Call { .. } => {
                    return Err(vec![RegisterCompileError {
                        message: "dynamic and user calls are not implemented in register bytecode"
                            .into(),
                        span: None,
                    }]);
                }
                MirInstruction::AssertNonNull { dst, value } => {
                    RegisterInstruction::AssertNonNull {
                        dst: register(*dst),
                        value: register(*value),
                    }
                }
                MirInstruction::SelectNonNull {
                    dst,
                    value,
                    fallback,
                } => RegisterInstruction::SelectNonNull {
                    dst: register(*dst),
                    value: register(*value),
                    fallback: register(*fallback),
                },
                MirInstruction::Statement { value } => RegisterInstruction::Statement {
                    value: register(*value),
                },
            });
        }
        output.push(match &block.terminator {
            MirTerminator::Jump(target) => RegisterInstruction::Jump(starts[target.0 as usize]),
            MirTerminator::Branch {
                condition,
                then_block,
                else_block,
            } => RegisterInstruction::Branch {
                condition: register(*condition),
                then_target: starts[then_block.0 as usize],
                else_target: starts[else_block.0 as usize],
            },
            MirTerminator::Return(_) => {
                return Err(vec![RegisterCompileError {
                    message: "register bytecode user returns are not implemented yet".into(),
                    span: None,
                }]);
            }
            MirTerminator::Halt => RegisterInstruction::Halt,
            MirTerminator::Unset => {
                return Err(vec![RegisterCompileError {
                    message: "MIR block has no terminator".into(),
                    span: None,
                }]);
            }
        });
    }
    Ok(output)
}

fn constant(value: MirConstant) -> RegisterConstant {
    match value {
        MirConstant::Null | MirConstant::Unit => RegisterConstant::Null,
        MirConstant::Ellipsis => RegisterConstant::Ellipsis,
        MirConstant::Bool(value) => RegisterConstant::Bool(value),
        MirConstant::Number(value) => RegisterConstant::Number(value),
        MirConstant::Percent(value) => RegisterConstant::Percent(value),
        MirConstant::String(value) => RegisterConstant::String(value),
        MirConstant::Symbol(value) => RegisterConstant::Symbol(value),
        MirConstant::Selector(value) => RegisterConstant::Selector(value),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegisterVmStatus {
    Ready,
    WaitingForHost,
    Completed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegisterVmEvent {
    Call(BuiltinCall),
    Statement(StatementValue),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterVmSnapshot {
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Value>,
    pub globals: Vec<Value>,
    pub waiting_destination: Option<Register>,
    pub status: RegisterVmStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegisterVm {
    bytecode: RegisterBytecode,
    pc: usize,
    registers: RegisterFrame,
    locals: Box<[Value]>,
    globals: Box<[Value]>,
    waiting_destination: Option<Register>,
    status: RegisterVmStatus,
}

impl RegisterVm {
    pub fn new(bytecode: RegisterBytecode) -> Result<Self, RegisterVmError> {
        if bytecode.version != REGISTER_BYTECODE_VERSION {
            return Err(RegisterVmError::UnsupportedBytecode(bytecode.version));
        }
        Ok(Self {
            registers: RegisterFrame::new(bytecode.register_count),
            locals: vec![Value::Uninitialized; bytecode.local_count as usize].into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: RegisterVmStatus::Ready,
        })
    }

    pub fn step(&mut self) -> Result<Option<RegisterVmEvent>, RegisterVmError> {
        if self.status != RegisterVmStatus::Ready {
            return Ok(None);
        }
        loop {
            let instruction = self
                .bytecode
                .instructions
                .get(self.pc)
                .cloned()
                .ok_or(RegisterVmError::InvalidProgramCounter(self.pc))?;
            self.pc += 1;
            match instruction {
                RegisterInstruction::Constant { dst, value } => {
                    let value = self.constant_value(value)?;
                    self.write(dst, value)?;
                }
                RegisterInstruction::LoadLocal { dst, local } => {
                    let value = self.local(local)?.clone();
                    if value == Value::Uninitialized {
                        return Err(RegisterVmError::UninitializedLocal(local));
                    }
                    self.write(dst, value)?;
                }
                RegisterInstruction::StoreLocal { local, src } => {
                    let value = self.read(src)?.clone();
                    *self.local_mut(local)? = value;
                }
                RegisterInstruction::LoadGlobal { dst, global } => {
                    let value = self.global_slot(global)?.clone();
                    if value == Value::Uninitialized {
                        return Err(RegisterVmError::UninitializedGlobal(global));
                    }
                    self.write(dst, value)?;
                }
                RegisterInstruction::StoreGlobal { global, src } => {
                    let value = self.read(src)?.clone();
                    *self.global_mut(global)? = value;
                }
                RegisterInstruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                } => {
                    let name = self.symbol(member)?.to_string();
                    let value = get_member(self.read(object)?, &name, safe)?;
                    self.write(dst, value)?;
                }
                RegisterInstruction::SetMember {
                    dst,
                    object,
                    member,
                    value,
                } => {
                    let name = self.symbol(member)?.to_string();
                    let mut object = self.read(object)?.clone();
                    let value = self.read(value)?.clone();
                    set_member(&mut object, &name, value)?;
                    self.write(dst, object)?;
                }
                RegisterInstruction::UnaryMinus { dst, value } => {
                    let Value::Number(value) = self.read(value)? else {
                        return Err(RegisterVmError::TypeMismatch("unary minus expects Number"));
                    };
                    self.write(dst, Value::Number(-value))?;
                }
                RegisterInstruction::Binary {
                    dst,
                    op,
                    left,
                    right,
                } => {
                    let value = binary(op, self.read(left)?, self.read(right)?)?;
                    self.write(dst, value)?;
                }
                RegisterInstruction::MakeTuple { dst, values } => {
                    let values = self.read_many(&values)?;
                    self.write(dst, Value::Tuple(values))?;
                }
                RegisterInstruction::MakeList { dst, values } => {
                    let values = self.read_many(&values)?;
                    self.write(dst, Value::List(values))?;
                }
                RegisterInstruction::MakeMap { dst, fields } => {
                    let fields = fields
                        .into_iter()
                        .map(|(name, register)| {
                            Ok((self.symbol(name)?.to_string(), self.read(register)?.clone()))
                        })
                        .collect::<Result<BTreeMap<_, _>, RegisterVmError>>()?;
                    self.write(dst, Value::Map(fields))?;
                }
                RegisterInstruction::CallBuiltin {
                    dst,
                    builtin,
                    receiver,
                    arguments,
                } => {
                    let receiver = receiver
                        .map(|receiver| self.read(receiver).cloned())
                        .transpose()?;
                    let arguments = arguments
                        .into_iter()
                        .map(|(label, value)| {
                            Ok(CallArgument {
                                label: label
                                    .map(|label| self.symbol(label).map(str::to_string))
                                    .transpose()?,
                                value: self.read(value)?.clone(),
                            })
                        })
                        .collect::<Result<Vec<_>, RegisterVmError>>()?;
                    self.status = RegisterVmStatus::WaitingForHost;
                    self.waiting_destination = Some(dst);
                    return Ok(Some(RegisterVmEvent::Call(BuiltinCall {
                        builtin,
                        receiver,
                        arguments,
                    })));
                }
                RegisterInstruction::AssertNonNull { dst, value } => {
                    let value = self.read(value)?.clone();
                    if value == Value::Null {
                        return Err(RegisterVmError::NullAssertion);
                    }
                    self.write(dst, value)?;
                }
                RegisterInstruction::SelectNonNull {
                    dst,
                    value,
                    fallback,
                } => {
                    let value = self.read(value)?.clone();
                    let value = if value == Value::Null {
                        self.read(fallback)?.clone()
                    } else {
                        value
                    };
                    self.write(dst, value)?;
                }
                RegisterInstruction::Statement { value } => {
                    let value = self.read(value)?;
                    let statement = match value {
                        Value::String(value) => StatementValue::String(value.clone()),
                        _ => StatementValue::Commit,
                    };
                    return Ok(Some(RegisterVmEvent::Statement(statement)));
                }
                RegisterInstruction::Jump(target) => self.pc = target,
                RegisterInstruction::Branch {
                    condition,
                    then_target,
                    else_target,
                } => {
                    let Value::Bool(condition) = self.read(condition)? else {
                        return Err(RegisterVmError::TypeMismatch("branch expects Bool"));
                    };
                    self.pc = if *condition { then_target } else { else_target };
                }
                RegisterInstruction::Halt => {
                    self.status = RegisterVmStatus::Completed;
                    return Ok(Some(RegisterVmEvent::Completed(Value::Null)));
                }
            }
        }
    }

    pub fn resume(&mut self, value: Value) -> Result<(), RegisterVmError> {
        if self.status != RegisterVmStatus::WaitingForHost {
            return Err(RegisterVmError::NotWaitingForHost);
        }
        let destination = self
            .waiting_destination
            .take()
            .ok_or(RegisterVmError::NotWaitingForHost)?;
        self.write(destination, value)?;
        self.status = RegisterVmStatus::Ready;
        Ok(())
    }

    pub fn snapshot(&self) -> RegisterVmSnapshot {
        RegisterVmSnapshot {
            source_hash: self.bytecode.source_hash,
            builtin_manifest_hash: self.bytecode.builtin_manifest_hash,
            pc: self.pc,
            registers: self.registers.values().to_vec(),
            locals: self.locals.to_vec(),
            globals: self.globals.to_vec(),
            waiting_destination: self.waiting_destination,
            status: self.status,
        }
    }

    pub fn restore(
        bytecode: RegisterBytecode,
        snapshot: RegisterVmSnapshot,
    ) -> Result<Self, RegisterVmError> {
        if bytecode.source_hash != snapshot.source_hash {
            return Err(RegisterVmError::SourceHashMismatch);
        }
        if bytecode.builtin_manifest_hash != snapshot.builtin_manifest_hash {
            return Err(RegisterVmError::BuiltinManifestMismatch);
        }
        if snapshot.registers.len() != usize::from(bytecode.register_count)
            || snapshot.locals.len() != bytecode.local_count as usize
            || snapshot.globals.len() != bytecode.globals.len()
        {
            return Err(RegisterVmError::FrameShapeMismatch);
        }
        let mut registers = RegisterFrame::new(bytecode.register_count);
        for (index, value) in snapshot.registers.into_iter().enumerate() {
            registers
                .write(Register(index as u16), value)
                .map_err(|_| RegisterVmError::InvalidRegister(Register(index as u16)))?;
        }
        Ok(Self {
            bytecode,
            pc: snapshot.pc,
            registers,
            locals: snapshot.locals.into_boxed_slice(),
            globals: snapshot.globals.into_boxed_slice(),
            waiting_destination: snapshot.waiting_destination,
            status: snapshot.status,
        })
    }

    pub fn status(&self) -> RegisterVmStatus {
        self.status
    }

    pub fn global(&self, name: &str) -> Option<&Value> {
        self.bytecode
            .globals
            .iter()
            .position(|symbol| self.bytecode.symbols.resolve(*symbol) == Some(name))
            .and_then(|index| self.globals.get(index))
    }

    fn constant_value(&self, value: RegisterConstant) -> Result<Value, RegisterVmError> {
        Ok(match value {
            RegisterConstant::Null | RegisterConstant::Unit => Value::Null,
            RegisterConstant::Ellipsis => Value::Ellipsis,
            RegisterConstant::Bool(value) => Value::Bool(value),
            RegisterConstant::Number(value) => Value::Number(value),
            RegisterConstant::Percent(value) => Value::Percent(value),
            RegisterConstant::String(value) => Value::String(value),
            RegisterConstant::Symbol(symbol) => Value::Symbol(self.symbol(symbol)?.to_string()),
            RegisterConstant::Selector(symbol) => Value::Selector(self.symbol(symbol)?.to_string()),
        })
    }

    fn symbol(&self, symbol: SymbolId) -> Result<&str, RegisterVmError> {
        self.bytecode
            .symbols
            .resolve(symbol)
            .ok_or(RegisterVmError::UnknownSymbol(symbol))
    }

    fn read(&self, register: Register) -> Result<&Value, RegisterVmError> {
        self.registers
            .read(register)
            .ok_or(RegisterVmError::InvalidRegister(register))
    }

    fn write(&mut self, register: Register, value: Value) -> Result<(), RegisterVmError> {
        self.registers
            .write(register, value)
            .map_err(|_| RegisterVmError::InvalidRegister(register))
    }

    fn read_many(&self, registers: &[Register]) -> Result<Vec<Value>, RegisterVmError> {
        registers
            .iter()
            .map(|register| self.read(*register).cloned())
            .collect()
    }

    fn local(&self, local: u32) -> Result<&Value, RegisterVmError> {
        self.locals
            .get(local as usize)
            .ok_or(RegisterVmError::InvalidLocal(local))
    }

    fn local_mut(&mut self, local: u32) -> Result<&mut Value, RegisterVmError> {
        self.locals
            .get_mut(local as usize)
            .ok_or(RegisterVmError::InvalidLocal(local))
    }

    fn global_slot(&self, global: u32) -> Result<&Value, RegisterVmError> {
        self.globals
            .get(global as usize)
            .ok_or(RegisterVmError::InvalidGlobal(global))
    }

    fn global_mut(&mut self, global: u32) -> Result<&mut Value, RegisterVmError> {
        self.globals
            .get_mut(global as usize)
            .ok_or(RegisterVmError::InvalidGlobal(global))
    }
}

fn get_member(value: &Value, name: &str, safe: bool) -> Result<Value, RegisterVmError> {
    if value == &Value::Null && safe {
        return Ok(Value::Null);
    }
    match value {
        Value::Map(fields) => fields
            .get(name)
            .cloned()
            .ok_or_else(|| RegisterVmError::UnknownMember(name.to_string())),
        Value::Typed { value, .. } => get_member(value, name, safe),
        Value::Null => Err(RegisterVmError::NullMemberAccess(name.to_string())),
        _ => Err(RegisterVmError::TypeMismatch(
            "member receiver is not a record",
        )),
    }
}

fn set_member(value: &mut Value, name: &str, new_value: Value) -> Result<(), RegisterVmError> {
    let fields = match value {
        Value::Map(fields) => fields,
        Value::Typed { value, .. } => match value.as_mut() {
            Value::Map(fields) => fields,
            _ => {
                return Err(RegisterVmError::TypeMismatch(
                    "member receiver is not a record",
                ));
            }
        },
        _ => {
            return Err(RegisterVmError::TypeMismatch(
                "member receiver is not a record",
            ));
        }
    };
    let field = fields
        .get_mut(name)
        .ok_or_else(|| RegisterVmError::UnknownMember(name.to_string()))?;
    *field = new_value;
    Ok(())
}

fn binary(op: crate::BinaryOp, left: &Value, right: &Value) -> Result<Value, RegisterVmError> {
    use crate::BinaryOp;
    match op {
        BinaryOp::Equal => Ok(Value::Bool(left == right)),
        BinaryOp::NotEqual => Ok(Value::Bool(left != right)),
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide => {
            let (Value::Number(left), Value::Number(right)) = (left, right) else {
                return Err(RegisterVmError::TypeMismatch(
                    "arithmetic expects Number operands",
                ));
            };
            if op == BinaryOp::Divide && *right == 0.0 {
                return Err(RegisterVmError::DivisionByZero);
            }
            Ok(Value::Number(match op {
                BinaryOp::Add => left + right,
                BinaryOp::Subtract => left - right,
                BinaryOp::Multiply => left * right,
                BinaryOp::Divide => left / right,
                _ => unreachable!(),
            }))
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            let (Value::Number(left), Value::Number(right)) = (left, right) else {
                return Err(RegisterVmError::TypeMismatch(
                    "comparison expects Number operands",
                ));
            };
            Ok(Value::Bool(match op {
                BinaryOp::Less => left < right,
                BinaryOp::LessEqual => left <= right,
                BinaryOp::Greater => left > right,
                BinaryOp::GreaterEqual => left >= right,
                _ => unreachable!(),
            }))
        }
        BinaryOp::Colon => Err(RegisterVmError::TypeMismatch(
            "dialogue operator must resolve to a registered builtin",
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterVmError {
    UnsupportedBytecode(u16),
    InvalidProgramCounter(usize),
    InvalidRegister(Register),
    InvalidLocal(u32),
    InvalidGlobal(u32),
    UnknownSymbol(SymbolId),
    UnknownMember(String),
    NullMemberAccess(String),
    NullAssertion,
    UninitializedLocal(u32),
    UninitializedGlobal(u32),
    TypeMismatch(&'static str),
    DivisionByZero,
    NotWaitingForHost,
    SourceHashMismatch,
    BuiltinManifestMismatch,
    FrameShapeMismatch,
}

#[cfg(test)]
mod tests {
    use crate::{parse_program, vm::BuiltinId};

    use super::*;

    fn compile(source: &str, manifest: &BuiltinManifest) -> RegisterBytecode {
        compile_register_with_manifest(&parse_program(source).expect("source parses"), 91, manifest)
            .expect("register bytecode compiles")
    }

    #[test]
    fn executes_control_flow_and_recursive_member_updates() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
                global player = .{ stats: .{ health: 1 } }
                let index = 0
                while index < 3 {
                    player.stats.health += 1
                    index += 1
                }
            "#,
            &manifest,
        );
        let mut vm = RegisterVm::new(bytecode).expect("VM initializes");
        loop {
            if matches!(
                vm.step().expect("VM executes"),
                Some(RegisterVmEvent::Completed(_))
            ) {
                break;
            }
        }
        let Value::Map(player) = vm.global("player").expect("player global exists") else {
            panic!("player is a record")
        };
        let Value::Map(stats) = &player["stats"] else {
            panic!("stats is a record")
        };
        assert_eq!(stats["health"], Value::Number(4.0));
    }

    #[test]
    fn native_wait_state_restores_and_resumes_into_its_destination() {
        let builtin = BuiltinId(8);
        let manifest = BuiltinManifest::new([("nativeValue", builtin)]);
        let bytecode = compile("let value = nativeValue()\nvalue += 1", &manifest);
        let mut vm = RegisterVm::new(bytecode.clone()).expect("VM initializes");
        let Some(RegisterVmEvent::Call(call)) = vm.step().expect("call yields") else {
            panic!("expected native call")
        };
        assert_eq!(call.builtin, builtin);
        let snapshot = vm.snapshot();
        let mut restored =
            RegisterVm::restore(bytecode, snapshot).expect("waiting VM snapshot restores");
        restored
            .resume(Value::Number(4.0))
            .expect("host value resumes VM");
        assert_eq!(
            restored.step().expect("statement executes"),
            Some(RegisterVmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().expect("assignment executes"),
            Some(RegisterVmEvent::Statement(StatementValue::Commit))
        );
    }
}
