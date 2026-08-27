//! The executable register-based HKS bytecode compiler, VM and task scheduler.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    HirArena, MirConstant, MirInstruction, MirTerminator, Program, Register, RegisterFrame,
    ResolvedFunction, Span, StatementValue, SymbolId, SymbolManifest, allocate_registers,
    lower_hir_to_mir, lower_to_hir,
    runtime::{BuiltinManifest, CallArgument, Value},
};

pub const REGISTER_BYTECODE_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSlice {
    pub start: Register,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterBytecode {
    pub version: u16,
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub symbols: SymbolManifest,
    pub globals: Vec<SymbolId>,
    pub locals: Vec<SymbolId>,
    pub local_count: u32,
    pub register_count: u16,
    pub instructions: Vec<RegisterInstruction>,
    pub functions: Vec<RegisterFunction>,
    pub regions: Vec<RegisterRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterFunction {
    pub name: SymbolId,
    pub exported: bool,
    pub parameters: Vec<u32>,
    pub register_count: u16,
    pub instructions: Vec<RegisterInstruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterRegion {
    pub register_count: u16,
    pub instructions: Vec<RegisterInstruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RegisterInstruction {
    Constant {
        dst: Register,
        value: RegisterConstant,
    },
    Move {
        dst: Register,
        src: Register,
    },
    MakeClosure {
        dst: Register,
        region: u32,
        statements: Vec<u32>,
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
        values: RegisterSlice,
    },
    MakeList {
        dst: Register,
        values: RegisterSlice,
    },
    MakeMap {
        dst: Register,
        names: Vec<SymbolId>,
        values: RegisterSlice,
    },
    /// A symbolic call. Runtime linking decides whether the target is script
    /// bytecode or a native implementation.
    Call {
        dst: Register,
        function: SymbolId,
        receiver: Option<Register>,
        labels: Vec<Option<SymbolId>>,
        arguments: RegisterSlice,
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
        string: bool,
    },
    Jump(usize),
    Branch {
        condition: Register,
        then_target: usize,
        else_target: usize,
    },
    Return(Option<Register>),
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

impl RegisterCompileError {
    pub fn diagnostic(&self, source: crate::SourceId) -> crate::Diagnostic {
        let mut diagnostic = crate::Diagnostic::error(&self.message).with_code("HKS-COMPILE");
        if let Some(span) = self.span {
            diagnostic =
                diagnostic.with_label(crate::DiagnosticLabel::primary(source, span.range()));
        }
        if self.message.starts_with("condition expects Bool") {
            diagnostic =
                diagnostic.with_help("use a comparison such as `value < limit` to produce a Bool");
        }
        diagnostic
    }
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
    let mir = lower_hir_to_mir(&hir).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| RegisterCompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    let mut symbols =
        crate::SymbolInterner::from_manifest(hir.symbols.clone()).map_err(|error| {
            vec![RegisterCompileError {
                message: format!("invalid symbol manifest: {error}"),
                span: None,
            }]
        })?;
    let function_symbols = hir
        .functions
        .iter()
        .map(|function| function.name)
        .collect::<Vec<_>>();
    let mut regions = Vec::new();
    let entry = compile_register_code(
        &mir.entry,
        manifest,
        &function_symbols,
        &mut symbols,
        &mut regions,
    )?;
    let mut functions = Vec::with_capacity(mir.functions.len());
    for (mir_function, hir_function) in mir.functions.iter().zip(hir.functions) {
        let code = compile_register_code(
            mir_function,
            manifest,
            &function_symbols,
            &mut symbols,
            &mut regions,
        )?;
        functions.push(RegisterFunction {
            name: hir_function.name,
            exported: hir_function.exported,
            parameters: hir_function
                .parameters
                .iter()
                .map(|local| local.0)
                .collect(),
            register_count: code.register_count,
            instructions: code.instructions,
        });
    }
    Ok(RegisterBytecode {
        version: REGISTER_BYTECODE_VERSION,
        source_hash,
        builtin_manifest_hash: manifest.hash(),
        symbols: symbols.manifest(),
        globals: hir.globals.iter().map(|global| global.name).collect(),
        locals: hir.locals.iter().map(|local| local.name).collect(),
        local_count: hir.locals.len() as u32,
        register_count: entry.register_count,
        instructions: entry.instructions,
        functions,
        regions,
    })
}

fn compile_register_code(
    function: &crate::MirFunction,
    manifest: &BuiltinManifest,
    function_symbols: &[SymbolId],
    symbols: &mut crate::SymbolInterner,
    regions: &mut Vec<RegisterRegion>,
) -> Result<RegisterRegion, Vec<RegisterCompileError>> {
    let mut region_ids = Vec::with_capacity(function.regions.len());
    for region in &function.regions {
        let compiled = compile_register_code(region, manifest, function_symbols, symbols, regions)?;
        let id = regions.len() as u32;
        regions.push(compiled);
        region_ids.push(id);
    }
    let allocation = allocate_registers(function).map_err(|error| {
        vec![RegisterCompileError {
            message: format!("register allocation failed: {error:?}"),
            span: None,
        }]
    })?;
    let (instructions, register_count) = emit_function(
        function,
        &allocation,
        manifest,
        function_symbols,
        &region_ids,
        symbols,
    )?;
    Ok(RegisterRegion {
        register_count,
        instructions,
    })
}

fn emit_function(
    function: &crate::MirFunction,
    allocation: &crate::RegisterAllocation,
    manifest: &BuiltinManifest,
    function_symbols: &[SymbolId],
    region_ids: &[u32],
    symbols: &mut crate::SymbolInterner,
) -> Result<(Vec<RegisterInstruction>, u16), Vec<RegisterCompileError>> {
    let register = |virtual_register| {
        allocation
            .register_for(virtual_register)
            .expect("MIR virtual register was allocated")
    };
    let mut blocks = Vec::with_capacity(function.blocks.len());
    let mut max_window = 0usize;
    for block in &function.blocks {
        let mut emitted = Vec::new();
        for instruction in &block.instructions {
            let scalar = match instruction {
                MirInstruction::MakeClosure {
                    dst,
                    region,
                    statements,
                } => RegisterInstruction::MakeClosure {
                    dst: register(*dst),
                    region: *region_ids.get(*region as usize).ok_or_else(|| {
                        vec![RegisterCompileError {
                            message: format!("unknown closure region {region}"),
                            span: None,
                        }]
                    })?,
                    statements: statements
                        .iter()
                        .map(|statement| {
                            region_ids.get(*statement as usize).copied().ok_or_else(|| {
                                vec![RegisterCompileError {
                                    message: format!("unknown statement region {statement}"),
                                    span: None,
                                }]
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                },
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
                MirInstruction::MakeTuple { dst, values } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        values.iter().map(|value| register(*value)),
                    )?;
                    max_window = max_window.max(values.len());
                    RegisterInstruction::MakeTuple {
                        dst: register(*dst),
                        values: slice,
                    }
                }
                MirInstruction::MakeList { dst, values } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        values.iter().map(|value| register(*value)),
                    )?;
                    max_window = max_window.max(values.len());
                    RegisterInstruction::MakeList {
                        dst: register(*dst),
                        values: slice,
                    }
                }
                MirInstruction::MakeMap { dst, fields } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        fields.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(fields.len());
                    RegisterInstruction::MakeMap {
                        dst: register(*dst),
                        names: fields.iter().map(|(name, _)| *name).collect(),
                        values: slice,
                    }
                }
                MirInstruction::Call {
                    dst,
                    function: ResolvedFunction::Builtin(builtin),
                    receiver,
                    dynamic_callee: None,
                    arguments,
                } => {
                    let name = manifest.callable_name(*builtin).ok_or_else(|| {
                        vec![RegisterCompileError {
                            message: format!("native callable {:?} has no public symbol", builtin),
                            span: None,
                        }]
                    })?;
                    let function = symbols.intern(name);
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        arguments.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(arguments.len());
                    RegisterInstruction::Call {
                        dst: register(*dst),
                        function,
                        receiver: receiver.map(register),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    dst,
                    function: ResolvedFunction::External(function),
                    receiver,
                    dynamic_callee: None,
                    arguments,
                } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        arguments.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(arguments.len());
                    RegisterInstruction::Call {
                        dst: register(*dst),
                        function: *function,
                        receiver: receiver.map(register),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    dst,
                    function: ResolvedFunction::User(function),
                    receiver: None,
                    dynamic_callee: None,
                    arguments,
                } => {
                    let function = *function_symbols.get(function.0 as usize).ok_or_else(|| {
                        vec![RegisterCompileError {
                            message: format!("unknown script function {:?}", function),
                            span: None,
                        }]
                    })?;
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        arguments.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(arguments.len());
                    RegisterInstruction::Call {
                        dst: register(*dst),
                        function,
                        receiver: None,
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
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
                MirInstruction::Statement { value, string } => RegisterInstruction::Statement {
                    value: register(*value),
                    string: *string,
                },
            };
            emitted.push(scalar);
        }
        blocks.push((emitted, block.terminator.clone()));
    }
    let mut starts = Vec::with_capacity(blocks.len());
    let mut offset = 0usize;
    for (instructions, _) in &blocks {
        starts.push(offset);
        offset += instructions.len() + 1;
    }
    let mut output = Vec::with_capacity(offset);
    for (mut instructions, terminator) in blocks {
        output.append(&mut instructions);
        output.push(match terminator {
            MirTerminator::Jump(target) => RegisterInstruction::Jump(starts[target.0 as usize]),
            MirTerminator::Branch {
                condition,
                then_block,
                else_block,
            } => RegisterInstruction::Branch {
                condition: register(condition),
                then_target: starts[then_block.0 as usize],
                else_target: starts[else_block.0 as usize],
            },
            MirTerminator::Return(value) => RegisterInstruction::Return(value.map(register)),
            MirTerminator::Halt => RegisterInstruction::Halt,
            MirTerminator::Unset => {
                return Err(vec![RegisterCompileError {
                    message: "MIR block has no terminator".into(),
                    span: None,
                }]);
            }
        });
    }
    let register_count = usize::from(allocation.register_count)
        .checked_add(max_window)
        .and_then(|count| u16::try_from(count).ok())
        .ok_or_else(|| {
            vec![RegisterCompileError {
                message: "register frame exceeds u16 capacity".into(),
                span: None,
            }]
        })?;
    Ok((output, register_count))
}

fn emit_register_window(
    output: &mut Vec<RegisterInstruction>,
    start: u16,
    values: impl IntoIterator<Item = Register>,
) -> Result<RegisterSlice, Vec<RegisterCompileError>> {
    let values = values.into_iter().collect::<Vec<_>>();
    for (offset, src) in values.iter().copied().enumerate() {
        let offset = u16::try_from(offset).map_err(|_| {
            vec![RegisterCompileError {
                message: "argument window exceeds u16 capacity".into(),
                span: None,
            }]
        })?;
        let dst = Register(start.checked_add(offset).ok_or_else(|| {
            vec![RegisterCompileError {
                message: "argument window exceeds u16 capacity".into(),
                span: None,
            }]
        })?);
        output.push(RegisterInstruction::Move { dst, src });
    }
    Ok(RegisterSlice {
        start: Register(start),
        count: values.len() as u32,
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegisterCodeLocation {
    Entry,
    Function(u32),
    Region(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegisterVmEvent {
    Call(SymbolCall),
    Statement(StatementValue),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolCall {
    pub function: SymbolId,
    pub receiver: Option<Value>,
    pub arguments: Vec<CallArgument>,
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
    pub location: RegisterCodeLocation,
    pub call_stack: Vec<RegisterCallFrameSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterCallFrameSnapshot {
    pub location: RegisterCodeLocation,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Value>,
    pub destination: Register,
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
    location: RegisterCodeLocation,
    call_stack: Vec<RegisterCallFrameSnapshot>,
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
            location: RegisterCodeLocation::Entry,
            call_stack: Vec::new(),
        })
    }

    pub fn from_closure(
        bytecode: RegisterBytecode,
        closure: &Value,
    ) -> Result<Self, RegisterVmError> {
        let Value::RegisterClosure {
            region, captures, ..
        } = closure
        else {
            return Err(RegisterVmError::TypeMismatch("expected Function"));
        };
        let code = bytecode
            .regions
            .get(*region as usize)
            .ok_or(RegisterVmError::UnknownRegion(*region))?;
        if captures.len() != bytecode.local_count as usize {
            return Err(RegisterVmError::FrameShapeMismatch);
        }
        Ok(Self {
            registers: RegisterFrame::new(code.register_count),
            locals: captures.clone().into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: RegisterVmStatus::Ready,
            location: RegisterCodeLocation::Region(*region),
            call_stack: Vec::new(),
        })
    }

    pub fn from_function(
        bytecode: RegisterBytecode,
        function: u32,
        arguments: Vec<Value>,
    ) -> Result<Self, RegisterVmError> {
        let metadata = bytecode
            .functions
            .get(function as usize)
            .ok_or(RegisterVmError::UnknownFunction(function))?;
        if metadata.parameters.len() != arguments.len() {
            return Err(RegisterVmError::FunctionArity {
                expected: metadata.parameters.len(),
                actual: arguments.len(),
            });
        }
        let register_count = metadata.register_count;
        let parameters = metadata.parameters.clone();
        let mut vm = Self {
            registers: RegisterFrame::new(register_count),
            locals: vec![Value::Uninitialized; bytecode.local_count as usize].into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: RegisterVmStatus::Ready,
            location: RegisterCodeLocation::Function(function),
            call_stack: Vec::new(),
        };
        for (local, value) in parameters.into_iter().zip(arguments) {
            *vm.local_mut(local)? = value;
        }
        Ok(vm)
    }

    pub fn step(&mut self) -> Result<Option<RegisterVmEvent>, RegisterVmError> {
        if self.status != RegisterVmStatus::Ready {
            return Ok(None);
        }
        loop {
            let instruction = self
                .current_instructions()
                .get(self.pc)
                .cloned()
                .ok_or(RegisterVmError::InvalidProgramCounter(self.pc))?;
            self.pc += 1;
            match instruction {
                RegisterInstruction::Constant { dst, value } => {
                    let value = self.constant_value(value)?;
                    self.write(dst, value)?;
                }
                RegisterInstruction::Move { dst, src } => {
                    let value = self.read(src)?.clone();
                    self.write(dst, value)?;
                }
                RegisterInstruction::MakeClosure {
                    dst,
                    region,
                    statements,
                } => {
                    if self.bytecode.regions.get(region as usize).is_none() {
                        return Err(RegisterVmError::UnknownRegion(region));
                    }
                    self.write(
                        dst,
                        Value::RegisterClosure {
                            region,
                            statements,
                            captures: self.locals.to_vec(),
                        },
                    )?;
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
                    let values = self.read_slice(values)?;
                    self.write(dst, Value::Tuple(values))?;
                }
                RegisterInstruction::MakeList { dst, values } => {
                    let values = self.read_slice(values)?;
                    self.write(dst, Value::List(values))?;
                }
                RegisterInstruction::MakeMap { dst, names, values } => {
                    let values = self.read_slice(values)?;
                    let fields = names
                        .into_iter()
                        .zip(values)
                        .map(|(name, value)| Ok((self.symbol(name)?.to_string(), value)))
                        .collect::<Result<BTreeMap<_, _>, RegisterVmError>>()?;
                    self.write(dst, Value::Map(fields))?;
                }
                RegisterInstruction::Call {
                    dst,
                    function,
                    receiver,
                    labels,
                    arguments,
                } => {
                    let receiver = receiver
                        .map(|receiver| self.read(receiver).cloned())
                        .transpose()?;
                    let values = self.read_slice(arguments)?;
                    let arguments = labels
                        .into_iter()
                        .zip(values)
                        .map(|(label, value)| {
                            Ok(CallArgument {
                                label: label
                                    .map(|label| self.symbol(label).map(str::to_string))
                                    .transpose()?,
                                value,
                            })
                        })
                        .collect::<Result<Vec<_>, RegisterVmError>>()?;
                    if receiver.is_none()
                        && let Some(function_index) = self
                            .bytecode
                            .functions
                            .iter()
                            .position(|candidate| candidate.name == function)
                    {
                        let values = arguments
                            .iter()
                            .map(|argument| argument.value.clone())
                            .collect::<Vec<_>>();
                        self.call_script(function_index, dst, values)?;
                        continue;
                    }
                    self.status = RegisterVmStatus::WaitingForHost;
                    self.waiting_destination = Some(dst);
                    return Ok(Some(RegisterVmEvent::Call(SymbolCall {
                        function,
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
                RegisterInstruction::Statement { value, string } => {
                    let value = self.read(value)?;
                    let statement = match (string, value) {
                        (true, Value::String(value)) => StatementValue::String(value.clone()),
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
                RegisterInstruction::Return(value) => {
                    let value = value
                        .map(|register| self.read(register).cloned())
                        .transpose()?
                        .unwrap_or(Value::Null);
                    if self.call_stack.is_empty() {
                        self.status = RegisterVmStatus::Completed;
                        return Ok(Some(RegisterVmEvent::Completed(value)));
                    }
                    self.return_from_script(value)?;
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
            location: self.location,
            call_stack: self.call_stack.clone(),
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
        let register_count = match snapshot.location {
            RegisterCodeLocation::Entry => bytecode.register_count,
            RegisterCodeLocation::Function(function) => bytecode
                .functions
                .get(function as usize)
                .map(|function| function.register_count)
                .ok_or(RegisterVmError::UnknownFunction(function))?,
            RegisterCodeLocation::Region(region) => bytecode
                .regions
                .get(region as usize)
                .map(|region| region.register_count)
                .ok_or(RegisterVmError::UnknownRegion(region))?,
        };
        if snapshot.registers.len() != usize::from(register_count)
            || snapshot.locals.len() != bytecode.local_count as usize
            || snapshot.globals.len() != bytecode.globals.len()
        {
            return Err(RegisterVmError::FrameShapeMismatch);
        }
        let mut registers = RegisterFrame::new(register_count);
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
            location: snapshot.location,
            call_stack: snapshot.call_stack,
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

    pub fn globals(&self) -> &[Value] {
        &self.globals
    }

    pub fn set_global_values(&mut self, values: Vec<Value>) -> Result<(), RegisterVmError> {
        if values.len() != self.bytecode.globals.len() {
            return Err(RegisterVmError::FrameShapeMismatch);
        }
        self.globals = values.into_boxed_slice();
        Ok(())
    }

    pub fn eval_template(&self, template: &str) -> Result<String, crate::TemplateError> {
        let mut context = BTreeMap::new();
        for (symbol, value) in self.bytecode.globals.iter().zip(self.globals.iter()) {
            if value != &Value::Uninitialized
                && let Some(name) = self.bytecode.symbols.resolve(*symbol)
            {
                context.insert(name.to_string(), value.clone());
            }
        }
        for (symbol, value) in self.bytecode.locals.iter().zip(self.locals.iter()) {
            if value != &Value::Uninitialized
                && let Some(name) = self.bytecode.symbols.resolve(*symbol)
            {
                context.insert(name.to_string(), value.clone());
            }
        }
        crate::eval_template(template, &mut context)
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

    fn current_instructions(&self) -> &[RegisterInstruction] {
        match self.location {
            RegisterCodeLocation::Entry => &self.bytecode.instructions,
            RegisterCodeLocation::Function(function) => {
                &self.bytecode.functions[function as usize].instructions
            }
            RegisterCodeLocation::Region(region) => {
                &self.bytecode.regions[region as usize].instructions
            }
        }
    }

    fn call_script(
        &mut self,
        function_index: usize,
        destination: Register,
        arguments: Vec<Value>,
    ) -> Result<(), RegisterVmError> {
        let function = self
            .bytecode
            .functions
            .get(function_index)
            .ok_or(RegisterVmError::UnknownFunction(function_index as u32))?;
        if function.parameters.len() != arguments.len() {
            return Err(RegisterVmError::FunctionArity {
                expected: function.parameters.len(),
                actual: arguments.len(),
            });
        }
        self.call_stack.push(RegisterCallFrameSnapshot {
            location: self.location,
            pc: self.pc,
            registers: self.registers.values().to_vec(),
            locals: self.locals.to_vec(),
            destination,
        });
        let register_count = function.register_count;
        let parameters = function.parameters.clone();
        self.location = RegisterCodeLocation::Function(function_index as u32);
        self.pc = 0;
        self.registers = RegisterFrame::new(register_count);
        self.locals =
            vec![Value::Uninitialized; self.bytecode.local_count as usize].into_boxed_slice();
        for (local, value) in parameters.into_iter().zip(arguments) {
            *self.local_mut(local)? = value;
        }
        Ok(())
    }

    fn return_from_script(&mut self, value: Value) -> Result<(), RegisterVmError> {
        let frame = self
            .call_stack
            .pop()
            .ok_or(RegisterVmError::ReturnOutsideFunction)?;
        self.location = frame.location;
        self.pc = frame.pc;
        let mut registers = RegisterFrame::new(frame.registers.len() as u16);
        for (index, value) in frame.registers.into_iter().enumerate() {
            registers
                .write(Register(index as u16), value)
                .map_err(|_| RegisterVmError::InvalidRegister(Register(index as u16)))?;
        }
        self.registers = registers;
        self.locals = frame.locals.into_boxed_slice();
        self.write(frame.destination, value)
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

    fn read_slice(&self, registers: RegisterSlice) -> Result<Vec<Value>, RegisterVmError> {
        (0..registers.count)
            .map(|offset| {
                let index = u32::from(registers.start.0)
                    .checked_add(offset)
                    .and_then(|index| u16::try_from(index).ok())
                    .ok_or(RegisterVmError::InvalidRegister(registers.start))?;
                self.read(Register(index)).cloned()
            })
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
    UnknownFunction(u32),
    UnknownRegion(u32),
    UnknownTask(u64),
    FunctionArity { expected: usize, actual: usize },
    ReturnOutsideFunction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegisterTaskMode {
    Sequence,
    Parallel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegisterTaskEvent {
    Call { task: u64, call: SymbolCall },
    Statement { task: u64, value: StatementValue },
    Completed { task: u64, value: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RegisterTaskSnapshot {
    vm: Option<RegisterVmSnapshot>,
    children: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterTaskSchedulerSnapshot {
    next_task: u64,
    tasks: BTreeMap<u64, RegisterTaskSnapshot>,
    globals: Vec<Value>,
}

pub struct RegisterTaskScheduler {
    bytecode: RegisterBytecode,
    next_task: u64,
    tasks: BTreeMap<u64, (Option<RegisterVm>, Vec<u64>)>,
    globals: Vec<Value>,
}

impl RegisterTaskScheduler {
    pub fn new(bytecode: RegisterBytecode) -> Self {
        Self {
            globals: vec![Value::Uninitialized; bytecode.globals.len()],
            bytecode,
            next_task: 1,
            tasks: BTreeMap::new(),
        }
    }

    pub fn set_global_values(&mut self, globals: Vec<Value>) -> Result<(), RegisterVmError> {
        if globals.len() != self.bytecode.globals.len() {
            return Err(RegisterVmError::FrameShapeMismatch);
        }
        self.globals = globals;
        Ok(())
    }

    pub fn global_values(&self) -> &[Value] {
        &self.globals
    }

    pub fn spawn_closure(
        &mut self,
        closure: &Value,
        mode: RegisterTaskMode,
    ) -> Result<u64, RegisterVmError> {
        let Value::RegisterClosure {
            region,
            statements,
            captures,
        } = closure
        else {
            return Err(RegisterVmError::TypeMismatch("expected Function"));
        };
        match mode {
            RegisterTaskMode::Sequence => self.spawn_region(*region, captures.clone()),
            RegisterTaskMode::Parallel => {
                let parent = self.allocate_task();
                let mut children = Vec::with_capacity(statements.len());
                for region in statements {
                    children.push(self.spawn_region(*region, captures.clone())?);
                }
                self.tasks.insert(parent, (None, children));
                Ok(parent)
            }
        }
    }

    pub fn step(&mut self) -> Result<Option<RegisterTaskEvent>, RegisterVmError> {
        let ids = self.tasks.keys().copied().collect::<Vec<_>>();
        for task in ids {
            let completed_parent = self
                .tasks
                .get(&task)
                .filter(|(vm, _)| vm.is_none())
                .is_some_and(|(_, children)| {
                    children.iter().all(|child| !self.tasks.contains_key(child))
                });
            if completed_parent {
                self.tasks.remove(&task);
                return Ok(Some(RegisterTaskEvent::Completed {
                    task,
                    value: Value::Null,
                }));
            }
            let Some((vm, children)) = self.tasks.get_mut(&task) else {
                continue;
            };
            if let Some(vm) = vm {
                match vm.step()? {
                    Some(RegisterVmEvent::Call(call)) => {
                        return Ok(Some(RegisterTaskEvent::Call { task, call }));
                    }
                    Some(RegisterVmEvent::Statement(value)) => {
                        return Ok(Some(RegisterTaskEvent::Statement { task, value }));
                    }
                    Some(RegisterVmEvent::Completed(value)) => {
                        self.globals = vm.globals().to_vec();
                        self.tasks.remove(&task);
                        return Ok(Some(RegisterTaskEvent::Completed { task, value }));
                    }
                    None => {}
                }
            } else {
                let _ = children;
            }
        }
        Ok(None)
    }

    pub fn resume(&mut self, task: u64, value: Value) -> Result<(), RegisterVmError> {
        let vm = self
            .tasks
            .get_mut(&task)
            .and_then(|(vm, _)| vm.as_mut())
            .ok_or(RegisterVmError::UnknownTask(task))?;
        vm.resume(value)
    }

    pub fn eval_template(&self, task: u64, template: &str) -> Result<String, crate::TemplateError> {
        self.tasks
            .get(&task)
            .and_then(|(vm, _)| vm.as_ref())
            .ok_or_else(|| crate::TemplateError::UnknownPath(format!("task {task}")))?
            .eval_template(template)
    }

    pub fn snapshot(&self) -> RegisterTaskSchedulerSnapshot {
        RegisterTaskSchedulerSnapshot {
            next_task: self.next_task,
            globals: self.globals.clone(),
            tasks: self
                .tasks
                .iter()
                .map(|(id, (vm, children))| {
                    (
                        *id,
                        RegisterTaskSnapshot {
                            vm: vm.as_ref().map(RegisterVm::snapshot),
                            children: children.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn restore(
        bytecode: RegisterBytecode,
        snapshot: RegisterTaskSchedulerSnapshot,
    ) -> Result<Self, RegisterVmError> {
        let tasks = snapshot
            .tasks
            .into_iter()
            .map(|(id, task)| {
                let vm = task
                    .vm
                    .map(|snapshot| RegisterVm::restore(bytecode.clone(), snapshot))
                    .transpose()?;
                Ok((id, (vm, task.children)))
            })
            .collect::<Result<_, RegisterVmError>>()?;
        Ok(Self {
            bytecode,
            next_task: snapshot.next_task,
            tasks,
            globals: snapshot.globals,
        })
    }

    fn allocate_task(&mut self) -> u64 {
        let task = self.next_task;
        self.next_task += 1;
        task
    }

    fn spawn_region(&mut self, region: u32, captures: Vec<Value>) -> Result<u64, RegisterVmError> {
        let task = self.allocate_task();
        let closure = Value::RegisterClosure {
            region,
            statements: Vec::new(),
            captures,
        };
        let mut vm = RegisterVm::from_closure(self.bytecode.clone(), &closure)?;
        vm.set_global_values(self.globals.clone())?;
        self.tasks.insert(task, (Some(vm), Vec::new()));
        Ok(task)
    }
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinId, parse_program};

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
        assert_eq!(bytecode.symbols.resolve(call.function), Some("nativeValue"));
        let linked = crate::link_register_bytecode(bytecode.clone(), &manifest)
            .expect("symbolic call must link");
        assert_eq!(
            linked.resolve(call.function),
            Some(crate::LinkedFunction::Native(builtin))
        );
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

    #[test]
    fn variadic_operands_are_packed_into_a_register_window() {
        let manifest = BuiltinManifest::new([("collect", BuiltinId(3))]);
        let bytecode = compile("collect(1, 2, 3)", &manifest);
        let call = bytecode
            .instructions
            .iter()
            .find_map(|instruction| match instruction {
                RegisterInstruction::Call { arguments, .. } => Some(*arguments),
                _ => None,
            })
            .expect("call instruction exists");
        assert_eq!(call.count, 3);
        assert!(bytecode.register_count >= call.start.0 + 3);
        assert_eq!(
            bytecode
                .instructions
                .iter()
                .filter(|instruction| matches!(instruction, RegisterInstruction::Move { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn script_call_frames_restore_across_native_yields() {
        let manifest = BuiltinManifest::new([("nativeValue", BuiltinId(8))]);
        let bytecode = compile(
            r#"
                fn increment(value) { nativeValue(value) + 1 }
                let result = increment(4)
            "#,
            &manifest,
        );
        let mut vm = RegisterVm::new(bytecode.clone()).expect("VM initializes");
        let Some(RegisterVmEvent::Call(call)) = vm.step().expect("native call yields") else {
            panic!("expected native call")
        };
        assert_eq!(bytecode.symbols.resolve(call.function), Some("nativeValue"));
        assert_eq!(call.arguments[0].value, Value::Number(4.0));
        let snapshot = vm.snapshot();
        assert_eq!(snapshot.call_stack.len(), 1);

        let mut restored = RegisterVm::restore(bytecode, snapshot).expect("call stack restores");
        restored
            .resume(Value::Number(4.0))
            .expect("native result resumes function");
        while !matches!(
            restored.step().expect("execution succeeds"),
            Some(RegisterVmEvent::Completed(_))
        ) {}
    }

    #[test]
    fn trailing_closure_compiles_to_a_captured_register_region() {
        let builtin = BuiltinId(9);
        let manifest = BuiltinManifest::new([("invoke", builtin)]).with_type_metadata(
            crate::SymbolManifest::default(),
            BTreeMap::from([(
                builtin,
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![crate::ScriptType::Function],
                    result: crate::ScriptType::Any,
                },
            )]),
            Vec::new(),
        );
        let bytecode = compile("let value = 4\ninvoke { value + 1 }", &manifest);
        assert_eq!(bytecode.regions.len(), 2);
        let mut vm = RegisterVm::new(bytecode).expect("VM initializes");
        let Some(RegisterVmEvent::Statement(_)) = vm.step().expect("let executes") else {
            panic!("expected let statement")
        };
        let Some(RegisterVmEvent::Call(call)) = vm.step().expect("invoke yields") else {
            panic!("expected invoke call")
        };
        assert!(matches!(
            call.arguments[0].value,
            Value::RegisterClosure { region: 0, ref statements, ref captures }
                if statements == &[1] && captures.contains(&Value::Number(4.0))
        ));
    }

    #[test]
    fn register_task_scheduler_restores_parallel_closure_children() {
        let manifest = BuiltinManifest::new([
            ("par", BuiltinId(1)),
            ("first", BuiltinId(2)),
            ("second", BuiltinId(3)),
        ]);
        let bytecode = compile("par { first(); second() }", &manifest);
        let mut main = RegisterVm::new(bytecode.clone()).expect("VM initializes");
        let Some(RegisterVmEvent::Call(call)) = main.step().expect("par yields") else {
            panic!("expected par call")
        };
        let closure = call.arguments[0].value.clone();
        let mut scheduler = RegisterTaskScheduler::new(bytecode.clone());
        let parent = scheduler
            .spawn_closure(&closure, RegisterTaskMode::Parallel)
            .expect("parallel closure spawns");
        let Some(RegisterTaskEvent::Call { task: first, call }) =
            scheduler.step().expect("first child yields")
        else {
            panic!("expected first child call")
        };
        assert_eq!(bytecode.symbols.resolve(call.function), Some("first"));

        let snapshot = scheduler.snapshot();
        let mut restored = RegisterTaskScheduler::restore(bytecode.clone(), snapshot)
            .expect("parallel scheduler restores");
        restored.resume(first, Value::Null).expect("first resumes");
        while !matches!(
            restored.step().expect("first completes"),
            Some(RegisterTaskEvent::Completed { task, .. }) if task == first
        ) {}
        let Some(RegisterTaskEvent::Call { task: second, call }) =
            restored.step().expect("second child yields")
        else {
            panic!("expected second child call")
        };
        assert_eq!(bytecode.symbols.resolve(call.function), Some("second"));
        restored
            .resume(second, Value::Null)
            .expect("second resumes");
        while !matches!(
            restored.step().expect("tasks advance"),
            Some(RegisterTaskEvent::Completed { task, .. }) if task == parent
        ) {}
    }
}
