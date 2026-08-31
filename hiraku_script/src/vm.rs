//! The executable register-based HKS bytecode compiler and VM.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    HirArena, MirConstant, MirInstruction, MirTerminator, Program, Register, RegisterFrame,
    ResolvedFunction, Span, StatementValue, SymbolId, SymbolManifest, allocate_registers,
    lower_hir_to_mir, lower_to_hir,
    runtime::{BuiltinManifest, CallArgument, Value},
};

pub const BYTECODE_VERSION: u16 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSlice {
    pub start: Register,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub version: u16,
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub symbols: SymbolManifest,
    pub globals: Vec<SymbolId>,
    pub locals: Vec<SymbolId>,
    pub local_count: u32,
    pub register_count: u16,
    pub instructions: Vec<Instruction>,
    pub functions: Vec<BytecodeFunction>,
    pub regions: Vec<BytecodeRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BytecodeFunction {
    pub name: SymbolId,
    pub exported: bool,
    pub parameters: Vec<u32>,
    pub register_count: u16,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BytecodeRegion {
    pub parameters: Vec<u32>,
    pub register_count: u16,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    Constant {
        dst: Register,
        value: Constant,
    },
    Move {
        dst: Register,
        src: Register,
    },
    MakeClosure {
        dst: Register,
        region: u32,
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
    CallValue {
        dst: Register,
        callee: Register,
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
        emit_value: bool,
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
pub enum Constant {
    Null,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(SymbolId),
    Selector(SymbolId),
    Function(SymbolId),
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub span: Option<Span>,
}

impl CompileError {
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

pub fn compile_with_manifest(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
) -> Result<Bytecode, Vec<CompileError>> {
    let arena = HirArena::new();
    let hir = lower_to_hir(&arena, program, Some(manifest)).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| CompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    let mir = lower_hir_to_mir(&hir).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| CompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    let mut symbols =
        crate::SymbolInterner::from_manifest(hir.symbols.clone()).map_err(|error| {
            vec![CompileError {
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
        functions.push(BytecodeFunction {
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
    Ok(Bytecode {
        version: BYTECODE_VERSION,
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
    regions: &mut Vec<BytecodeRegion>,
) -> Result<BytecodeRegion, Vec<CompileError>> {
    let mut region_ids = Vec::with_capacity(function.regions.len());
    for region in &function.regions {
        let compiled = compile_register_code(region, manifest, function_symbols, symbols, regions)?;
        let id = regions.len() as u32;
        regions.push(compiled);
        region_ids.push(id);
    }
    let allocation = allocate_registers(function).map_err(|error| {
        vec![CompileError {
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
    Ok(BytecodeRegion {
        parameters: function
            .parameters
            .iter()
            .map(|parameter| parameter.0)
            .collect(),
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
) -> Result<(Vec<Instruction>, u16), Vec<CompileError>> {
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
                MirInstruction::MakeClosure { dst, region } => Instruction::MakeClosure {
                    dst: register(*dst),
                    region: *region_ids.get(*region as usize).ok_or_else(|| {
                        vec![CompileError {
                            message: format!("unknown closure region {region}"),
                            span: None,
                        }]
                    })?,
                },
                MirInstruction::Constant { dst, value } => {
                    let value = match value {
                        MirConstant::Function(ResolvedFunction::Builtin(builtin)) => {
                            let name = manifest.callable_name(*builtin).ok_or_else(|| {
                                vec![CompileError {
                                    message: format!(
                                        "native callable {builtin:?} has no public symbol"
                                    ),
                                    span: None,
                                }]
                            })?;
                            Constant::Function(symbols.intern(name))
                        }
                        MirConstant::Function(ResolvedFunction::User(function)) => {
                            Constant::Function(
                                *function_symbols.get(function.0 as usize).ok_or_else(|| {
                                    vec![CompileError {
                                        message: format!("unknown script function {function:?}"),
                                        span: None,
                                    }]
                                })?,
                            )
                        }
                        MirConstant::Function(ResolvedFunction::External(symbol)) => {
                            Constant::Function(*symbol)
                        }
                        MirConstant::Function(ResolvedFunction::Dynamic) => {
                            return Err(vec![CompileError {
                                message: "a dynamic function cannot be used as a constant".into(),
                                span: None,
                            }]);
                        }
                        value => constant(value.clone()),
                    };
                    Instruction::Constant {
                        dst: register(*dst),
                        value,
                    }
                }
                MirInstruction::LoadLocal { dst, local } => Instruction::LoadLocal {
                    dst: register(*dst),
                    local: local.0,
                },
                MirInstruction::StoreLocal { local, src } => Instruction::StoreLocal {
                    local: local.0,
                    src: register(*src),
                },
                MirInstruction::LoadGlobal { dst, global } => Instruction::LoadGlobal {
                    dst: register(*dst),
                    global: global.0,
                },
                MirInstruction::StoreGlobal { global, src } => Instruction::StoreGlobal {
                    global: global.0,
                    src: register(*src),
                },
                MirInstruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                } => Instruction::GetMember {
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
                } => Instruction::SetMember {
                    dst: register(*dst),
                    object: register(*object),
                    member: *member,
                    value: register(*value),
                },
                MirInstruction::UnaryMinus { dst, value } => Instruction::UnaryMinus {
                    dst: register(*dst),
                    value: register(*value),
                },
                MirInstruction::Binary {
                    dst,
                    op,
                    left,
                    right,
                } => Instruction::Binary {
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
                    Instruction::MakeTuple {
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
                    Instruction::MakeList {
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
                    Instruction::MakeMap {
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
                        vec![CompileError {
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
                    Instruction::Call {
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
                    Instruction::Call {
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
                        vec![CompileError {
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
                    Instruction::Call {
                        dst: register(*dst),
                        function,
                        receiver: None,
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    dst,
                    function: ResolvedFunction::Dynamic,
                    receiver: None,
                    dynamic_callee: Some(callee),
                    arguments,
                } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        arguments.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(arguments.len());
                    Instruction::CallValue {
                        dst: register(*dst),
                        callee: register(*callee),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call { .. } => {
                    return Err(vec![CompileError {
                        message: "dynamic and user calls are not implemented in register bytecode"
                            .into(),
                        span: None,
                    }]);
                }
                MirInstruction::AssertNonNull { dst, value } => Instruction::AssertNonNull {
                    dst: register(*dst),
                    value: register(*value),
                },
                MirInstruction::SelectNonNull {
                    dst,
                    value,
                    fallback,
                } => Instruction::SelectNonNull {
                    dst: register(*dst),
                    value: register(*value),
                    fallback: register(*fallback),
                },
                MirInstruction::Statement {
                    value,
                    string,
                    emit_value,
                } => Instruction::Statement {
                    value: register(*value),
                    string: *string,
                    emit_value: *emit_value,
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
            MirTerminator::Jump(target) => Instruction::Jump(starts[target.0 as usize]),
            MirTerminator::Branch {
                condition,
                then_block,
                else_block,
            } => Instruction::Branch {
                condition: register(condition),
                then_target: starts[then_block.0 as usize],
                else_target: starts[else_block.0 as usize],
            },
            MirTerminator::Return(value) => Instruction::Return(value.map(register)),
            MirTerminator::Halt => Instruction::Halt,
            MirTerminator::Unset => {
                return Err(vec![CompileError {
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
            vec![CompileError {
                message: "register frame exceeds u16 capacity".into(),
                span: None,
            }]
        })?;
    Ok((output, register_count))
}

fn emit_register_window(
    output: &mut Vec<Instruction>,
    start: u16,
    values: impl IntoIterator<Item = Register>,
) -> Result<RegisterSlice, Vec<CompileError>> {
    let values = values.into_iter().collect::<Vec<_>>();
    for (offset, src) in values.iter().copied().enumerate() {
        let offset = u16::try_from(offset).map_err(|_| {
            vec![CompileError {
                message: "argument window exceeds u16 capacity".into(),
                span: None,
            }]
        })?;
        let dst = Register(start.checked_add(offset).ok_or_else(|| {
            vec![CompileError {
                message: "argument window exceeds u16 capacity".into(),
                span: None,
            }]
        })?);
        output.push(Instruction::Move { dst, src });
    }
    Ok(RegisterSlice {
        start: Register(start),
        count: values.len() as u32,
    })
}

fn constant(value: MirConstant) -> Constant {
    match value {
        MirConstant::Null => Constant::Null,
        MirConstant::Unit => Constant::Unit,
        MirConstant::Ellipsis => Constant::Ellipsis,
        MirConstant::Bool(value) => Constant::Bool(value),
        MirConstant::Number(value) => Constant::Number(value),
        MirConstant::Percent(value) => Constant::Percent(value),
        MirConstant::String(value) => Constant::String(value),
        MirConstant::Symbol(value) => Constant::Symbol(value),
        MirConstant::Selector(value) => Constant::Selector(value),
        MirConstant::Function(_) => unreachable!("function constants require symbol resolution"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmStatus {
    Ready,
    WaitingForHost,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeLocation {
    Entry,
    Function(u32),
    Region(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VmEvent {
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
pub struct VmSnapshot {
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Value>,
    pub globals: Vec<Value>,
    pub waiting_destination: Option<Register>,
    pub status: VmStatus,
    pub location: CodeLocation,
    pub call_stack: Vec<CallFrameSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallFrameSnapshot {
    pub location: CodeLocation,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Value>,
    pub destination: Register,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vm {
    bytecode: Bytecode,
    pc: usize,
    registers: RegisterFrame,
    locals: Box<[Value]>,
    globals: Box<[Value]>,
    waiting_destination: Option<Register>,
    status: VmStatus,
    location: CodeLocation,
    call_stack: Vec<CallFrameSnapshot>,
}

impl Vm {
    pub fn new(bytecode: Bytecode) -> Result<Self, VmError> {
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        Ok(Self {
            registers: RegisterFrame::new(bytecode.register_count),
            locals: vec![Value::Uninitialized; bytecode.local_count as usize].into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: VmStatus::Ready,
            location: CodeLocation::Entry,
            call_stack: Vec::new(),
        })
    }

    pub fn from_closure(bytecode: Bytecode, closure: &Value) -> Result<Self, VmError> {
        let Value::Closure {
            region, captures, ..
        } = closure
        else {
            return Err(VmError::TypeMismatch("expected Function"));
        };
        let code = bytecode
            .regions
            .get(*region as usize)
            .ok_or(VmError::UnknownRegion(*region))?;
        if captures.len() != bytecode.local_count as usize {
            return Err(VmError::FrameShapeMismatch);
        }
        Ok(Self {
            registers: RegisterFrame::new(code.register_count),
            locals: captures.clone().into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: VmStatus::Ready,
            location: CodeLocation::Region(*region),
            call_stack: Vec::new(),
        })
    }

    /// Creates an independent VM invocation from a save-safe function value.
    pub fn from_callable(
        bytecode: Bytecode,
        callable: &Value,
        arguments: Vec<Value>,
    ) -> Result<Self, VmError> {
        match callable {
            Value::Closure { region, .. } => {
                let metadata = bytecode
                    .regions
                    .get(*region as usize)
                    .ok_or(VmError::UnknownRegion(*region))?;
                if metadata.parameters.len() != arguments.len() {
                    return Err(VmError::FunctionArity {
                        expected: metadata.parameters.len(),
                        actual: arguments.len(),
                    });
                }
                let parameters = metadata.parameters.clone();
                let mut vm = Self::from_closure(bytecode, callable)?;
                for (local, value) in parameters.into_iter().zip(arguments) {
                    *vm.local_mut(local)? = value;
                }
                Ok(vm)
            }
            Value::Function { symbol, .. } => {
                let index = bytecode
                    .functions
                    .iter()
                    .position(|function| function.name == *symbol)
                    .ok_or(VmError::UnknownSymbol(*symbol))?;
                Self::from_function(bytecode, index as u32, arguments)
            }
            _ => Err(VmError::TypeMismatch("expected Function")),
        }
    }

    pub fn from_function(
        bytecode: Bytecode,
        function: u32,
        arguments: Vec<Value>,
    ) -> Result<Self, VmError> {
        let metadata = bytecode
            .functions
            .get(function as usize)
            .ok_or(VmError::UnknownFunction(function))?;
        if metadata.parameters.len() != arguments.len() {
            return Err(VmError::FunctionArity {
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
            status: VmStatus::Ready,
            location: CodeLocation::Function(function),
            call_stack: Vec::new(),
        };
        for (local, value) in parameters.into_iter().zip(arguments) {
            *vm.local_mut(local)? = value;
        }
        Ok(vm)
    }

    pub fn step(&mut self) -> Result<Option<VmEvent>, VmError> {
        if self.status != VmStatus::Ready {
            return Ok(None);
        }
        loop {
            let instruction = self
                .current_instructions()
                .get(self.pc)
                .cloned()
                .ok_or(VmError::InvalidProgramCounter(self.pc))?;
            self.pc += 1;
            match instruction {
                Instruction::Constant { dst, value } => {
                    let value = self.constant_value(value)?;
                    self.write(dst, value)?;
                }
                Instruction::Move { dst, src } => {
                    let value = self.read(src)?.clone();
                    self.write(dst, value)?;
                }
                Instruction::MakeClosure { dst, region } => {
                    if self.bytecode.regions.get(region as usize).is_none() {
                        return Err(VmError::UnknownRegion(region));
                    }
                    self.write(
                        dst,
                        Value::Closure {
                            module: None,
                            region,
                            captures: self.locals.to_vec(),
                        },
                    )?;
                }
                Instruction::LoadLocal { dst, local } => {
                    let value = self.local(local)?.clone();
                    if value == Value::Uninitialized {
                        return Err(VmError::UninitializedLocal(local));
                    }
                    self.write(dst, value)?;
                }
                Instruction::StoreLocal { local, src } => {
                    let value = self.read(src)?.clone();
                    *self.local_mut(local)? = value;
                }
                Instruction::LoadGlobal { dst, global } => {
                    let value = self.global_slot(global)?.clone();
                    if value == Value::Uninitialized {
                        return Err(VmError::UninitializedGlobal(global));
                    }
                    self.write(dst, value)?;
                }
                Instruction::StoreGlobal { global, src } => {
                    let value = self.read(src)?.clone();
                    *self.global_mut(global)? = value;
                }
                Instruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                } => {
                    let name = self.symbol(member)?.to_string();
                    let value = get_member(self.read(object)?, &name, safe)?;
                    self.write(dst, value)?;
                }
                Instruction::SetMember {
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
                Instruction::UnaryMinus { dst, value } => {
                    let Value::Number(value) = self.read(value)? else {
                        return Err(VmError::TypeMismatch("unary minus expects Number"));
                    };
                    self.write(dst, Value::Number(-value))?;
                }
                Instruction::Binary {
                    dst,
                    op,
                    left,
                    right,
                } => {
                    let value = binary(op, self.read(left)?, self.read(right)?)?;
                    self.write(dst, value)?;
                }
                Instruction::MakeTuple { dst, values } => {
                    let values = self.read_slice(values)?;
                    self.write(dst, Value::Tuple(values))?;
                }
                Instruction::MakeList { dst, values } => {
                    let values = self.read_slice(values)?;
                    self.write(dst, Value::List(values))?;
                }
                Instruction::MakeMap { dst, names, values } => {
                    let values = self.read_slice(values)?;
                    let fields = names
                        .into_iter()
                        .zip(values)
                        .map(|(name, value)| Ok((self.symbol(name)?.to_string(), value)))
                        .collect::<Result<BTreeMap<_, _>, VmError>>()?;
                    self.write(dst, Value::Map(fields))?;
                }
                Instruction::Call {
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
                        .collect::<Result<Vec<_>, VmError>>()?;
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
                    self.status = VmStatus::WaitingForHost;
                    self.waiting_destination = Some(dst);
                    return Ok(Some(VmEvent::Call(SymbolCall {
                        function,
                        receiver,
                        arguments,
                    })));
                }
                Instruction::CallValue {
                    dst,
                    callee,
                    labels,
                    arguments,
                } => {
                    let callee = self.read(callee)?.clone();
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
                        .collect::<Result<Vec<_>, VmError>>()?;
                    match callee {
                        Value::Function {
                            module: _,
                            symbol: function,
                        } => {
                            if let Some(function_index) = self
                                .bytecode
                                .functions
                                .iter()
                                .position(|candidate| candidate.name == function)
                            {
                                self.call_script(
                                    function_index,
                                    dst,
                                    arguments
                                        .into_iter()
                                        .map(|argument| argument.value)
                                        .collect(),
                                )?;
                                continue;
                            }
                            self.status = VmStatus::WaitingForHost;
                            self.waiting_destination = Some(dst);
                            return Ok(Some(VmEvent::Call(SymbolCall {
                                function,
                                receiver: None,
                                arguments,
                            })));
                        }
                        Value::Closure {
                            region, captures, ..
                        } => {
                            let parameters = self
                                .bytecode
                                .regions
                                .get(region as usize)
                                .ok_or(VmError::UnknownRegion(region))?
                                .parameters
                                .clone();
                            if parameters.len() != arguments.len() {
                                return Err(VmError::FunctionArity {
                                    expected: parameters.len(),
                                    actual: arguments.len(),
                                });
                            }
                            self.call_closure(
                                region,
                                captures,
                                parameters,
                                arguments
                                    .into_iter()
                                    .map(|argument| argument.value)
                                    .collect(),
                                dst,
                            )?;
                            continue;
                        }
                        _ => return Err(VmError::TypeMismatch("callee expects Function")),
                    }
                }
                Instruction::AssertNonNull { dst, value } => {
                    let value = self.read(value)?.clone();
                    if value == Value::Null {
                        return Err(VmError::NullAssertion);
                    }
                    self.write(dst, value)?;
                }
                Instruction::SelectNonNull {
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
                Instruction::Statement {
                    value,
                    string,
                    emit_value,
                } => {
                    let value = self.read(value)?;
                    let statement = match (string, emit_value, value) {
                        (true, _, Value::String(value)) => StatementValue::String(value.clone()),
                        (_, true, Value::Unit) | (_, false, _) => StatementValue::Commit,
                        (_, true, _) => StatementValue::Value(value.clone()),
                    };
                    return Ok(Some(VmEvent::Statement(statement)));
                }
                Instruction::Jump(target) => self.pc = target,
                Instruction::Branch {
                    condition,
                    then_target,
                    else_target,
                } => {
                    let Value::Bool(condition) = self.read(condition)? else {
                        return Err(VmError::TypeMismatch("branch expects Bool"));
                    };
                    self.pc = if *condition { then_target } else { else_target };
                }
                Instruction::Return(value) => {
                    let value = value
                        .map(|register| self.read(register).cloned())
                        .transpose()?
                        .unwrap_or(Value::Unit);
                    if self.call_stack.is_empty() {
                        self.status = VmStatus::Completed;
                        return Ok(Some(VmEvent::Completed(value)));
                    }
                    self.return_from_script(value)?;
                }
                Instruction::Halt => {
                    self.status = VmStatus::Completed;
                    return Ok(Some(VmEvent::Completed(Value::Unit)));
                }
            }
        }
    }

    pub fn resume(&mut self, value: Value) -> Result<(), VmError> {
        if self.status != VmStatus::WaitingForHost {
            return Err(VmError::NotWaitingForHost);
        }
        let destination = self
            .waiting_destination
            .take()
            .ok_or(VmError::NotWaitingForHost)?;
        self.write(destination, value)?;
        self.status = VmStatus::Ready;
        Ok(())
    }

    pub fn snapshot(&self) -> VmSnapshot {
        VmSnapshot {
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

    pub fn restore(bytecode: Bytecode, snapshot: VmSnapshot) -> Result<Self, VmError> {
        if bytecode.source_hash != snapshot.source_hash {
            return Err(VmError::SourceHashMismatch);
        }
        if bytecode.builtin_manifest_hash != snapshot.builtin_manifest_hash {
            return Err(VmError::BuiltinManifestMismatch);
        }
        let register_count = match snapshot.location {
            CodeLocation::Entry => bytecode.register_count,
            CodeLocation::Function(function) => bytecode
                .functions
                .get(function as usize)
                .map(|function| function.register_count)
                .ok_or(VmError::UnknownFunction(function))?,
            CodeLocation::Region(region) => bytecode
                .regions
                .get(region as usize)
                .map(|region| region.register_count)
                .ok_or(VmError::UnknownRegion(region))?,
        };
        if snapshot.registers.len() != usize::from(register_count)
            || snapshot.locals.len() != bytecode.local_count as usize
            || snapshot.globals.len() != bytecode.globals.len()
        {
            return Err(VmError::FrameShapeMismatch);
        }
        let mut registers = RegisterFrame::new(register_count);
        for (index, value) in snapshot.registers.into_iter().enumerate() {
            registers
                .write(Register(index as u16), value)
                .map_err(|_| VmError::InvalidRegister(Register(index as u16)))?;
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

    pub fn status(&self) -> VmStatus {
        self.status
    }

    pub fn bytecode(&self) -> &Bytecode {
        &self.bytecode
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

    pub fn set_global_values(&mut self, values: Vec<Value>) -> Result<(), VmError> {
        if values.len() != self.bytecode.globals.len() {
            return Err(VmError::FrameShapeMismatch);
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

    fn constant_value(&self, value: Constant) -> Result<Value, VmError> {
        Ok(match value {
            Constant::Null => Value::Null,
            Constant::Unit => Value::Unit,
            Constant::Ellipsis => Value::Ellipsis,
            Constant::Bool(value) => Value::Bool(value),
            Constant::Number(value) => Value::Number(value),
            Constant::Percent(value) => Value::Percent(value),
            Constant::String(value) => Value::String(value),
            Constant::Symbol(symbol) => Value::Symbol(self.symbol(symbol)?.to_string()),
            Constant::Selector(symbol) => Value::Selector(self.symbol(symbol)?.to_string()),
            Constant::Function(symbol) => Value::Function {
                module: None,
                symbol,
            },
        })
    }

    fn symbol(&self, symbol: SymbolId) -> Result<&str, VmError> {
        self.bytecode
            .symbols
            .resolve(symbol)
            .ok_or(VmError::UnknownSymbol(symbol))
    }

    fn current_instructions(&self) -> &[Instruction] {
        match self.location {
            CodeLocation::Entry => &self.bytecode.instructions,
            CodeLocation::Function(function) => {
                &self.bytecode.functions[function as usize].instructions
            }
            CodeLocation::Region(region) => &self.bytecode.regions[region as usize].instructions,
        }
    }

    fn call_script(
        &mut self,
        function_index: usize,
        destination: Register,
        arguments: Vec<Value>,
    ) -> Result<(), VmError> {
        let function = self
            .bytecode
            .functions
            .get(function_index)
            .ok_or(VmError::UnknownFunction(function_index as u32))?;
        if function.parameters.len() != arguments.len() {
            return Err(VmError::FunctionArity {
                expected: function.parameters.len(),
                actual: arguments.len(),
            });
        }
        self.call_stack.push(CallFrameSnapshot {
            location: self.location,
            pc: self.pc,
            registers: self.registers.values().to_vec(),
            locals: self.locals.to_vec(),
            destination,
        });
        let register_count = function.register_count;
        let parameters = function.parameters.clone();
        self.location = CodeLocation::Function(function_index as u32);
        self.pc = 0;
        self.registers = RegisterFrame::new(register_count);
        self.locals =
            vec![Value::Uninitialized; self.bytecode.local_count as usize].into_boxed_slice();
        for (local, value) in parameters.into_iter().zip(arguments) {
            *self.local_mut(local)? = value;
        }
        Ok(())
    }

    fn call_closure(
        &mut self,
        region: u32,
        captures: Vec<Value>,
        parameters: Vec<u32>,
        arguments: Vec<Value>,
        destination: Register,
    ) -> Result<(), VmError> {
        let code = self
            .bytecode
            .regions
            .get(region as usize)
            .ok_or(VmError::UnknownRegion(region))?;
        if captures.len() != self.bytecode.local_count as usize {
            return Err(VmError::FrameShapeMismatch);
        }
        self.call_stack.push(CallFrameSnapshot {
            location: self.location,
            pc: self.pc,
            registers: self.registers.values().to_vec(),
            locals: self.locals.to_vec(),
            destination,
        });
        let register_count = code.register_count;
        self.location = CodeLocation::Region(region);
        self.pc = 0;
        self.registers = RegisterFrame::new(register_count);
        self.locals = captures.into_boxed_slice();
        for (local, value) in parameters.into_iter().zip(arguments) {
            *self.local_mut(local)? = value;
        }
        Ok(())
    }

    fn return_from_script(&mut self, value: Value) -> Result<(), VmError> {
        let frame = self
            .call_stack
            .pop()
            .ok_or(VmError::ReturnOutsideFunction)?;
        self.location = frame.location;
        self.pc = frame.pc;
        let mut registers = RegisterFrame::new(frame.registers.len() as u16);
        for (index, value) in frame.registers.into_iter().enumerate() {
            registers
                .write(Register(index as u16), value)
                .map_err(|_| VmError::InvalidRegister(Register(index as u16)))?;
        }
        self.registers = registers;
        self.locals = frame.locals.into_boxed_slice();
        self.write(frame.destination, value)
    }

    fn read(&self, register: Register) -> Result<&Value, VmError> {
        self.registers
            .read(register)
            .ok_or(VmError::InvalidRegister(register))
    }

    fn write(&mut self, register: Register, value: Value) -> Result<(), VmError> {
        self.registers
            .write(register, value)
            .map_err(|_| VmError::InvalidRegister(register))
    }

    fn read_slice(&self, registers: RegisterSlice) -> Result<Vec<Value>, VmError> {
        (0..registers.count)
            .map(|offset| {
                let index = u32::from(registers.start.0)
                    .checked_add(offset)
                    .and_then(|index| u16::try_from(index).ok())
                    .ok_or(VmError::InvalidRegister(registers.start))?;
                self.read(Register(index)).cloned()
            })
            .collect()
    }

    fn local(&self, local: u32) -> Result<&Value, VmError> {
        self.locals
            .get(local as usize)
            .ok_or(VmError::InvalidLocal(local))
    }

    fn local_mut(&mut self, local: u32) -> Result<&mut Value, VmError> {
        self.locals
            .get_mut(local as usize)
            .ok_or(VmError::InvalidLocal(local))
    }

    fn global_slot(&self, global: u32) -> Result<&Value, VmError> {
        self.globals
            .get(global as usize)
            .ok_or(VmError::InvalidGlobal(global))
    }

    fn global_mut(&mut self, global: u32) -> Result<&mut Value, VmError> {
        self.globals
            .get_mut(global as usize)
            .ok_or(VmError::InvalidGlobal(global))
    }
}

fn get_member(value: &Value, name: &str, safe: bool) -> Result<Value, VmError> {
    if value == &Value::Null && safe {
        return Ok(Value::Null);
    }
    match value {
        Value::Map(fields) => fields
            .get(name)
            .cloned()
            .ok_or_else(|| VmError::UnknownMember(name.to_string())),
        Value::Typed { value, .. } => get_member(value, name, safe),
        Value::Null => Err(VmError::NullMemberAccess(name.to_string())),
        _ => Err(VmError::TypeMismatch("member receiver is not a record")),
    }
}

fn set_member(value: &mut Value, name: &str, new_value: Value) -> Result<(), VmError> {
    let fields = match value {
        Value::Map(fields) => fields,
        Value::Typed { value, .. } => match value.as_mut() {
            Value::Map(fields) => fields,
            _ => {
                return Err(VmError::TypeMismatch("member receiver is not a record"));
            }
        },
        _ => {
            return Err(VmError::TypeMismatch("member receiver is not a record"));
        }
    };
    let field = fields
        .get_mut(name)
        .ok_or_else(|| VmError::UnknownMember(name.to_string()))?;
    *field = new_value;
    Ok(())
}

fn binary(op: crate::BinaryOp, left: &Value, right: &Value) -> Result<Value, VmError> {
    use crate::BinaryOp;
    match op {
        BinaryOp::Equal => Ok(Value::Bool(left == right)),
        BinaryOp::NotEqual => Ok(Value::Bool(left != right)),
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide => {
            let (Value::Number(left), Value::Number(right)) = (left, right) else {
                return Err(VmError::TypeMismatch("arithmetic expects Number operands"));
            };
            if op == BinaryOp::Divide && *right == 0.0 {
                return Err(VmError::DivisionByZero);
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
                return Err(VmError::TypeMismatch("comparison expects Number operands"));
            };
            Ok(Value::Bool(match op {
                BinaryOp::Less => left < right,
                BinaryOp::LessEqual => left <= right,
                BinaryOp::Greater => left > right,
                BinaryOp::GreaterEqual => left >= right,
                _ => unreachable!(),
            }))
        }
        BinaryOp::Colon => Err(VmError::TypeMismatch(
            "dialogue operator must resolve to a registered builtin",
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
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
    FunctionArity { expected: usize, actual: usize },
    ReturnOutsideFunction,
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinId, parse_program};

    use super::*;

    fn compile(source: &str, manifest: &BuiltinManifest) -> Bytecode {
        compile_with_manifest(&parse_program(source).expect("source parses"), 91, manifest)
            .expect("register bytecode compiles")
    }

    #[test]
    fn unit_literal_remains_distinct_from_null_through_snapshot_restore() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile("fn noop() -> Unit { () }\nnoop()", &manifest);
        assert!(
            bytecode.functions[0]
                .instructions
                .iter()
                .any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::Constant {
                            value: Constant::Unit,
                            ..
                        }
                    )
                })
        );

        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        assert_eq!(
            vm.step().expect("unit statement executes"),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).expect("unit snapshot restores");
        loop {
            if let Some(VmEvent::Completed(value)) = restored.step().expect("restored VM executes")
            {
                assert_eq!(value, Value::Unit);
                break;
            }
        }
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
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        loop {
            if matches!(vm.step().expect("VM executes"), Some(VmEvent::Completed(_))) {
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
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        let Some(VmEvent::Call(call)) = vm.step().expect("call yields") else {
            panic!("expected native call")
        };
        assert_eq!(bytecode.symbols.resolve(call.function), Some("nativeValue"));
        let linked =
            crate::link_bytecode(bytecode.clone(), &manifest).expect("symbolic call must link");
        assert_eq!(
            linked.resolve(call.function),
            Some(crate::LinkedFunction::Native(builtin))
        );
        let snapshot = vm.snapshot();
        let mut restored = Vm::restore(bytecode, snapshot).expect("waiting VM snapshot restores");
        restored
            .resume(Value::Number(4.0))
            .expect("host value resumes VM");
        assert_eq!(
            restored.step().expect("statement executes"),
            Some(VmEvent::Statement(StatementValue::Commit))
        );
        assert_eq!(
            restored.step().expect("assignment executes"),
            Some(VmEvent::Statement(StatementValue::Commit))
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
                Instruction::Call { arguments, .. } => Some(*arguments),
                _ => None,
            })
            .expect("call instruction exists");
        assert_eq!(call.count, 3);
        assert!(bytecode.register_count >= call.start.0 + 3);
        assert_eq!(
            bytecode
                .instructions
                .iter()
                .filter(|instruction| matches!(instruction, Instruction::Move { .. }))
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
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        let Some(VmEvent::Call(call)) = vm.step().expect("native call yields") else {
            panic!("expected native call")
        };
        assert_eq!(bytecode.symbols.resolve(call.function), Some("nativeValue"));
        assert_eq!(call.arguments[0].value, Value::Number(4.0));
        let snapshot = vm.snapshot();
        assert_eq!(snapshot.call_stack.len(), 1);

        let mut restored = Vm::restore(bytecode, snapshot).expect("call stack restores");
        restored
            .resume(Value::Number(4.0))
            .expect("native result resumes function");
        while !matches!(
            restored.step().expect("execution succeeds"),
            Some(VmEvent::Completed(_))
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
        assert_eq!(bytecode.regions.len(), 1);
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        let Some(VmEvent::Statement(_)) = vm.step().expect("let executes") else {
            panic!("expected let statement")
        };
        let Some(VmEvent::Call(call)) = vm.step().expect("invoke yields") else {
            panic!("expected invoke call")
        };
        assert!(matches!(
            call.arguments[0].value,
            Value::Closure {
                module: None,
                region: 0,
                ref captures,
            }
                if captures.contains(&Value::Number(4.0))
        ));
    }

    #[test]
    fn typed_lambda_parameters_execute_through_dynamic_calls() {
        let bytecode = compile(
            "let add = { left: Int, right: Int -> left + right }\nlet result = add(2, 3)",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        assert_eq!(bytecode.regions.len(), 1);
        assert_eq!(bytecode.regions[0].parameters.len(), 2);

        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("typed lambda executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert!(vm.snapshot().locals.contains(&Value::Number(5.0)));
    }

    #[test]
    fn embedding_reactive_parameters_capture_typed_expressions_as_closures() {
        let builtin = BuiltinId(10);
        let manifest = BuiltinManifest::new([("observe", builtin)]).with_type_metadata(
            crate::SymbolManifest::default(),
            BTreeMap::from([(
                builtin,
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![crate::ScriptType::Binding(Box::new(
                        crate::ScriptType::Bool,
                    ))],
                    result: crate::ScriptType::Unit,
                },
            )]),
            Vec::new(),
        );
        let bytecode = compile("let health = 2\nobserve(${health > 0})", &manifest);
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        assert!(matches!(
            vm.step().expect("local declaration executes"),
            Some(VmEvent::Statement(StatementValue::Commit))
        ));
        let Some(VmEvent::Call(call)) = vm.step().expect("reactive call yields") else {
            panic!("expected reactive native call")
        };
        let closure = call.arguments[0].value.clone();
        assert!(matches!(closure, Value::Closure { .. }));

        let mut binding = Vm::from_callable(bytecode, &closure, Vec::new())
            .expect("reactive closure should be independently callable");
        assert_eq!(
            binding.step().expect("binding expression evaluates"),
            Some(VmEvent::Statement(StatementValue::Value(Value::Bool(true))))
        );
        assert_eq!(
            binding.step().expect("binding expression returns"),
            Some(VmEvent::Completed(Value::Bool(true)))
        );
    }

    #[test]
    fn named_functions_are_first_class_and_dynamically_callable() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            "fn increment(value: Int) -> Int { value + 1 }\nlet callable = increment\nglobal result = callable(2)",
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("function value executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn named_functions_can_cross_the_native_call_boundary() {
        let builtin = BuiltinId(12);
        let manifest = BuiltinManifest::new([("schedule", builtin)]).with_type_metadata(
            crate::SymbolManifest::default(),
            BTreeMap::from([(
                builtin,
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![crate::ScriptType::Function],
                    result: crate::ScriptType::Task,
                },
            )]),
            Vec::new(),
        );
        let bytecode = compile("fn work() { 1 }\nschedule(work)", &manifest);
        let function_symbol = bytecode.functions[0].name;
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        let Some(VmEvent::Call(call)) = vm.step().expect("schedule yields") else {
            panic!("expected native call")
        };
        assert_eq!(
            call.arguments[0].value,
            Value::Function {
                module: None,
                symbol: function_symbol,
            }
        );
    }
}
