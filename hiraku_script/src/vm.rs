//! The executable register-based HKS bytecode compiler and VM.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{StringId, StringPool, string_pool::StringPoolBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    HirArena, MirConstant, MirInstruction, MirTerminator, Program, Register, RegisterFrame,
    ResolvedFunction, Span, StatementValue, SymbolId, SymbolManifest, allocate_registers,
    lower_hir_to_mir, lower_to_hir,
    runtime::{BuiltinManifest, CallArgument, Value},
};

pub const BYTECODE_VERSION: u16 = 21;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSlice {
    pub start: Register,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    pub debug: crate::debug::DebugInfo,
    pub version: u16,
    pub source_hash: u64,
    pub builtin_manifest_hash: u64,
    pub symbols: SymbolManifest,
    pub global_types: Vec<SymbolId>,
    pub strings: StringPool,
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
    /// Receiver, parameter and return types refer to this module's symbol manifest.
    pub signature: crate::FunctionSignature,
    pub parameters: Vec<u32>,
    pub register_count: u16,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BytecodeRegion {
    pub signature: crate::FunctionSignature,
    pub parameters: Vec<u32>,
    pub register_count: u16,
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    Panic {
        message: Register,
    },
    Udf(StringId),
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
    GlobalInitialized {
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
    ToString {
        dst: Register,
        value: Register,
    },
    Cast {
        dst: Register,
        value: Register,
        target: crate::ScriptType,
        mode: crate::CastMode,
    },
    MakeOptional {
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
    IsVariant {
        dst: Register,
        value: Register,
        type_name: SymbolId,
        variant: SymbolId,
    },
    VariantField {
        dst: Register,
        value: Register,
        index: u32,
    },
    MakeVariant {
        dst: Register,
        type_name: SymbolId,
        variant: SymbolId,
        values: RegisterSlice,
    },
    MakeMap {
        dst: Register,
        type_name: Option<SymbolId>,
        names: Vec<SymbolId>,
        values: RegisterSlice,
    },
    /// A symbolic call. Runtime linking decides whether the target is script
    /// bytecode or a native implementation.
    Call {
        #[serde(with = "type_binding_table")]
        type_bindings: BTreeMap<SymbolId, crate::ScriptType>,
        dst: Register,
        function: SymbolId,
        /// Caller-side types, including an explicit receiver when present.
        argument_types: Vec<crate::runtime::ArgumentType>,
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
    Uninitialized,
    Null,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(StringId),
    TextTemplate(StringId),
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
    compile_program(program, source_hash, manifest, None, None)
}

pub fn compile_with_project_interface(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
    interface: &crate::project::ProjectInterface,
) -> Result<Bytecode, Vec<CompileError>> {
    compile_program(program, source_hash, manifest, Some(interface), None)
}

pub fn compile_with_hir_pass(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
    interface: &crate::project::ProjectInterface,
    pass: &mut dyn crate::hir::HirPass,
) -> Result<Bytecode, Vec<CompileError>> {
    compile_program(program, source_hash, manifest, Some(interface), Some(pass))
}

fn compile_program(
    program: &Program,
    source_hash: u64,
    manifest: &BuiltinManifest,
    interface: Option<&crate::project::ProjectInterface>,
    pass: Option<&mut dyn crate::hir::HirPass>,
) -> Result<Bytecode, Vec<CompileError>> {
    let arena = HirArena::new();
    let mut hir = match interface {
        Some(interface) => {
            crate::hir::lower_with_project_interface(&arena, program, manifest, interface)
        }
        None => lower_to_hir(&arena, program, Some(manifest)),
    }
    .map_err(|errors| {
        errors
            .into_iter()
            .map(|error| CompileError {
                message: error.message,
                span: Some(error.span),
            })
            .collect::<Vec<_>>()
    })?;
    if let Some(pass) = pass {
        pass.run(&arena, &mut hir, manifest)?;
    }
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
    let mut strings = StringPoolBuilder::default();
    let mut debug = crate::debug::DebugInfo::default();
    let (entry, entry_debug) = compile_register_code(
        &mir.entry,
        manifest,
        &function_symbols,
        &mut symbols,
        &mut regions,
        &mut strings,
        &mut debug.regions,
    )?;
    debug.entry = entry_debug;
    let mut functions = Vec::with_capacity(mir.functions.len());
    for (mir_function, hir_function) in mir.functions.iter().zip(hir.functions) {
        let (code, mut code_debug) = compile_register_code(
            mir_function,
            manifest,
            &function_symbols,
            &mut symbols,
            &mut regions,
            &mut strings,
            &mut debug.regions,
        )?;
        code_debug.track_caller = hir_function.track_caller;
        debug.functions.push(code_debug);
        functions.push(BytecodeFunction {
            name: hir_function.name,
            exported: hir_function.exported,
            signature: code.signature,
            parameters: code.parameters,
            register_count: code.register_count,
            instructions: code.instructions,
        });
    }
    Ok(Bytecode {
        debug,
        version: BYTECODE_VERSION,
        source_hash,
        builtin_manifest_hash: manifest.hash(),
        symbols: symbols.manifest(),
        global_types: hir.global_types.clone(),
        strings: strings.finish(),
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
    strings: &mut StringPoolBuilder,
    region_debug: &mut Vec<crate::debug::CodeDebugInfo>,
) -> Result<(BytecodeRegion, crate::debug::CodeDebugInfo), Vec<CompileError>> {
    let mut region_ids = Vec::with_capacity(function.regions.len());
    for region in &function.regions {
        let (compiled, debug) = compile_register_code(
            region,
            manifest,
            function_symbols,
            symbols,
            regions,
            strings,
            region_debug,
        )?;
        let id = regions.len() as u32;
        regions.push(compiled);
        region_debug.push(debug);
        region_ids.push(id);
    }
    let allocation = allocate_registers(function).map_err(|error| {
        vec![CompileError {
            message: format!("register allocation failed: {error:?}"),
            span: None,
        }]
    })?;
    let (instructions, register_count, debug) = emit_function(
        function,
        &allocation,
        manifest,
        function_symbols,
        &region_ids,
        symbols,
        strings,
    )?;
    Ok((
        BytecodeRegion {
            signature: function.signature.clone(),
            parameters: function
                .parameters
                .iter()
                .map(|parameter| parameter.0)
                .collect(),
            register_count,
            instructions,
        },
        debug,
    ))
}

fn emit_function(
    function: &crate::MirFunction,
    allocation: &crate::RegisterAllocation,
    manifest: &BuiltinManifest,
    function_symbols: &[SymbolId],
    region_ids: &[u32],
    symbols: &mut crate::SymbolInterner,
    strings: &mut StringPoolBuilder,
) -> Result<(Vec<Instruction>, u16, crate::debug::CodeDebugInfo), Vec<CompileError>> {
    let register = |virtual_register| {
        allocation
            .register_for(virtual_register)
            .expect("MIR virtual register was allocated")
    };
    let mut blocks = Vec::with_capacity(function.blocks.len());
    let mut max_window = 0usize;
    for block in &function.blocks {
        let mut emitted = Vec::new();
        let mut locations = BTreeMap::new();
        for instruction in &block.instructions {
            let scalar = match instruction {
                MirInstruction::Panic { message, .. } => Instruction::Panic {
                    message: register(*message),
                },
                MirInstruction::Udf(reason) => Instruction::Udf(strings.intern(reason.clone())),
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
                        value => constant(value.clone(), strings),
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
                MirInstruction::GlobalInitialized { dst, global } => {
                    Instruction::GlobalInitialized {
                        dst: register(*dst),
                        global: global.0,
                    }
                }
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
                MirInstruction::ToString { dst, value } => Instruction::ToString {
                    dst: register(*dst),
                    value: register(*value),
                },
                MirInstruction::Cast {
                    dst,
                    value,
                    target,
                    mode,
                } => Instruction::Cast {
                    dst: register(*dst),
                    value: register(*value),
                    target: target.clone(),
                    mode: *mode,
                },
                MirInstruction::MakeOptional { dst, value } => Instruction::MakeOptional {
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
                MirInstruction::Move { dst, src } => Instruction::Move {
                    dst: register(*dst),
                    src: register(*src),
                },
                MirInstruction::IsVariant {
                    dst,
                    value,
                    type_name,
                    variant,
                } => Instruction::IsVariant {
                    dst: register(*dst),
                    value: register(*value),
                    type_name: *type_name,
                    variant: *variant,
                },
                MirInstruction::VariantField { dst, value, index } => Instruction::VariantField {
                    dst: register(*dst),
                    value: register(*value),
                    index: *index,
                },
                MirInstruction::MakeVariant {
                    dst,
                    type_name,
                    variant,
                    values,
                } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        values.iter().map(|value| register(*value)),
                    )?;
                    max_window = max_window.max(values.len());
                    Instruction::MakeVariant {
                        dst: register(*dst),
                        type_name: *type_name,
                        variant: *variant,
                        values: slice,
                    }
                }
                MirInstruction::MakeMap {
                    dst,
                    type_name,
                    fields,
                } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        fields.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(fields.len());
                    Instruction::MakeMap {
                        dst: register(*dst),
                        type_name: *type_name,
                        names: fields.iter().map(|(name, _)| *name).collect(),
                        values: slice,
                    }
                }
                MirInstruction::Call {
                    type_bindings,
                    span: _,
                    dst,
                    function: ResolvedFunction::Builtin(builtin),
                    receiver,
                    dynamic_callee: None,
                    arguments,
                    argument_types,
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
                        type_bindings: type_bindings.clone(),
                        dst: register(*dst),
                        function,
                        receiver: receiver.map(register),
                        argument_types: argument_types.clone(),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    type_bindings,
                    span: _,
                    dst,
                    function: ResolvedFunction::External(function),
                    receiver,
                    dynamic_callee: None,
                    arguments,
                    argument_types,
                } => {
                    let slice = emit_register_window(
                        &mut emitted,
                        allocation.register_count,
                        arguments.iter().map(|(_, value)| register(*value)),
                    )?;
                    max_window = max_window.max(arguments.len());
                    Instruction::Call {
                        type_bindings: type_bindings.clone(),
                        dst: register(*dst),
                        function: *function,
                        receiver: receiver.map(register),
                        argument_types: argument_types.clone(),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    type_bindings,
                    span: _,
                    dst,
                    function: ResolvedFunction::User(function),
                    receiver: None,
                    dynamic_callee: None,
                    arguments,
                    argument_types,
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
                        type_bindings: type_bindings.clone(),
                        dst: register(*dst),
                        function,
                        receiver: None,
                        argument_types: argument_types.clone(),
                        labels: arguments.iter().map(|(label, _)| *label).collect(),
                        arguments: slice,
                    }
                }
                MirInstruction::Call {
                    span: _,
                    dst,
                    function: ResolvedFunction::Dynamic,
                    receiver: None,
                    dynamic_callee: Some(callee),
                    arguments,
                    ..
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
                MirInstruction::AssertNonNull { dst, value, .. } => Instruction::AssertNonNull {
                    dst: register(*dst),
                    value: register(*value),
                },
                MirInstruction::Statement {
                    value,
                    string,
                    emit_value,
                    ..
                } => Instruction::Statement {
                    value: register(*value),
                    string: *string,
                    emit_value: *emit_value,
                },
            };
            if let MirInstruction::Panic { span, .. }
            | MirInstruction::AssertNonNull { span, .. }
            | MirInstruction::Call { span, .. }
            | MirInstruction::Statement { span, .. } = instruction
            {
                locations.insert(emitted.len(), *span);
            }
            emitted.push(scalar);
        }
        blocks.push((emitted, block.terminator.clone(), locations));
    }
    let mut starts = Vec::with_capacity(blocks.len());
    let mut offset = 0usize;
    for (instructions, _, _) in &blocks {
        starts.push(offset);
        offset += instructions.len() + 1;
    }
    let mut output = Vec::with_capacity(offset);
    let mut debug = crate::debug::CodeDebugInfo::default();
    for (mut instructions, terminator, locations) in blocks {
        debug.locations.extend(
            locations
                .into_iter()
                .map(|(pc, span)| (output.len() + pc, span)),
        );
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
    Ok((output, register_count, debug))
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

fn constant(value: MirConstant, strings: &mut StringPoolBuilder) -> Constant {
    match value {
        MirConstant::Uninitialized => Constant::Uninitialized,
        MirConstant::Null => Constant::Null,
        MirConstant::Unit => Constant::Unit,
        MirConstant::Ellipsis => Constant::Ellipsis,
        MirConstant::Bool(value) => Constant::Bool(value),
        MirConstant::Number(value) => Constant::Number(value),
        MirConstant::Percent(value) => Constant::Percent(value),
        MirConstant::String(value) => Constant::String(strings.intern(value)),
        MirConstant::TextTemplate(value) => Constant::TextTemplate(strings.intern(value)),
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
    /// Cooperative yield, not a statement commit or a host wait.
    BudgetExhausted,
    Call(SymbolCall),
    Statement(StatementValue),
    Completed(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolCall {
    pub type_bindings: BTreeMap<SymbolId, crate::ScriptType>,
    pub function: SymbolId,
    pub receiver: Option<Value>,
    pub arguments: Vec<CallArgument>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VmSnapshot {
    #[serde(with = "type_binding_table")]
    pub type_bindings: BTreeMap<SymbolId, crate::ScriptType>,
    #[serde(default)]
    pub read_only_globals: std::collections::BTreeSet<String>,
    pub objects: crate::ObjectHeap,
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
    #[serde(with = "type_binding_table")]
    pub type_bindings: BTreeMap<SymbolId, crate::ScriptType>,
    pub location: CodeLocation,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub locals: Vec<Value>,
    pub destination: Register,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vm {
    type_bindings: BTreeMap<SymbolId, crate::ScriptType>,
    read_only_globals: std::collections::BTreeSet<String>,
    objects: crate::ObjectHeap,
    bytecode: Arc<Bytecode>,
    pc: usize,
    registers: RegisterFrame,
    locals: Box<[Value]>,
    globals: Box<[Value]>,
    waiting_destination: Option<Register>,
    status: VmStatus,
    location: CodeLocation,
    call_stack: Vec<CallFrameSnapshot>,
}

mod type_binding_table {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        values: &BTreeMap<SymbolId, crate::ScriptType>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        values.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<SymbolId, crate::ScriptType>, D::Error> {
        let entries = Vec::<(SymbolId, crate::ScriptType)>::deserialize(deserializer)?;
        let count = entries.len();
        let values: BTreeMap<_, _> = entries.into_iter().collect();
        if values.len() != count {
            return Err(serde::de::Error::custom("duplicate generic type binding"));
        }
        Ok(values)
    }
}

impl Vm {
    pub fn new(bytecode: impl Into<Arc<Bytecode>>) -> Result<Self, VmError> {
        let bytecode = bytecode.into();
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        Ok(Self {
            registers: RegisterFrame::new(bytecode.register_count),
            read_only_globals: Default::default(),
            type_bindings: Default::default(),
            locals: vec![Value::Uninitialized; bytecode.local_count as usize].into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: VmStatus::Ready,
            location: CodeLocation::Entry,
            call_stack: Vec::new(),
            objects: crate::ObjectHeap::default(),
        })
    }

    pub fn from_closure(
        bytecode: impl Into<Arc<Bytecode>>,
        closure: &Value,
    ) -> Result<Self, VmError> {
        let bytecode = bytecode.into();
        let mut objects = crate::ObjectHeap::default();
        let closure = objects.import(closure.clone());
        let Value::Closure {
            region,
            captures,
            type_bindings,
            ..
        } = &closure
        else {
            return Err(VmError::TypeMismatch("expected Function"));
        };
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
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
            read_only_globals: Default::default(),
            type_bindings: type_bindings.iter().cloned().collect(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: VmStatus::Ready,
            location: CodeLocation::Region(*region),
            call_stack: Vec::new(),
            objects,
        })
    }

    /// Creates an independent VM invocation from a save-safe function value.
    pub fn from_callable(
        bytecode: impl Into<Arc<Bytecode>>,
        callable: &Value,
        arguments: Vec<Value>,
    ) -> Result<Self, VmError> {
        let bytecode = bytecode.into();
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
                    *vm.local_mut(local)? = vm.objects.import(value);
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
        bytecode: impl Into<Arc<Bytecode>>,
        function: u32,
        arguments: Vec<Value>,
    ) -> Result<Self, VmError> {
        let bytecode = bytecode.into();
        let metadata = bytecode
            .functions
            .get(function as usize)
            .ok_or(VmError::UnknownFunction(function))?;
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
        if metadata.parameters.len() != arguments.len() {
            return Err(VmError::FunctionArity {
                expected: metadata.parameters.len(),
                actual: arguments.len(),
            });
        }
        let register_count = metadata.register_count;
        let parameters = metadata.parameters.clone();
        let mut vm = Self {
            read_only_globals: Default::default(),
            type_bindings: Default::default(),
            registers: RegisterFrame::new(register_count),
            locals: vec![Value::Uninitialized; bytecode.local_count as usize].into_boxed_slice(),
            globals: vec![Value::Uninitialized; bytecode.globals.len()].into_boxed_slice(),
            bytecode,
            pc: 0,
            waiting_destination: None,
            status: VmStatus::Ready,
            location: CodeLocation::Function(function),
            call_stack: Vec::new(),
            objects: crate::ObjectHeap::default(),
        };
        for (local, value) in parameters.into_iter().zip(arguments) {
            *vm.local_mut(local)? = vm.objects.import(value);
        }
        Ok(vm)
    }

    pub fn step(&mut self) -> Result<Option<VmEvent>, VmError> {
        self.step_with_budget(&mut 10_000)
    }

    /// The caller may share a budget across events and linked function calls.
    /// Exhaustion preserves the exact execution state and requires no `resume`.
    pub fn step_with_budget(&mut self, remaining: &mut u32) -> Result<Option<VmEvent>, VmError> {
        if self.status != VmStatus::Ready {
            return Ok(None);
        }
        loop {
            if *remaining == 0 {
                return Ok(Some(VmEvent::BudgetExhausted));
            }
            // Validate after the embedding has installed the execution heap.
            // This also covers linked calls and restored function-entry frames.
            if self.pc == 0 {
                self.validate_function_arguments()?;
            }
            *remaining -= 1;
            let instruction = self
                .current_instructions()
                .get(self.pc)
                .cloned()
                .ok_or(VmError::InvalidProgramCounter(self.pc))?;
            self.pc += 1;
            match instruction {
                Instruction::Panic { message } => {
                    let Value::String(message) = self.read(message)? else {
                        return Err(VmError::UndefinedInstruction(
                            "panic expects a String".into(),
                        ));
                    };
                    return Err(self.panic_error(message.clone()));
                }
                Instruction::Udf(reason) => {
                    return Err(VmError::UndefinedInstruction(
                        self.string(reason)?.to_owned(),
                    ));
                }
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
                            type_bindings: self
                                .type_bindings
                                .iter()
                                .map(|(name, ty)| (*name, ty.clone()))
                                .collect(),
                            objects: None,
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
                    if self.global_is_read_only(global) {
                        self.objects.freeze(&value)?;
                    }
                    if value == Value::Uninitialized {
                        return Err(VmError::UninitializedGlobal(global));
                    }
                    self.write(dst, value)?;
                }
                Instruction::StoreGlobal { global, src } => {
                    if self.global_is_read_only(global) {
                        return Err(VmError::ReadOnlyValue);
                    }
                    let value = self.read(src)?.clone();
                    *self.global_mut(global)? = value;
                }
                Instruction::GlobalInitialized { dst, global } => {
                    let initialized = *self.global_slot(global)? != Value::Uninitialized;
                    self.write(dst, Value::Bool(initialized))?;
                }
                Instruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                } => {
                    let name = self.symbol(member)?.to_string();
                    let value = get_member(self.read(object)?, &name, safe, &self.objects)?;
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
                    if let Value::Object(id) = object {
                        set_member(self.objects.get_mut(id)?, &name, value)?;
                    } else {
                        set_member(&mut object, &name, value)?;
                    }
                    self.write(dst, object)?;
                }
                Instruction::UnaryMinus { dst, value } => {
                    let Value::Number(value) = self.read(value)? else {
                        return Err(VmError::TypeMismatch("unary minus expects Number"));
                    };
                    self.write(dst, Value::Number(-value))?;
                }
                Instruction::ToString { dst, value } => {
                    let text = match self.read(value)? {
                        Value::Number(number) => number.to_string(),
                        Value::Bool(value) => value.to_string(),
                        Value::String(value) => value.clone(),
                        _ => {
                            return Err(VmError::TypeMismatch(
                                "toString expects a primitive value",
                            ));
                        }
                    };
                    self.write(dst, Value::String(text))?;
                }
                Instruction::Cast {
                    dst,
                    value,
                    target,
                    mode,
                } => {
                    let target = crate::hir::substitute_type(&target, &self.type_bindings);
                    let source = self.read(value)?.clone();
                    let value = cast_value_with_heap(&source, &target, &self.objects);
                    let value = match (mode, value) {
                        (crate::CastMode::Optional, Ok(value)) => {
                            Value::Optional(Some(Box::new(value)))
                        }
                        (_, Ok(value)) => value,
                        (crate::CastMode::Optional, Err(_)) => Value::Optional(None),
                        (_, Err(error)) => return Err(error),
                    };
                    self.write(dst, value)?;
                }
                Instruction::MakeOptional { dst, value } => {
                    let value = self.read(value)?.clone();
                    self.write(dst, Value::Optional(Some(Box::new(value))))?;
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
                Instruction::IsVariant {
                    dst,
                    value,
                    type_name,
                    variant,
                } => {
                    let tag = self.symbol(variant)?;
                    let matches = match self.read(value)? {
                        Value::Optional(payload) if self.symbol(type_name)? == "Optional" => {
                            if payload.is_some() {
                                tag == "some"
                            } else {
                                tag == "none"
                            }
                        }
                        Value::Null if self.symbol(type_name)? == "Optional" => tag == "none",
                        Value::Typed { type_id, value } => {
                            *type_id == type_name
                                && matches!(value.as_ref(), Value::Tuple(fields) if matches!(fields.first(), Some(Value::Symbol(name)) if name == tag))
                        }
                        _ => false,
                    };
                    self.write(dst, Value::Bool(matches))?;
                }
                Instruction::VariantField { dst, value, index } => {
                    if let Value::Optional(Some(payload)) = self.read(value)? {
                        if index != 0 {
                            return Err(VmError::TypeMismatch("invalid enum payload index"));
                        }
                        let field = payload.as_ref().clone();
                        self.write(dst, field)?;
                        continue;
                    }
                    let Value::Typed { value, .. } = self.read(value)? else {
                        return Err(VmError::TypeMismatch("expected enum value"));
                    };
                    let Value::Tuple(fields) = value.as_ref() else {
                        return Err(VmError::TypeMismatch("expected enum payload"));
                    };
                    let field = fields
                        .get(index as usize + 1)
                        .ok_or(VmError::TypeMismatch("invalid enum payload index"))?
                        .clone();
                    self.write(dst, field)?;
                }
                Instruction::MakeVariant {
                    dst,
                    type_name,
                    variant,
                    values,
                } => {
                    let mut payload = vec![Value::Symbol(self.symbol(variant)?.to_string())];
                    payload.extend(self.read_slice(values)?);
                    self.write(
                        dst,
                        Value::Typed {
                            type_id: type_name,
                            value: Box::new(Value::Tuple(payload)),
                        },
                    )?;
                }
                Instruction::MakeMap {
                    dst,
                    type_name,
                    names,
                    values,
                } => {
                    let values = self.read_slice(values)?;
                    let fields = names
                        .into_iter()
                        .zip(values)
                        .map(|(name, value)| Ok((self.symbol(name)?.to_string(), value)))
                        .collect::<Result<BTreeMap<_, _>, VmError>>()?;
                    let value = Value::Map(fields);
                    let value = type_name.map_or(value.clone(), |type_id| Value::Typed {
                        type_id,
                        value: Box::new(value),
                    });
                    let value = self.objects.allocate(value);
                    self.write(dst, value)?;
                }
                Instruction::Call {
                    type_bindings,
                    dst,
                    function,
                    receiver,
                    labels,
                    arguments,
                    ..
                } => {
                    let type_bindings = type_bindings
                        .into_iter()
                        .map(|(name, ty)| {
                            (name, crate::hir::substitute_type(&ty, &self.type_bindings))
                        })
                        .collect::<BTreeMap<_, _>>();
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
                        self.type_bindings = type_bindings;
                        continue;
                    }
                    self.status = VmStatus::WaitingForHost;
                    self.waiting_destination = Some(dst);
                    return Ok(Some(VmEvent::Call(SymbolCall {
                        type_bindings,
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
                                type_bindings: Default::default(),
                                function,
                                receiver: None,
                                arguments,
                            })));
                        }
                        Value::Closure {
                            region,
                            captures,
                            type_bindings,
                            ..
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
                            self.type_bindings = type_bindings.into_iter().collect();
                            continue;
                        }
                        _ => return Err(VmError::TypeMismatch("callee expects Function")),
                    }
                }
                Instruction::AssertNonNull { dst, value } => {
                    let value = self.read(value)?.clone();
                    match value {
                        Value::Optional(Some(value)) => self.write(dst, *value)?,
                        Value::Optional(None) | Value::Null => {
                            return Err(self.panic_error(
                                "non-null assertion failed: expected a value, got null",
                            ));
                        }
                        value => self.write(dst, value)?,
                    }
                }
                Instruction::Statement {
                    value,
                    string,
                    emit_value,
                } => {
                    let value = self.read(value)?;
                    let statement = match (string, emit_value, value) {
                        (true, _, Value::String(value)) => {
                            StatementValue::TextTemplate(value.clone())
                        }
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
        let value = self.objects.import(value);
        self.write(destination, value)?;
        self.status = VmStatus::Ready;
        Ok(())
    }

    pub fn snapshot(&self) -> VmSnapshot {
        VmSnapshot {
            type_bindings: self.type_bindings.clone(),
            read_only_globals: self.read_only_globals.clone(),
            objects: self.objects.clone(),
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
        bytecode: impl Into<Arc<Bytecode>>,
        snapshot: VmSnapshot,
    ) -> Result<Self, VmError> {
        let bytecode = bytecode.into();
        if bytecode.version != BYTECODE_VERSION {
            return Err(VmError::UnsupportedBytecode(bytecode.version));
        }
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
            read_only_globals: snapshot.read_only_globals,
            type_bindings: snapshot.type_bindings,
            pc: snapshot.pc,
            registers,
            locals: snapshot.locals.into_boxed_slice(),
            globals: snapshot.globals.into_boxed_slice(),
            waiting_destination: snapshot.waiting_destination,
            status: snapshot.status,
            location: snapshot.location,
            call_stack: snapshot.call_stack,
            objects: snapshot.objects,
        })
    }

    pub fn status(&self) -> VmStatus {
        self.status
    }

    pub fn bytecode(&self) -> &Bytecode {
        &self.bytecode
    }

    pub fn objects(&self) -> &crate::ObjectHeap {
        &self.objects
    }

    /// Freeze objects supplied to a fresh invocation (parameters and captures).
    /// Call before stepping when an embedding exposes borrowed, read-only inputs.
    pub fn freeze_invocation_inputs(&mut self) -> Result<(), VmError> {
        for value in &self.locals {
            self.objects.freeze(value)?;
        }
        Ok(())
    }

    /// Roots for a collector owned by an embedding that shares this VM's heap.
    pub fn object_roots(&self) -> impl Iterator<Item = &Value> {
        self.registers
            .values()
            .iter()
            .chain(self.locals.iter())
            .chain(self.globals.iter())
            .chain(
                self.call_stack
                    .iter()
                    .flat_map(|frame| frame.registers.iter().chain(frame.locals.iter())),
            )
    }

    /// Collect this VM's private heap. Host-retained values must be included.
    /// For shared heaps, enumerate every execution with `object_roots` instead.
    pub fn collect_objects(&mut self, host_roots: &[Value]) -> Result<usize, VmError> {
        let roots = self
            .registers
            .values()
            .iter()
            .chain(self.locals.iter())
            .chain(self.globals.iter())
            .chain(
                self.call_stack
                    .iter()
                    .flat_map(|frame| frame.registers.iter().chain(frame.locals.iter())),
            )
            .chain(host_roots);
        self.objects.collect(roots)
    }

    /// An embedding that schedules multiple VMs owns one heap and lends it to
    /// the currently executing VM. This transfer never blocks a worker thread.
    pub fn swap_objects(&mut self, objects: &mut crate::ObjectHeap) {
        std::mem::swap(&mut self.objects, objects);
    }

    pub fn export_value(&self, value: &Value) -> Result<Value, VmError> {
        self.objects.export(value)
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

    pub fn set_read_only_globals(&mut self, names: std::collections::BTreeSet<String>) {
        self.read_only_globals = names;
    }

    pub fn set_type_bindings(&mut self, bindings: BTreeMap<SymbolId, crate::ScriptType>) {
        self.type_bindings = bindings;
    }

    pub(crate) fn read_only_globals(&self) -> &std::collections::BTreeSet<String> {
        &self.read_only_globals
    }

    fn global_is_read_only(&self, index: u32) -> bool {
        self.bytecode
            .globals
            .get(index as usize)
            .and_then(|symbol| self.bytecode.symbols.resolve(*symbol))
            .is_some_and(|name| self.read_only_globals.contains(name))
    }

    pub fn set_global_values(&mut self, values: Vec<Value>) -> Result<(), VmError> {
        if values.len() != self.bytecode.globals.len() {
            return Err(VmError::FrameShapeMismatch);
        }
        self.globals = values
            .into_iter()
            .map(|value| self.objects.import(value))
            .collect();
        Ok(())
    }

    pub fn eval_template(&self, template: &str) -> Result<String, crate::TemplateError> {
        self.eval_template_with(template, |source| Ok(source.to_string()))
    }

    /// Rewrites a lazy text template before evaluating its expressions.
    ///
    /// Embeddings can use this boundary for localization. The rewrite runs on
    /// the complete template source first, so the translated text may use a
    /// different set of `${...}` expressions than the source text.
    pub fn eval_template_with(
        &self,
        template: &str,
        rewrite: impl FnOnce(&str) -> Result<String, crate::TemplateError>,
    ) -> Result<String, crate::TemplateError> {
        let mut context = self.template_context();
        let template = rewrite(template)?;
        crate::eval_template(&template, &mut context)
    }

    /// Evaluate after localization, using the literal's lexical capture scope.
    pub fn eval_template_value_with(
        &self,
        template: &crate::runtime::TemplateValue,
        rewrite: impl FnOnce(&str) -> Result<String, crate::TemplateError>,
    ) -> Result<String, crate::TemplateError> {
        let source = rewrite(&template.source)?;
        let mut context = self.template_context();
        context.extend(template.captures.iter().map(|(name, value)| {
            (
                name.clone(),
                self.objects.export(value).unwrap_or_else(|_| value.clone()),
            )
        }));
        crate::eval_template(&source, &mut context)
    }

    fn template_context(&self) -> BTreeMap<String, Value> {
        self.template_captures()
            .into_iter()
            .map(|(name, value)| {
                let value = self.objects.export(&value).unwrap_or(value);
                (name, value)
            })
            .collect()
    }

    fn template_captures(&self) -> BTreeMap<String, Value> {
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
        context
    }

    fn constant_value(&self, value: Constant) -> Result<Value, VmError> {
        Ok(match value {
            Constant::Uninitialized => Value::Uninitialized,
            Constant::Null => Value::Optional(None),
            Constant::Unit => Value::Unit,
            Constant::Ellipsis => Value::Ellipsis,
            Constant::Bool(value) => Value::Bool(value),
            Constant::Number(value) => Value::Number(value),
            Constant::Percent(value) => Value::Percent(value),
            Constant::String(id) => Value::String(self.string(id)?.to_owned()),
            Constant::TextTemplate(id) => Value::TextTemplate(crate::runtime::TemplateValue {
                source: self.string(id)?.to_owned(),
                captures: self.template_captures().into(),
            }),
            Constant::Symbol(symbol) => Value::Symbol(self.symbol(symbol)?.to_string()),
            Constant::Selector(symbol) => Value::Selector(self.symbol(symbol)?.to_string()),
            Constant::Function(symbol) => Value::Function {
                module: None,
                symbol,
            },
        })
    }

    fn string(&self, id: StringId) -> Result<&str, VmError> {
        self.bytecode
            .strings
            .get(id)
            .ok_or(VmError::UnknownString(id))
    }

    fn symbol(&self, symbol: SymbolId) -> Result<&str, VmError> {
        self.bytecode
            .symbols
            .resolve(symbol)
            .ok_or(VmError::UnknownSymbol(symbol))
    }

    fn current_instructions(&self) -> &[Instruction] {
        self.instructions_at(self.location)
    }

    fn instructions_at(&self, location: CodeLocation) -> &[Instruction] {
        match location {
            CodeLocation::Entry => &self.bytecode.instructions,
            CodeLocation::Function(function) => {
                &self.bytecode.functions[function as usize].instructions
            }
            CodeLocation::Region(region) => &self.bytecode.regions[region as usize].instructions,
        }
    }

    fn panic_error(&self, message: impl Into<String>) -> VmError {
        let frames = self.stack_trace();
        VmError::Panic {
            message: message.into(),
            span: frames
                .first()
                .and_then(|frame| frame.span)
                .unwrap_or(Span { start: 0, end: 0 }),
            frames,
        }
    }

    pub fn stack_trace(&self) -> Vec<crate::debug::StackTraceFrame> {
        let source = self.bytecode.debug.source.clone().map(Arc::new);
        std::iter::once((self.location, self.pc))
            .chain(
                self.call_stack
                    .iter()
                    .rev()
                    .map(|frame| (frame.location, frame.pc)),
            )
            .filter_map(|(location, return_pc)| {
                let function = match location {
                    CodeLocation::Entry => "entry".to_string(),
                    CodeLocation::Region(index) => format!("closure#{index}"),
                    CodeLocation::Function(index) => {
                        let function = &self.bytecode.functions[index as usize];
                        if self
                            .bytecode
                            .debug
                            .functions
                            .get(index as usize)
                            .is_some_and(|info| info.track_caller)
                        {
                            return None;
                        }
                        self.bytecode
                            .symbols
                            .resolve(function.name)
                            .unwrap_or("<unknown>")
                            .replace("::", ".")
                    }
                };
                let pc = return_pc.saturating_sub(1);
                Some(crate::debug::StackTraceFrame {
                    function,
                    pc,
                    span: self.bytecode.debug.span(location, pc),
                    source: source.clone(),
                })
            })
            .collect()
    }

    /// Borrow source positions without cloning registers, heap, or source text.
    /// Includes suspended callers so hosts can inspect their continuations.
    pub fn source_positions(&self) -> impl Iterator<Item = (&str, crate::Span)> {
        std::iter::once((self.location, self.pc))
            .chain(
                self.call_stack
                    .iter()
                    .rev()
                    .map(|frame| (frame.location, frame.pc)),
            )
            .filter_map(|(location, pc)| {
                let source = self.bytecode.debug.source.as_ref()?;
                let info = match location {
                    CodeLocation::Entry => &self.bytecode.debug.entry,
                    CodeLocation::Function(index) => {
                        self.bytecode.debug.functions.get(index as usize)?
                    }
                    CodeLocation::Region(index) => {
                        self.bytecode.debug.regions.get(index as usize)?
                    }
                };
                let (_, span) = info.locations.range(..=pc.saturating_sub(1)).next_back()?;
                Some((source.path.as_str(), *span))
            })
    }

    fn validate_function_arguments(&self) -> Result<(), VmError> {
        let (parameters, signature) = match self.location {
            CodeLocation::Entry => return Ok(()),
            CodeLocation::Function(index) => {
                let function = &self.bytecode.functions[index as usize];
                (&function.parameters, &function.signature)
            }
            CodeLocation::Region(index) => {
                let region = &self.bytecode.regions[index as usize];
                (&region.parameters, &region.signature)
            }
        };
        let types = signature.receiver.iter().chain(&signature.parameters);
        for (index, (local, ty)) in parameters.iter().zip(types).enumerate() {
            let ty = crate::hir::substitute_type(ty, &self.type_bindings);
            if !argument_matches(self.local(*local)?, &ty, &self.objects)? {
                return Err(VmError::ArgumentTypeMismatch {
                    argument: index + 1,
                    expected: format!("{ty:?}"),
                });
            }
        }
        Ok(())
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
            type_bindings: self.type_bindings.clone(),
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
            type_bindings: self.type_bindings.clone(),
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
        self.type_bindings = frame.type_bindings;
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

fn get_member(
    value: &Value,
    name: &str,
    safe: bool,
    objects: &crate::ObjectHeap,
) -> Result<Value, VmError> {
    match value {
        Value::Object(id) => get_member(objects.get(*id)?, name, safe, objects),
        Value::Optional(None) if safe => Ok(Value::Optional(None)),
        Value::Optional(None) => Err(VmError::NullMemberAccess(name.to_string())),
        Value::Optional(Some(value)) if safe => get_member(value, name, false, objects)
            .map(|value| Value::Optional(Some(Box::new(value)))),
        Value::Optional(Some(value)) => get_member(value, name, false, objects),
        Value::Null if safe => Ok(Value::Optional(None)),
        Value::Map(fields) => fields
            .get(name)
            .cloned()
            .ok_or_else(|| VmError::UnknownMember(name.to_string())),
        Value::Typed { value, .. } => get_member(value, name, safe, objects),
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
    if op == BinaryOp::Add
        && let (Value::String(left), Value::String(right)) = (left, right)
    {
        let mut result = String::with_capacity(left.len() + right.len());
        result.push_str(left);
        result.push_str(right);
        return Ok(Value::String(result));
    }
    match op {
        BinaryOp::And | BinaryOp::Or => Err(VmError::TypeMismatch(
            "logical operators must be lowered to branches",
        )),
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

/// Non-coercing validation of the runtime representation at a script boundary.
/// Nominal host IDs and erased callable signatures require linker/host metadata;
/// those checks are intentionally not guessed from module-local symbol IDs.
fn argument_matches(
    value: &Value,
    ty: &crate::ScriptType,
    heap: &crate::ObjectHeap,
) -> Result<bool, VmError> {
    use crate::ScriptType as T;
    if matches!(ty, T::Any | T::TypeParameter(_)) {
        return Ok(!matches!(value, Value::Uninitialized));
    }
    if let Value::Object(id) = value {
        return argument_matches(heap.get(*id)?, ty, heap);
    }
    Ok(match (ty, value) {
        (T::Unit, Value::Unit)
        | (T::Ellipsis, Value::Ellipsis)
        | (T::Bool, Value::Bool(_))
        | (T::Float, Value::Number(_))
        | (T::Percent, Value::Percent(_))
        | (T::String, Value::String(_))
        | (T::TextTemplate, Value::TextTemplate(_))
        | (T::Symbol, Value::Symbol(_))
        | (T::Selector, Value::Selector(_))
        | (T::Task, Value::Task(_))
        | (T::Tuple, Value::Tuple(_)) => true,
        (T::Int, Value::Number(n)) => {
            n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
        }
        (T::Optional(_), Value::Optional(None) | Value::Null) => true,
        (T::Optional(inner), Value::Optional(Some(value))) => argument_matches(value, inner, heap)?,
        (T::Optional(inner), value) => argument_matches(value, inner, heap)?,
        (T::TupleOf(types), Value::Tuple(values)) => {
            if types.len() != values.len() {
                return Ok(false);
            }
            for (ty, value) in types.iter().zip(values) {
                if !argument_matches(value, ty, heap)? {
                    return Ok(false);
                }
            }
            true
        }
        (T::List(element), Value::List(values)) => {
            for value in values {
                if !argument_matches(value, element, heap)? {
                    return Ok(false);
                }
            }
            true
        }
        (T::Record(fields), Value::Map(values)) => {
            for (name, ty) in fields {
                let Some(value) = values.get(name) else {
                    return Ok(false);
                };
                if !argument_matches(value, ty, heap)? {
                    return Ok(false);
                }
            }
            true
        }
        (T::Map(key, element), Value::Map(values)) if **key == T::String => {
            for value in values.values() {
                if !argument_matches(value, element, heap)? {
                    return Ok(false);
                }
            }
            true
        }
        (T::Union(types), value) => {
            for ty in types {
                if argument_matches(value, ty, heap)? {
                    return Ok(true);
                }
            }
            false
        }
        (T::Function | T::Callable { .. }, Value::Function { .. } | Value::Closure { .. })
        | (T::Binding(_), Value::Closure { .. })
        | (T::Named(_), Value::Handle { .. } | Value::Typed { .. }) => true,
        (T::Enum { name, variants, .. }, Value::Typed { type_id, value }) if name == type_id => {
            let Value::Tuple(fields) = value.as_ref() else {
                return Ok(false);
            };
            let Some(Value::Symbol(tag)) = fields.first() else {
                return Ok(false);
            };
            let Some(types) = variants.get(tag) else {
                return Ok(false);
            };
            if types.len() + 1 != fields.len() {
                return Ok(false);
            }
            for (value, ty) in fields[1..].iter().zip(types) {
                if !argument_matches(value, ty, heap)? {
                    return Ok(false);
                }
            }
            true
        }
        (T::Struct { fields, .. }, Value::Typed { value, .. }) => {
            argument_matches(value, &T::Record(fields.clone()), heap)?
        }
        _ => false,
    })
}

fn cast_value_with_heap(
    value: &Value,
    target: &crate::ScriptType,
    heap: &crate::objects::ObjectHeap,
) -> Result<Value, VmError> {
    use crate::ScriptType as T;
    match (value, target) {
        (_, T::Any) => Ok(value.clone()),
        (Value::List(values), T::List(element)) => values
            .iter()
            .map(|value| cast_value_with_heap(value, element, heap))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        (Value::Tuple(values), T::TupleOf(types)) if values.len() == types.len() => values
            .iter()
            .zip(types)
            .map(|(value, ty)| cast_value_with_heap(value, ty, heap))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Tuple),
        (Value::Null | Value::Optional(None), T::Optional(_)) => Ok(Value::Optional(None)),
        (Value::Optional(Some(value)), T::Optional(inner)) => {
            cast_value_with_heap(value, inner, heap).map(|v| Value::Optional(Some(Box::new(v))))
        }
        (value, T::Optional(inner)) => {
            cast_value_with_heap(value, inner, heap).map(|v| Value::Optional(Some(Box::new(v))))
        }
        (Value::Object(_), _) => heap
            .export(value)
            .and_then(|exported| cast_value(&exported, target))
            .map(|_| value.clone()),
        _ => cast_value(value, target),
    }
}

fn cast_value(value: &Value, target: &crate::ScriptType) -> Result<Value, VmError> {
    use crate::ScriptType;

    let mismatch = || VmError::CastFailed(format!("value cannot be cast to {target:?}"));
    match target {
        ScriptType::Any => Ok(value.clone()),
        ScriptType::Unit if matches!(value, Value::Unit) => Ok(value.clone()),
        ScriptType::Ellipsis if matches!(value, Value::Ellipsis) => Ok(value.clone()),
        ScriptType::Bool if matches!(value, Value::Bool(_)) => Ok(value.clone()),
        ScriptType::Int => match value {
            Value::Number(number) if number.is_finite() => {
                // Until integers have a dedicated runtime representation, do not
                // silently round/clamp values outside the exact integer range.
                const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
                if number.trunc().abs() > MAX_SAFE_INTEGER {
                    return Err(mismatch());
                }
                Ok(Value::Number(number.trunc()))
            }
            _ => Err(mismatch()),
        },
        ScriptType::Float if matches!(value, Value::Number(_)) => Ok(value.clone()),
        ScriptType::Percent if matches!(value, Value::Percent(_)) => Ok(value.clone()),
        ScriptType::String if matches!(value, Value::String(_)) => Ok(value.clone()),
        ScriptType::TextTemplate if matches!(value, Value::TextTemplate(_)) => Ok(value.clone()),
        ScriptType::Symbol if matches!(value, Value::Symbol(_)) => Ok(value.clone()),
        ScriptType::Selector if matches!(value, Value::Selector(_)) => Ok(value.clone()),
        ScriptType::Function if matches!(value, Value::Function { .. } | Value::Closure { .. }) => {
            Ok(value.clone())
        }
        ScriptType::Task if matches!(value, Value::Task(_)) => Ok(value.clone()),
        ScriptType::Named(expected) => match value {
            Value::Typed { type_id, .. } if type_id == expected => Ok(value.clone()),
            Value::Handle { type_id, .. } if *type_id == expected.0 => Ok(value.clone()),
            _ => Err(mismatch()),
        },
        ScriptType::Enum { name, variants, .. } => match value {
            Value::Typed {
                type_id,
                value: payload,
            } if name == type_id => {
                let Value::Tuple(fields) = payload.as_ref() else {
                    return Err(mismatch());
                };
                let Some(Value::Symbol(tag)) = fields.first() else {
                    return Err(mismatch());
                };
                let Some(types) = variants.get(tag) else {
                    return Err(mismatch());
                };
                if types.len() + 1 != fields.len() {
                    return Err(mismatch());
                }
                let mut values = vec![Value::Symbol(tag.clone())];
                for (value, ty) in fields[1..].iter().zip(types) {
                    values.push(cast_value(value, ty)?);
                }
                Ok(Value::Typed {
                    type_id: *name,
                    value: Box::new(Value::Tuple(values)),
                })
            }
            _ => Err(mismatch()),
        },
        ScriptType::Struct { name, fields, .. } => match value {
            Value::Typed { type_id, value } if type_id == name => match value.as_ref() {
                Value::Map(values) => fields
                    .iter()
                    .map(|(field, ty)| {
                        let value = values.get(field).ok_or_else(mismatch)?;
                        Ok((field.clone(), cast_value(value, ty)?))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()
                    .map(|value| Value::Typed {
                        type_id: *name,
                        value: Box::new(Value::Map(value)),
                    }),
                _ => Err(mismatch()),
            },
            _ => Err(mismatch()),
        },
        ScriptType::TypeParameter(_) => Err(VmError::CastFailed(
            "cast target contains an unresolved generic parameter".into(),
        )),
        ScriptType::Optional(inner) => match value {
            Value::Optional(None) | Value::Null => Ok(Value::Optional(None)),
            Value::Optional(Some(value)) => {
                cast_value(value, inner).map(|value| Value::Optional(Some(Box::new(value))))
            }
            value => cast_value(value, inner).map(|value| Value::Optional(Some(Box::new(value)))),
        },
        ScriptType::Union(types) => types
            .iter()
            .find_map(|candidate| cast_value(value, candidate).ok())
            .ok_or_else(mismatch),
        ScriptType::TupleOf(types) => {
            let Value::Tuple(values) = value else {
                return Err(mismatch());
            };
            if values.len() != types.len() {
                return Err(mismatch());
            }
            Ok(Value::Tuple(
                values
                    .iter()
                    .zip(types)
                    .map(|(value, ty)| cast_value(value, ty))
                    .collect::<Result<_, _>>()?,
            ))
        }
        ScriptType::Tuple if matches!(value, Value::Tuple(_)) => Ok(value.clone()),
        ScriptType::List(element) => match value {
            Value::List(values) => values
                .iter()
                .map(|value| cast_value(value, element))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List),
            _ => Err(mismatch()),
        },
        ScriptType::Record(fields) => match value {
            Value::Map(values) => fields
                .iter()
                .map(|(name, ty)| {
                    let value = values.get(name).ok_or_else(mismatch)?;
                    Ok((name.clone(), cast_value(value, ty)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(Value::Map),
            _ => Err(mismatch()),
        },
        ScriptType::Map(key, element) if key.as_ref() == &ScriptType::String => match value {
            Value::Map(values) => values
                .iter()
                .map(|(name, value)| Ok((name.clone(), cast_value(value, element)?)))
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(Value::Map),
            _ => Err(mismatch()),
        },
        ScriptType::Binding(_) if matches!(value, Value::Closure { .. }) => Ok(value.clone()),
        _ => Err(mismatch()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    ReadOnlyValue,
    ProgramFingerprintMismatch,
    InvalidObject(crate::ObjectId),
    CyclicHostValue,
    Panic {
        message: String,
        span: Span,
        frames: Vec<crate::debug::StackTraceFrame>,
    },
    UndefinedInstruction(String),
    UnsupportedBytecode(u16),
    InvalidProgramCounter(usize),
    InvalidRegister(Register),
    InvalidLocal(u32),
    InvalidGlobal(u32),
    UnknownSymbol(SymbolId),
    UnknownString(StringId),
    UnknownMember(String),
    NullMemberAccess(String),
    CastFailed(String),
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
    FunctionArity {
        expected: usize,
        actual: usize,
    },
    ArgumentTypeMismatch {
        argument: usize,
        expected: String,
    },
    ReturnOutsideFunction,
}

impl VmError {
    pub fn render_diagnostic(&self, options: crate::RenderOptions) -> Option<String> {
        let Self::Panic {
            message, frames, ..
        } = self
        else {
            return None;
        };
        let mut output = format!("script panicked: {message}\n");
        let pretty_start = frames.len().saturating_sub(3);
        for (index, frame) in frames.iter().enumerate() {
            if index >= pretty_start
                && let (Some(source), Some(span)) = (&frame.source, frame.span)
            {
                let mut sources = crate::SourceMap::new();
                let id = sources.insert(&source.path, &source.text);
                let diagnostic = crate::Diagnostic::error(format!("at {}", frame.function))
                    .with_code("HKS-PANIC")
                    .with_label(crate::DiagnosticLabel::primary(id, span.range()));
                output.push_str(&crate::render_diagnostics(&[diagnostic], &sources, options));
            } else {
                output.push_str(&format!("  at {}\n", frame.location()));
            }
        }
        Some(output)
    }

    pub fn diagnostic(&self, source: crate::SourceId) -> crate::Diagnostic {
        match self {
            Self::Panic { message, span, .. } => crate::Diagnostic::error(message)
                .with_code("HKS-PANIC")
                .with_label(crate::DiagnosticLabel::primary(source, span.range())),
            error => crate::Diagnostic::error(format!("{error:?}")).with_code("HKS-RUNTIME"),
        }
    }
}

impl std::fmt::Display for VmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(rendered) = self.render_diagnostic(crate::RenderOptions::terminal()) {
            return formatter.write_str(&rendered);
        }
        match self {
            Self::ProgramFingerprintMismatch => formatter.write_str("compiled program fingerprint does not match the saved program; restoring its program counter is unsafe"),
            Self::ReadOnlyValue => formatter.write_str("cannot modify a read-only value; request changes through an explicitly provided callback"),
            Self::Panic { message, span, .. } => write!(
                formatter,
                "script panicked at bytes {}..{}: {message}",
                span.start, span.end
            ),
            error => write!(formatter, "{error:?}"),
        }
    }
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
    fn elvis_chains_are_short_circuiting_and_preserve_bottom_types() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for (expression, expected, expected_calls) in [
            (
                r#"pick(true) ?: null ?: panic("some null")"#,
                Some("Alice"),
                1.0,
            ),
            (r#"pick(false) ?: null ?: panic("some null")"#, None, 1.0),
            (
                r#"pick(false) ?: "123" ?: panic("some null")"#,
                Some("123"),
                1.0,
            ),
            (
                r#"pick(true) ?: "123" ?: panic("some null")"#,
                Some("Alice"),
                1.0,
            ),
            (r#""123" ?: panic("some null")"#, Some("123"), 0.0),
            (r#"null ?: "123" ?: panic("some null")"#, Some("123"), 0.0),
            (
                r#"(pick(true) ?: null) ?: panic("some null")"#,
                Some("Alice"),
                1.0,
            ),
            (r#"(pick(false) ?: null) ?: panic("some null")"#, None, 1.0),
            (
                r#"pick(false) ?: pick(true) ?: panic("some null")"#,
                Some("Alice"),
                2.0,
            ),
        ] {
            let source = format!(
                r#"
                global var calls: Int = 0
                fn pick(present: Bool) -> String? {{
                    calls += 1
                    if present {{ return "Alice" }}
                    return null
                }}
                global let result: String = {expression}
            "#
            );
            let mut vm = Vm::new(compile(&source, &manifest)).expect("VM");
            let mut finished = false;
            for _ in 0..2000 {
                match vm.step() {
                    Ok(Some(VmEvent::Completed(_))) => {
                        assert_eq!(
                            vm.global("result"),
                            expected.map(|s| Value::String(s.into())).as_ref(),
                            "{expression}"
                        );
                        finished = true;
                        break;
                    }
                    Err(VmError::Panic { message, .. }) => {
                        assert!(
                            expected.is_none(),
                            "{expression}: unexpected panic {message}"
                        );
                        assert_eq!(message, "some null");
                        finished = true;
                        break;
                    }
                    Ok(Some(VmEvent::Statement(_) | VmEvent::BudgetExhausted)) => {}
                    other => panic!("{expression}: {other:?}"),
                }
            }
            assert!(finished, "{expression}");
            assert_eq!(
                vm.global("calls"),
                Some(&Value::Number(expected_calls)),
                "{expression}"
            );
        }
    }

    #[test]
    fn elvis_chain_restores_a_suspended_fallback() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let code = compile(
            r#"
            fn fallback() -> String { checkpoint(); "Bob" }
            let absent: String? = null
            global let result: String = absent ?: null ?: fallback() ?: panic("unreachable")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM");
        let mut restored = false;
        for _ in 0..1000 {
            match vm.step().expect("execute") {
                Some(VmEvent::Call(_)) => {
                    assert!(!restored);
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("restore");
                    vm.resume(Value::Unit).expect("resume");
                    restored = true;
                }
                Some(VmEvent::Completed(_)) => {
                    assert!(restored);
                    assert_eq!(vm.global("result"), Some(&Value::String("Bob".into())));
                    return;
                }
                _ => {}
            }
        }
        panic!("execution did not finish");
    }

    #[test]
    fn null_assertion_uses_panic_diagnostics_and_exact_source_span() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let source = "fn require(value: String?) -> String {\n    value!\n}\nrequire(null)";
        let mut code = compile(source, &manifest);
        code.debug.source = Some(crate::debug::DebugSource {
            path: "assertion.hks".into(),
            text: source.into(),
        });
        let mut vm = Vm::new(code).expect("VM");
        for _ in 0..1000 {
            match vm.step() {
                Err(error @ VmError::Panic { .. }) => {
                    let VmError::Panic {
                        message,
                        span,
                        frames,
                    } = &error
                    else {
                        unreachable!()
                    };
                    assert_eq!(&source[span.range()], "value!");
                    assert!(message.contains("non-null assertion failed"));
                    assert_eq!(
                        frames
                            .iter()
                            .map(|frame| frame.function.as_str())
                            .collect::<Vec<_>>(),
                        ["require", "entry"]
                    );
                    let report = error
                        .render_diagnostic(crate::RenderOptions::plain())
                        .expect("pretty diagnostic");
                    assert!(report.contains("assertion.hks:2:5"), "{report}");
                    assert!(report.contains("value!"), "{report}");
                    assert!(report.contains("non-null assertion failed"), "{report}");
                    return;
                }
                Ok(Some(VmEvent::Statement(_))) | Ok(Some(VmEvent::BudgetExhausted)) => {}
                result => panic!("expected assertion panic, got {result:?}"),
            }
        }
        panic!("assertion did not fail");
    }

    #[test]
    fn interpolation_uses_lexical_scope_and_calls_resume_normally() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let code = compile(
            r#"
            fn next(value: Int) -> Int { checkpoint(); value + 1 }
            var index = 0
            global var output = ""
            while index < 3 {
                let evaluated: String = "Iteration ${index}"
                output += evaluated
                index += 1
            }
            let raw: TextTemplate = "${undefinedName}"
            global let savedTemplate: TextTemplate = raw
            global let call = "Next: ${next(index)}"
            let fallback: String? = null
            global let nested = "Name: ${fallback ?: "Alice"}"
            global let method = "Method: ${index.toString()}"
            let producer = { "Captured: ${index}" }
            global let captured = producer()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM");
        let mut resumed = false;
        let mut completed = false;
        for _ in 0..2000 {
            match vm.step().expect("interpolation executes") {
                Some(VmEvent::Call(_)) => {
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("restore");
                    vm.resume(Value::Unit).expect("resume");
                    resumed = true;
                }
                Some(VmEvent::Completed(_)) => {
                    completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(resumed && completed);
        for (name, expected) in [
            ("output", "Iteration 0Iteration 1Iteration 2"),
            ("call", "Next: 4"),
            ("nested", "Name: Alice"),
            ("method", "Method: 3"),
            ("captured", "Captured: 3"),
        ] {
            assert_eq!(vm.global(name), Some(&Value::String(expected.into())));
        }
        assert!(
            matches!(vm.global("savedTemplate"), Some(Value::TextTemplate(template))
            if template.source == "${undefinedName}")
        );
    }

    #[test]
    fn string_addition_in_when_and_compound_assignment_survives_restore() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let code = compile(
            r#"
            enum Greeting { hello(String), goodbye }
            fn greet(value: Greeting) -> String {
                when value {
                    .hello(name) -> "Hello, " + name
                    .goodbye -> "Goodbye!"
                }
            }
            global var message = greet(.hello("Alice"))
            checkpoint()
            message += "!"
            global let empty = "" + ""
            global let unicode = "こんにちは" + " 🌸"
            global let chained = "a" + "b" + "c"
            global let explicit = "value: " + 42.toString()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM initializes");
        let mut restored = false;
        let mut completed = false;
        for _ in 0..2000 {
            match vm.step().expect("execute string concatenation") {
                Some(VmEvent::Call(_)) => {
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("restore");
                    vm.resume(Value::Unit).expect("resume");
                    restored = true;
                }
                Some(VmEvent::Completed(_)) => {
                    completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(restored && completed);
        for (name, expected) in [
            ("message", "Hello, Alice!"),
            ("empty", ""),
            ("unicode", "こんにちは 🌸"),
            ("chained", "abc"),
            ("explicit", "value: 42"),
        ] {
            assert_eq!(vm.global(name), Some(&Value::String(expected.into())));
        }
        assert!(
            binary(
                crate::BinaryOp::Add,
                &Value::String("a".into()),
                &Value::Number(1.0)
            )
            .is_err()
        );
    }

    #[test]
    fn std_result_and_nullable_share_typed_match_and_restore() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let code = compile(
            r#"
            fn wrap<T>(value: T) -> Result<T, String> { .success(value) }
            let optional: Int? = Optional.some<Int>(42)
            let result: Result<Optional<Int>, String> = wrap(optional)
            global var answer = when result {
                .success(value) -> when value {
                    .some(number) -> { checkpoint(); number }
                    .none -> 0
                }
                .error(message) -> -1
            }
            let failed: Result<Int, String> = .error("alice")
            global var label = when failed {
                .success(value) -> "bob"
                .error(message) -> message
            }
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("initialize");
        let mut restored = false;
        for _ in 0..2000 {
            match vm.step().expect("execute") {
                Some(VmEvent::Call(_)) => {
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("restore");
                    vm.resume(Value::Unit).expect("resume");
                    restored = true;
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert!(restored);
        assert_eq!(vm.global("answer"), Some(&Value::Number(42.0)));
        assert_eq!(vm.global("label"), Some(&Value::String("alice".into())));
    }

    #[test]
    fn std_optional_match_fallback_is_lazy() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile(
            r#"
            global var calls=0
            fn fallback() -> Int { calls+=1; 7 }
            let nested: Optional<Optional<Int>> = .some(.none)
            global var result = when nested {
                .some(inner) -> when inner {
                    .some(value) -> value
                    .none -> fallback()
                }
                .none -> fallback()
            }
            let present: Optional<Int> = .some(42)
            global var lazy = present ?: fallback()
            let absent: Int? = null
            global var absentValue = absent ?: fallback()
            global var second: Float = when present {
                .some(value) -> 2
                .none -> 3
            }
            global var preserved = when present {
                .some(value) -> value
                .none -> fallback()
            }
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code).expect("initialize");
        let mut completed = false;
        for _ in 0..2000 {
            if matches!(vm.step().expect("execute"), Some(VmEvent::Completed(_))) {
                completed = true;
                break;
            }
        }
        assert!(completed);
        assert_eq!(vm.global("calls"), Some(&Value::Number(2.0)));
        assert_eq!(vm.global("lazy"), Some(&Value::Number(42.0)));
        assert_eq!(vm.global("absentValue"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("result"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("preserved"), Some(&Value::Number(42.0)));
        assert_eq!(vm.global("second"), Some(&Value::Number(2.0)));
    }

    #[test]
    fn enum_when_evaluates_subject_once_and_restores_payload_scope() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let code = compile(
            r#"
            enum Packet<T> { data(T), empty }
            global var calls=0
            fn identity<T>(value: Packet<T>) -> Packet<T> { value }
            fn subject() -> Packet<Int> {
                calls += 1
                return .data(42)
            }
            global var result = when identity(subject()) {
                .data(item) -> {
                    let capture = { item }
                    checkpoint()
                    capture()
                }
                .empty -> { 0 }
            }
            global var fallback = when Packet.empty<Int>() {
                .data(item) -> item
                .empty -> 7
            }
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("initialize");
        for _ in 0..1000 {
            if matches!(vm.step().expect("execute"), Some(VmEvent::Call(_))) {
                break;
            }
        }
        assert_eq!(vm.global("calls"), Some(&Value::Number(1.0)));
        let snapshot = crate::hson::to_string(&vm.snapshot()).expect("snapshot");
        let mut vm =
            Vm::restore(code, crate::hson::from_str(&snapshot).expect("decode")).expect("restore");
        vm.resume(Value::Unit).expect("resume");
        let mut completed = false;
        for _ in 0..1000 {
            if matches!(vm.step().expect("finish"), Some(VmEvent::Completed(_))) {
                completed = true;
                break;
            }
        }
        assert!(completed);
        assert_eq!(vm.global("result"), Some(&Value::Number(42.0)));
        assert_eq!(vm.global("fallback"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("calls"), Some(&Value::Number(1.0)));
    }

    #[test]
    fn boolean_expressions_short_circuit_and_survive_instruction_snapshots() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let source = r#"
            global var calls = 0
            fn probe() -> Bool { calls += 1; true }
            let state = .{ finished: false }
            let optional: Bool? = true
            global let results = [
                !state.finished, !(false || true), !!true, !optional!,
                true || false && false, (true || false) && false,
                false && probe(), true || probe(), true && probe(), false || probe(),
                false && (unreachable() as! Bool), true || (unreachable() as! Bool)
            ]
            var finished = false
            var count = 0
            while !finished && count < 4 {
                count += 1
                finished = count == 3
            }
            global let iterations = count
        "#;
        let code = Arc::new(compile(source, &manifest));
        let mut vm = Vm::new(code.clone()).expect("VM initializes");
        for _ in 0..2000 {
            let snapshot = crate::hson::from_str(
                &crate::hson::to_string(&vm.snapshot()).expect("snapshot serializes"),
            )
            .expect("snapshot parses");
            vm = Vm::restore(code.clone(), snapshot).expect("snapshot restores");
            if matches!(
                vm.step_with_budget(&mut 1).expect("boolean program runs"),
                Some(VmEvent::Completed(_))
            ) {
                assert_eq!(vm.globals()[0], Value::Number(2.0));
                assert_eq!(
                    vm.globals()[1],
                    Value::List(
                        [
                            true, false, true, false, true, false, false, true, true, true, false,
                            true
                        ]
                        .into_iter()
                        .map(Value::Bool)
                        .collect()
                    )
                );
                assert_eq!(vm.globals()[2], Value::Number(3.0));
                return;
            }
        }
        panic!("boolean program exceeded instruction budget");
    }

    #[test]
    fn boolean_operators_reject_non_bool_even_in_unreachable_operands() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "let value = !1",
            "let value = !\"false\"",
            "let value = false && 1",
            "let value = true || 1",
            "let value: Bool? = true; !value",
            "let value: Any = true; !value",
            "if 1 {}",
            "while \"true\" {}",
            "let result = true == 1",
            "let result = false < true",
        ] {
            let program = parse_program(source).expect("invalid types still parse");
            let error = compile_with_manifest(&program, 91, &manifest).expect_err(source);
            assert!(format!("{error:?}").contains("Bool"), "{source}: {error:?}");
        }
    }

    #[test]
    fn boolean_rhs_can_wait_for_a_native_response() {
        let mut registry = crate::native::NativeRegistry::<()>::default();
        registry
            .register_fn(
                "probe",
                |_: &mut ()| -> Result<bool, crate::native::NativeError> { Ok(true) },
            )
            .expect("native function registers");
        registry
            .set_signature_for(
                "probe",
                crate::FunctionSignature {
                    receiver: None,
                    parameters: Vec::new(),
                    variadic: None,
                    result: crate::ScriptType::Bool,
                },
            )
            .expect("Bool signature registers");
        for (expression, response, expected) in [
            ("true && probe()", false, false),
            ("false || probe()", true, true),
            ("!probe()", false, true),
        ] {
            let code = Arc::new(compile(
                &format!("global let result = {expression}"),
                &registry.manifest(),
            ));
            let mut vm = Vm::new(code.clone()).expect("VM initializes");
            let mut calls = 0;
            for _ in 0..100 {
                match vm.step().expect("boolean call runs") {
                    Some(VmEvent::Call(_)) => {
                        calls += 1;
                        vm = Vm::restore(code.clone(), vm.snapshot()).expect("host wait restores");
                        vm.resume(Value::Bool(response))
                            .expect("native response resumes");
                    }
                    Some(VmEvent::Completed(_)) => break,
                    _ => {}
                }
            }
            assert_eq!(calls, 1);
            assert_eq!(vm.globals(), &[Value::Bool(expected)]);
        }
    }

    #[test]
    fn boolean_conditions_narrow_optional_operands() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = Arc::new(compile(
            r#"
            fn positive(value: Int) -> Bool { value > 0 }
            let value: Int? = 2
            global let andResult = value != null && positive(value)
            global let orResult = value == null || positive(value)
            global var negated = false
            if !(value == null) { negated = positive(value) }
        "#,
            &manifest,
        ));
        let mut vm = Vm::new(code).expect("VM initializes");
        for _ in 0..100 {
            if matches!(
                vm.step().expect("narrowed program runs"),
                Some(VmEvent::Completed(_))
            ) {
                assert_eq!(
                    vm.globals(),
                    &[Value::Bool(true), Value::Bool(true), Value::Bool(true)]
                );
                return;
            }
        }
        panic!("narrowed program did not finish");
    }

    #[test]
    fn boolean_constants_support_logical_and_comparison_operators() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = Arc::new(compile(
            r#"
            const ready: Bool = !false && (1 <= 2 || false)
            const same = "alice" == "alice"
            const other = 2 != 1 && 3 >= 2 && 3 > 1
            const shortCircuit = true || (1 / 0 > 0)
            global let result = ready && same && other && shortCircuit
        "#,
            &manifest,
        ));
        let mut vm = Vm::new(code).expect("VM initializes");
        for _ in 0..100 {
            if matches!(
                vm.step().expect("constant program runs"),
                Some(VmEvent::Completed(_))
            ) {
                assert_eq!(vm.globals(), &[Value::Bool(true)]);
                return;
            }
        }
        panic!("constant program did not finish");
    }

    #[test]
    fn generic_cast_uses_call_site_type_and_survives_snapshot() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for literal in ["1", "\"alice\""] {
            let source = format!(
                "fn cast<T>(value: Any) -> T {{ value as! T }}\nglobal var result: Int = cast({literal})"
            );
            let code = Arc::new(compile(&source, &manifest));
            let mut vm = Vm::new(code.clone()).expect("VM");
            let mut failed = false;
            loop {
                let encoded =
                    crate::hson::to_string(&vm.snapshot()).expect("generic snapshot encodes");
                let snapshot = crate::hson::from_str(&encoded).expect("generic snapshot decodes");
                vm = Vm::restore(code.clone(), snapshot).expect("restore");
                match vm.step_with_budget(&mut 1) {
                    Err(VmError::CastFailed(_)) => {
                        failed = true;
                        break;
                    }
                    Ok(Some(VmEvent::Completed(_))) => break,
                    Ok(_) => {}
                    Err(error) => panic!("{error}"),
                }
            }
            assert_eq!(failed, literal != "1");
        }
    }

    #[test]
    fn generic_wrapper_checks_host_result_after_restoring_a_wait() {
        let manifest = BuiltinManifest::new([("raw", BuiltinId(91))]);
        let code = Arc::new(compile(
            "fn open<T>() -> T { raw() as! T }\nglobal var result: Int = open()",
            &manifest,
        ));
        let mut vm = Vm::new(code.clone()).expect("VM");
        loop {
            if matches!(vm.step().expect("wait for raw API"), Some(VmEvent::Call(_))) {
                break;
            }
        }
        let encoded = crate::hson::to_string(&vm.snapshot()).expect("waiting state serializes");
        for value in [Value::Number(1.0), Value::String("bob".into())] {
            let mut restored = Vm::restore(
                code.clone(),
                crate::hson::from_str(&encoded).expect("snapshot"),
            )
            .expect("restore");
            restored.resume(value.clone()).expect("resume raw API");
            let mut failed = false;
            loop {
                match restored.step() {
                    Err(VmError::CastFailed(_)) => {
                        failed = true;
                        break;
                    }
                    Ok(Some(VmEvent::Completed(_))) => break,
                    Ok(_) => {}
                    Err(error) => panic!("{error}"),
                }
            }
            assert_eq!(failed, matches!(value, Value::String(_)));
        }
    }

    #[test]
    fn returned_closures_capture_generic_types_not_the_callers_types() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for literal in ["1", "\"alice\""] {
            let source = format!(
                "fn converter<T>() -> (Any) -> T {{ {{ value: Any -> value as! T }} }}\nlet convert: (Any) -> Int = converter()\nlet result: Int = convert({literal})"
            );
            let code = Arc::new(compile(&source, &manifest));
            let mut vm = Vm::new(code.clone()).expect("VM");
            let mut failed = false;
            loop {
                let encoded = crate::hson::to_string(&vm.snapshot()).expect("snapshot");
                vm = Vm::restore(
                    code.clone(),
                    crate::hson::from_str(&encoded).expect("snapshot decodes"),
                )
                .expect("restore");
                match vm.step_with_budget(&mut 1) {
                    Err(VmError::CastFailed(_)) => {
                        failed = true;
                        break;
                    }
                    Ok(Some(VmEvent::Completed(_))) => break,
                    Ok(_) => {}
                    Err(error) => panic!("{error}"),
                }
            }
            assert_eq!(failed, literal != "1");
        }
    }

    #[test]
    fn readonly_globals_reject_rebinding_and_mutation_through_aliases_after_restore() {
        for update in [
            "player = .{ name: \"bob\" }",
            "player.name = \"bob\"",
            "let alias = player; alias.name = \"bob\"",
            "fn change(value: .{ name: String }) { value.name = \"bob\" }; change(player)",
        ] {
            let source = format!("global var player = .{{ name: \"alice\" }}; {update}");
            let code = Arc::new(compile(
                &source,
                &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
            ));
            let mut vm = Vm::new(code.clone()).expect("VM");
            assert!(matches!(
                vm.step().expect("initializer"),
                Some(VmEvent::Statement(_))
            ));
            vm.set_read_only_globals(["player".into()].into_iter().collect());
            let mut vm = Vm::restore(code, vm.snapshot()).expect("restore readonly policy");
            loop {
                match vm.step() {
                    Err(VmError::ReadOnlyValue) => break,
                    Ok(Some(VmEvent::Statement(_))) => {}
                    other => panic!("expected readonly error for {update}, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn function_entry_rejects_wrong_host_values_without_coercion() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = Arc::new(compile(
            "fn identity(value: Int) -> Int { value }",
            &manifest,
        ));
        for value in [Value::String("alice".into()), Value::Number(1.5)] {
            let mut vm =
                Vm::from_function(bytecode.clone(), 0, vec![value]).expect("arity is valid");
            let before = vm.snapshot();
            assert_eq!(
                vm.step(),
                Err(VmError::ArgumentTypeMismatch {
                    argument: 1,
                    expected: "Int".into(),
                })
            );
            assert_eq!(
                vm.snapshot(),
                before,
                "invalid arguments must not execute the body"
            );
        }
        let mut vm =
            Vm::from_function(bytecode, 0, vec![Value::Number(2.0)]).expect("arity is valid");
        assert!(vm.step().is_ok());
    }

    #[test]
    fn function_entry_checks_nested_values_after_snapshot_restore() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = Arc::new(compile(
            "fn inspect(value: List<(Int, String?)>) { }",
            &manifest,
        ));
        for (value, valid) in [
            (Value::Optional(None), true),
            (
                Value::Optional(Some(Box::new(Value::String("bob".into())))),
                true,
            ),
            (Value::Optional(Some(Box::new(Value::Bool(false)))), false),
        ] {
            let vm = Vm::from_function(
                bytecode.clone(),
                0,
                vec![Value::List(vec![Value::Tuple(vec![
                    Value::Number(1.0),
                    value,
                ])])],
            )
            .expect("arity is valid");
            let mut restored =
                Vm::restore(bytecode.clone(), vm.snapshot()).expect("entry restores");
            assert_eq!(restored.step().is_ok(), valid);
        }
    }

    #[test]
    fn any_function_call_requires_an_explicit_cast() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let syntax = parse_program(
            r#"
            fn identity(value: Int) -> Int { value }
            let dynamic: Any = identity
            dynamic("alice")
        "#,
        )
        .expect("source parses");
        let errors =
            compile_with_manifest(&syntax, 91, &manifest).expect_err("Any is not callable");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("cannot call Any"))
        );
    }

    #[test]
    fn proven_any_cast_preserves_the_value() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            "let a: Int = 1\nlet b: Any = a\nlet c: Int = b as Int",
            &manifest,
        );
        let result = bytecode
            .locals
            .iter()
            .position(|symbol| bytecode.symbols.resolve(*symbol) == Some("c"))
            .expect("result local exists");
        let mut vm = Vm::new(bytecode).expect("program initializes");
        while vm.step().expect("proven cast executes").is_some() {}
        assert_eq!(vm.locals[result], Value::Number(1.0));
    }

    #[test]
    fn explicitly_cast_lambda_retains_its_signature() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for declaration in [
            "let callback = { value: Int -> value }",
            "let callback: (Int) -> Int = { value -> value }",
        ] {
            let bytecode = compile(
                &format!(
                    "{declaration}\nlet erased: Any = callback\nlet typed = erased as (Int) -> Int\ntyped(1)"
                ),
                &manifest,
            );
            assert_eq!(
                bytecode.regions[0].signature.parameters,
                vec![crate::ScriptType::Int]
            );
            assert_eq!(bytecode.regions[0].signature.result, crate::ScriptType::Int);
            let mut vm = Vm::new(bytecode).expect("program initializes");
            while vm
                .step()
                .expect("explicitly cast lambda executes")
                .is_some()
            {}
        }
    }

    #[test]
    fn restored_closure_entry_validates_host_arguments() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = Arc::new(compile(
            "let callback = { value: String -> value }",
            &manifest,
        ));
        let mut vm = Vm::new(bytecode.clone()).expect("program initializes");
        while vm.step().expect("closure initializes").is_some() {}
        let closure = vm
            .locals
            .iter()
            .find(|value| matches!(value, Value::Closure { .. }))
            .expect("closure is stored")
            .clone();
        for (value, valid) in [
            (Value::String("bob".into()), true),
            (Value::Bool(false), false),
        ] {
            let vm =
                Vm::from_callable(bytecode.clone(), &closure, vec![value]).expect("arity matches");
            let mut restored =
                Vm::restore(bytecode.clone(), vm.snapshot()).expect("closure entry restores");
            assert_eq!(restored.step().is_ok(), valid);
        }
    }

    #[test]
    fn instruction_budget_yields_without_a_host_wait_or_commit() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let program = Arc::new(compile("while true {}", &manifest));
        let mut vm = Vm::new(program.clone()).expect("loop compiles");
        let mut budget = 20;
        assert_eq!(
            vm.step_with_budget(&mut budget),
            Ok(Some(VmEvent::BudgetExhausted))
        );
        assert_eq!(budget, 0);
        assert_eq!(vm.status(), VmStatus::Ready);
        let snapshot = vm.snapshot();
        assert_eq!(
            vm.step_with_budget(&mut budget),
            Ok(Some(VmEvent::BudgetExhausted))
        );
        assert_eq!(
            vm.snapshot(),
            snapshot,
            "zero budget must not execute instructions"
        );
        let mut restored = Vm::restore(program, snapshot).expect("yielded state restores");
        assert_eq!(
            restored.step_with_budget(&mut 20),
            Ok(Some(VmEvent::BudgetExhausted))
        );
    }

    #[test]
    fn repeated_literals_share_a_module_pool_across_functions() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let source =
            "fn greeting() -> String { \"Hello, alice 🌸\" }\nlet label = \"Hello, alice 🌸\"";
        let first = compile(source, &manifest);
        let second = compile(source, &manifest);
        assert_eq!(first, second, "pool IDs must be deterministic");
        assert_eq!(first.strings.strings(), &["Hello, alice 🌸"]);
        let ids = first
            .instructions
            .iter()
            .chain(
                first
                    .functions
                    .iter()
                    .flat_map(|function| &function.instructions),
            )
            .filter_map(|instruction| match instruction {
                Instruction::Constant {
                    value: Constant::String(id),
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(ids.len() >= 2);
        assert!(ids.iter().all(|id| *id == StringId(0)));
    }

    #[test]
    fn invocations_share_bytecode_but_not_registers() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let program = Arc::new(compile("let label = \"alice\"", &manifest));
        let mut first = Vm::new(program.clone()).expect("first VM initializes");
        let second = Vm::new(program.clone()).expect("second VM initializes");
        assert!(Arc::ptr_eq(&first.bytecode, &second.bytecode));
        first.step().expect("literal statement executes");
        assert_eq!(second.pc, 0);
        let restored = Vm::restore(program, first.snapshot()).expect("snapshot restores");
        assert!(Arc::ptr_eq(&first.bytecode, &restored.bytecode));
        assert_eq!(restored.snapshot(), first.snapshot());
    }

    #[test]
    fn invalid_literal_pool_reference_returns_an_error() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let mut program = compile("\"alice\"", &manifest);
        let instruction = program
            .instructions
            .iter_mut()
            .find(|instruction| {
                matches!(
                    instruction,
                    Instruction::Constant {
                        value: Constant::String(_),
                        ..
                    }
                )
            })
            .expect("literal instruction exists");
        if let Instruction::Constant { value, .. } = instruction {
            *value = Constant::String(StringId(u32::MAX));
        }
        let mut vm = Vm::new(program).expect("VM initializes");
        assert_eq!(vm.step(), Err(VmError::UnknownString(StringId(u32::MAX))));
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
                global var player = .{ stats: .{ health: 1 } }
                var index = 0
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
        let Value::Map(player) = vm
            .export_value(vm.global("player").expect("player global exists"))
            .expect("record exports")
        else {
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
        let bytecode = compile("var value = nativeValue()\nvalue += 1", &manifest);
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
                    variadic: None,
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
                objects: None,
                module: None,
                region: 0,
                ref captures,
                ..
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
                    variadic: None,
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
    fn explicit_returns_exit_only_the_current_callable_and_restore() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = Arc::new(compile(
            r#"
            fn name(value: String) -> String {
                if value == "alice" { return "" }
                return value
            }
            fn number() -> Int {
                var i = 0
                while i < 4 {
                    if i == 2 { return i }
                    i += 1
                }
                9
            }
            fn outer() -> Int {
                let inner: () -> String = { return "bob" }
                let label = inner()
                return number()
            }
            fn inferred() { return 7 }
            fn varied(flag: Bool) -> Any {
                if flag { return 1 }
                return "alice"
            }
            fn branching(flag: Bool) -> Int {
                if flag { return 3 } else { return 4 }
            }
            fn noop() -> Unit { return; unreachable() }
            noop()
            global let result = outer()
            global let label = name("alice")
            global let inferredResult = inferred()
            global let branch = branching(false)
            global let flexible: Any = varied(false)
        "#,
            &manifest,
        ));
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        loop {
            let snapshot = vm.snapshot();
            vm = Vm::restore(bytecode.clone(), snapshot).expect("return frame restores");
            if matches!(
                vm.step_with_budget(&mut 1).expect("return executes"),
                Some(VmEvent::Completed(_))
            ) {
                break;
            }
        }
        assert_eq!(vm.global("result"), Some(&Value::Number(2.0)));
        assert_eq!(vm.global("label"), Some(&Value::String(String::new())));
        assert_eq!(vm.global("inferredResult"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("branch"), Some(&Value::Number(4.0)));
        assert_eq!(vm.global("flexible"), Some(&Value::String("alice".into())));
    }

    #[test]
    fn explicit_returns_require_the_declared_type_and_all_paths() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "return 1",
            "fn invalid() -> String { return 1 }",
            "fn invalid() -> Int { return }",
            "fn invalid() -> Never { return }",
            "fn invalid(flag: Bool) -> Int { if flag { return 1 } }",
            "let f: () -> Int = { return \"alice\" }",
        ] {
            let ast = parse_program(source).expect("return syntax parses");
            assert!(
                compile_with_manifest(&ast, 91, &manifest).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn named_functions_are_first_class_and_dynamically_callable() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            "fn increment(value: Int) -> Int { value + 1 }\nlet callable = increment\nglobal var result = callable(2)",
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
                    variadic: None,
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

    #[test]
    fn cast_modes_execute_with_checked_runtime_semantics() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            "global var rounded: Int = 3.75 as Int\nglobal var absent: String? = 4 as? String",
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("casts execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("rounded"), Some(&Value::Number(3.0)));
        assert_eq!(vm.global("absent"), Some(&Value::Optional(None)));
    }

    #[test]
    fn optional_primitives_preserve_nested_some_and_none() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            "global var present: String? = \"alice\"\nglobal var nested: Optional<Optional<String>> = .some(null)\nglobal var empty: Optional<Optional<String>> = .none",
            &manifest,
        );
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        while !matches!(
            vm.step().expect("optional values execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(
            vm.global("present"),
            Some(&Value::Optional(Some(Box::new(Value::String(
                "alice".into()
            )))))
        );
        assert_eq!(
            vm.global("nested"),
            Some(&Value::Optional(Some(Box::new(Value::Optional(None)))))
        );
        assert_eq!(vm.global("empty"), Some(&Value::Optional(None)));

        let restored = Vm::restore(bytecode, vm.snapshot()).expect("snapshot restores");
        assert_eq!(
            restored.global("nested"),
            Some(&Value::Optional(Some(Box::new(Value::Optional(None)))))
        );
    }

    #[test]
    fn collection_cast_validates_nested_heap_objects_without_copying_identity() {
        let mut heap = crate::objects::ObjectHeap::default();
        let value = heap.import(Value::List(vec![Value::List(vec![Value::Map(
            BTreeMap::from([("name".into(), Value::String("alice".into()))]),
        )])]));
        let target = crate::ScriptType::List(Box::new(crate::ScriptType::List(Box::new(
            crate::ScriptType::Record(BTreeMap::from([("name".into(), crate::ScriptType::String)])),
        ))));
        assert_eq!(
            cast_value_with_heap(&value, &target, &heap).expect("nested cast"),
            value
        );
        let bad = heap.import(Value::List(vec![Value::List(vec![Value::Map(
            BTreeMap::from([("name".into(), Value::Number(1.0))]),
        )])]));
        assert!(cast_value_with_heap(&bad, &target, &heap).is_err());
    }

    #[test]
    fn narrowing_unwraps_an_optional_before_the_native_boundary() {
        let consume = BuiltinId(20);
        let manifest = BuiltinManifest::new([("consume", consume)]).with_type_metadata(
            crate::SymbolManifest::default(),
            BTreeMap::from([(
                consume,
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![crate::ScriptType::String],
                    variadic: None,
                    result: crate::ScriptType::Unit,
                },
            )]),
            Vec::new(),
        );
        let bytecode = compile(
            "let name: String? = \"alice\"\nif name != null { consume(name) }",
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        let Some(VmEvent::Call(call)) = vm.step().expect("condition and call execute") else {
            panic!("expected native call")
        };
        assert_eq!(call.arguments[0].value, Value::String("alice".into()));
    }

    #[test]
    fn forced_cast_failure_is_a_runtime_error() {
        let bytecode = compile(
            "global var result: Int = \"alice\" as! Int",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        assert!(matches!(vm.step(), Err(VmError::CastFailed(_))));
    }

    #[test]
    fn primitive_to_string_runs_in_the_script_call_frame() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let mut vm = Vm::new(compile(
            "fn label(value: Float) -> String { let rounded = value * 2; rounded.toString() }\nglobal let result = label(0.25)\nglobal let integer = 12.toString()\nglobal let flag = true.toString()",
            &manifest,
        )).expect("VM initializes");
        while !matches!(
            vm.step().expect("string conversion succeeds"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::String("0.5".into())));
        assert_eq!(vm.global("integer"), Some(&Value::String("12".into())));
        assert_eq!(vm.global("flag"), Some(&Value::String("true".into())));
    }

    #[test]
    fn explicit_to_int_truncates_and_rejects_invalid_values() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let mut vm = Vm::new(compile(
            "let value = -3.8\nglobal var result = value.toInt()",
            &manifest,
        ))
        .expect("VM initializes");
        while !matches!(
            vm.step().expect("conversion succeeds"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(-3.0)));
        assert!(cast_value(&Value::Number(f64::INFINITY), &crate::ScriptType::Int).is_err());
        assert!(cast_value(&Value::Number(1e30), &crate::ScriptType::Int).is_err());
    }

    #[test]
    fn inline_hint_preserves_argument_evaluation_and_removes_small_calls() {
        let manifest = BuiltinManifest::new([("host", BuiltinId(1))]);
        let code = compile(
            r#"
            @inline
            fn twice(value: Int) -> Int { value + value }
            @inline
            fn first(left: Int, right: Int) -> Int { left }
            fn argument(value: Int) -> Int { host(value); value }
            global let result = first(twice(argument(3)), argument(4))
        "#,
            &manifest,
        );
        for instruction in &code.instructions {
            if let Instruction::Call { function, .. } = instruction {
                assert!(
                    !matches!(code.symbols.resolve(*function), Some("twice" | "first")),
                    "small annotated calls should be inlined"
                );
            }
        }
        let mut vm = Vm::new(code.clone()).expect("VM initializes");
        let mut calls = Vec::new();
        loop {
            match vm.step().expect("inline program executes") {
                Some(VmEvent::Call(call)) => {
                    calls.push(call.arguments[0].value.clone());
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("argument wait restores");
                    vm.resume(Value::Unit).expect("host resumes");
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(calls, vec![Value::Number(3.0), Value::Number(4.0)]);
        assert_eq!(vm.global("result"), Some(&Value::Number(6.0)));
    }

    #[test]
    fn inline_hint_keeps_effectful_functions_as_ordinary_calls() {
        let manifest = BuiltinManifest::new([("host", BuiltinId(1))]);
        let code = compile(
            "@inline\nfn effect(value: Int) -> Int { host(value); value }\neffect(2)",
            &manifest,
        );
        assert!(code.instructions.iter().any(|instruction| matches!(instruction, Instruction::Call { function, .. } if code.symbols.resolve(*function) == Some("effect"))));
    }

    #[test]
    fn compiler_intrinsics_are_only_available_to_injected_core_functions() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "intrinsics.floatToInt(1.5)",
            "fn bypass() { intrinsics.int.add(1, 2) }\nbypass()",
            "fn panic(message: String) -> Never { intrinsics.panic(message) }\npanic(\"alice\")",
            "extend String { fn bypass(self) { intrinsics.toString(self) } }\n\"alice\".bypass()",
            "@compilerIntrinsics\nfn bypass() { intrinsics.intToFloat(1) }\nbypass()",
        ] {
            let syntax = crate::parse_program(source).expect("syntax parses");
            let errors = compile_with_manifest(&syntax, 0, &manifest)
                .expect_err("user functions must not gain intrinsic access");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("capability")),
                "{errors:?}"
            );
        }
        let mut syntax = crate::parse_program("fn bypass() { intrinsics.int.add(1, 2) }\nbypass()")
            .expect("syntax parses");
        if let crate::Stmt::Function {
            compiler_intrinsics,
            ..
        } = &mut syntax.statements[0]
        {
            *compiler_intrinsics = true;
        }
        assert!(
            compile_with_manifest(&syntax, 0, &manifest).is_err(),
            "normalization must discard forged AST privileges"
        );
    }

    #[test]
    fn explicit_to_float_uses_the_core_method_and_intrinsic() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let mut vm = Vm::new(compile(
            "let n: Int = 7\nglobal var result: Float = n.toFloat()",
            &manifest,
        ))
        .expect("VM initializes");
        while !matches!(
            vm.step().expect("conversion executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(7.0)));
    }

    #[test]
    fn script_operators_dispatch_by_receiver_and_restore_host_waits() {
        let manifest = BuiltinManifest::new([("host", BuiltinId(1))]);
        let bytecode = compile(
            r#"
            struct Actor { name: String }
            extend Actor: Colon<TextTemplate> {
                type Output = Unit
                fn colon(self, text: TextTemplate) { host(self.name, text) }
            }
            extend String: Colon<TextTemplate> {
                type Output = Unit
                fn colon(self, text: TextTemplate) { host(self, text) }
            }
            let alice = Actor.{ name: "alice" }
            alice: "Hello ${1 + 2}"
            "bob": "World"
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        let mut calls = Vec::new();
        loop {
            match vm.step().expect("operator executes") {
                Some(VmEvent::Call(call)) => {
                    calls.push(
                        call.arguments
                            .iter()
                            .map(|arg| arg.value.clone())
                            .collect::<Vec<_>>(),
                    );
                    vm = Vm::restore(bytecode.clone(), vm.snapshot())
                        .expect("operator frame restores");
                    vm.resume(Value::Unit).expect("host resumes");
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(calls.len(), 2);
        for (call, (speaker, source)) in calls
            .iter()
            .zip([("alice", "Hello ${1 + 2}"), ("bob", "World")])
        {
            assert_eq!(call[0], Value::String(speaker.into()));
            assert!(matches!(&call[1], Value::TextTemplate(template) if template.source == source));
        }
    }

    #[test]
    fn protocol_methods_are_callable_by_name_from_other_implementations() {
        let manifest = BuiltinManifest::new([("print", BuiltinId(1))]);
        let code = compile(
            r#"
            struct Player { name: String? }
            protocol Test { fn test(self) -> Unit }
            extend Player: Test {
                fn test(self) { print("name: ${self.name ?: "alice"}") }
            }
            extend Player: Colon<String> {
                type Output = Unit
                fn colon(self, rhs: String) {
                    self.test()
                    print("rhs: ${rhs}")
                }
            }
            let player = Player.{ name: null }
            player: "bob"
            player.test()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM initializes");
        let mut output = Vec::new();
        loop {
            match vm.step().expect("protocol method executes") {
                Some(VmEvent::Call(call)) => {
                    output.push(call.arguments[0].value.clone());
                    vm = Vm::restore(code.clone(), vm.snapshot())
                        .expect("cross-protocol frame restores");
                    vm.resume(Value::Unit).expect("host resumes");
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(
            output,
            vec![
                Value::String("name: alice".into()),
                Value::String("rhs: bob".into()),
                Value::String("name: alice".into())
            ]
        );
    }

    #[test]
    fn primitive_protocol_methods_are_loaded_for_named_calls() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile(
            r#"
            let number = 3
            let scale: Float = 2
            global let sum = number.add(4)
            global let product = scale.multiply(3)
            global let greeting = "alice".add("bob")
            global let same = "alice".equal("alice")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code).expect("VM initializes");
        while !matches!(
            vm.step().expect("named primitive method executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("sum"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("product"), Some(&Value::Number(6.0)));
        assert_eq!(
            vm.global("greeting"),
            Some(&Value::String("alicebob".into()))
        );
        assert_eq!(vm.global("same"), Some(&Value::Bool(true)));
    }

    #[test]
    fn ambiguous_protocol_method_names_are_compile_errors() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let syntax = crate::parse_program(
            r#"
            protocol First { fn test(self) -> Unit }
            protocol Second { fn test(self) -> Unit }
            extend String: First { fn test(self) {} }
            extend String: Second { fn test(self) {} }
            "alice".test()
        "#,
        )
        .expect("syntax parses");
        let errors = compile_with_manifest(&syntax, 0, &manifest)
            .expect_err("ambiguous calls must not pick by declaration order");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("ambiguous protocol method"))
        );
    }

    #[test]
    fn protocol_bounds_pass_resumable_witnesses_through_generic_calls() {
        let manifest = BuiltinManifest::new([("host", BuiltinId(1))]);
        let code = compile(
            r#"
            protocol Test { fn test(self) -> String }
            extend String: Test { fn test(self) -> String { host(self); self } }
            fn what<T: Test>(t: T) { t.test() }
            fn forward<T: Test>(t: T) -> String { what(t) }
            global let result = forward("alice")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM initializes");
        loop {
            match vm.step().expect("bounded generic function executes") {
                Some(VmEvent::Call(_)) => {
                    vm = Vm::restore(code.clone(), vm.snapshot()).expect("witness frame restores");
                    vm.resume(Value::Unit).expect("host resumes");
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(vm.global("result"), Some(&Value::String("alice".into())));
    }

    #[test]
    fn generic_protocol_arguments_multiple_bounds_and_closures_are_checked() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile(
            r#"
            protocol Label { fn label(self) -> String }
            protocol Append<Rhs> { fn append(self, rhs: Rhs) -> String }
            extend String: Label { fn label(self) -> String { self } }
            extend String: Append<Int> { fn append(self, rhs: Int) -> String { self + rhs.toString() } }
            fn make<T: Label + Append<Int>>(value: T) -> () -> String {
                { value.label() + value.append(2) }
            }
            struct Helper {}
            extend Helper {
                fn apply<T: Label>(self, value: T) -> String { value.label() }
            }
            let callback = make("alice")
            global let result = callback()
            global let methodResult = Helper.{}.apply("bob")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code).expect("VM initializes");
        while !matches!(
            vm.step().expect("constrained closure executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(
            vm.global("result"),
            Some(&Value::String("alicealice2".into()))
        );
        assert_eq!(
            vm.global("methodResult"),
            Some(&Value::String("bob".into()))
        );
    }

    #[test]
    fn protocol_bounds_reject_missing_evidence_before_execution() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "protocol Test { fn test(self) -> String }\nfn what<T: Test>(t: T) -> String { t.test() }\nwhat(1)",
            "fn what<T: Missing>(t: T) {}",
            "fn what<T>(t: T) { t.test() }",
            "protocol Test { fn test(self) -> String }\nfn what<T: Test>(t: T) -> String { t.test() }\nlet callback = what",
            "protocol Test { fn test(self) -> String }\nfn what<T: Test>(t: T) -> String { t.test() }\nfn forward<T>(t: T) -> String { what(t) }",
        ] {
            let syntax = crate::parse_program(source).expect("syntax parses");
            assert!(
                compile_with_manifest(&syntax, 0, &manifest).is_err(),
                "must reject {source}"
            );
        }
    }

    #[test]
    fn protocol_associated_output_is_checked_and_custom_addition_executes() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile(
            r#"
            struct Counter { value: Int }
            extend Counter: Add<Int> {
                type Output = String
                fn add(self, rhs: Int) -> Self.Output { (self.value + rhs).toString() }
            }
            extend Counter: Negate {
                type Output = Int
                fn negate(self) -> Output { -self.value }
            }
            global let result = Counter.{ value: 2 } + 3
            global let negative = -Counter.{ value: 2 }
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code).expect("VM initializes");
        while !matches!(
            vm.step().expect("protocol call executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::String("5".into())));
        assert_eq!(vm.global("negative"), Some(&Value::Number(-2.0)));
        assert!(
            crate::parse_program("extend String { operator fn colon(self, rhs: String) {} }")
                .is_err()
        );
        for source in [
            "protocol Invalid<T> { type T; fn call(self, rhs: T) -> T }",
            "extend String: Colon<String> { type Output = Int; fn colon(self, rhs: String) { rhs } }",
            "extend String: Colon { type Output = Unit; fn colon(self, rhs: String) {} }",
            "extend String: Colon<String> { type Output = Unit; fn colon(self, rhs: String) {} }\nextend String: Colon<String> { type Output = Unit; fn colon(self, rhs: String) {} }",
        ] {
            let program = crate::parse_program(source).expect("syntax parses");
            assert!(
                compile_with_manifest(&program, 0, &manifest).is_err(),
                "must reject {source}"
            );
        }
    }

    #[test]
    fn script_operator_signatures_are_checked_statically() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "extend String: Colon<String> { type Output = Unit; fn colon(self, value: String) {} }\n\"alice\": 42",
            "extend String: Colon<String> { type Output = Unit; fn colon(value: String) {} }",
            "extend String: Colon<String> { type Output = Unit; fn colon(self, value) {} }",
            "extend String: Colon<String> { type Output = Unit; fn unknown(self, value: String) {} }",
            "extend String: Colon<String> { fn colon(self, value: String) {} }",
            "extend String: Colon<String> { type Output = Int; fn colon(self, value: String) -> String { value } }",
        ] {
            let syntax = crate::parse_program(source).expect("operator syntax parses");
            assert!(
                compile_with_manifest(&syntax, 0, &manifest).is_err(),
                "must reject {source}"
            );
        }
    }

    #[test]
    fn extension_methods_receive_self_and_use_the_ordinary_call_path() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
            type Player = .{ score: Int }
            extend Player {
                fn some_fn(self) { () }
                fn scorePlus(self, extra: Int) -> Int { self.score + extra }
            }
            let player = Player.{ score: 12 }
            player.some_fn()
            global var result = player.scorePlus(3)
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("method executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn rejects_invalid_constants_properties_and_callable_arguments() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "const score = todo()",
            "global const score = .{ value: 1 }",
            "const score = 1\nscore = 2",
            "type Player = .{}\nextend Player { var score: Int { 1 } }\nlet alice = Player.{}\nalice.score = 2",
            "let callback: (Int) -> Int = { value -> value }\ncallback(\"wrong\")",
            "let callback: (Int) -> Int = { value -> value }\ncallback()",
            "let callback: (Int) -> Int = { value -> \"wrong\" }",
        ] {
            let syntax =
                crate::parse_program(source).expect("invalid program is syntactically valid");
            assert!(
                compile_with_manifest(&syntax, 0, &manifest).is_err(),
                "must reject {source}"
            );
        }
        for source in [
            "type Player = .{}\nextend Player { let score = 1 }",
            "type Player = .{}\nextend Player { var score: Int { get { 1 } set { 2 } } }",
            "type Player = .{}\nextend Player { var score: Int { get { 1 } set() { 2 } } }",
        ] {
            assert!(
                crate::parse_program(source).is_err(),
                "must reject {source}"
            );
        }
    }

    #[test]
    fn tuple_literals_use_element_type_context_without_converting_int_bindings() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let source = "global let pair: (Float, String) = (1, \"alice\")";
        let mut vm = Vm::new(compile(source, &manifest)).expect("VM initializes");
        while !matches!(
            vm.step().expect("tuple executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(
            vm.global("pair"),
            Some(&Value::Tuple(vec![
                Value::Number(1.0),
                Value::String("alice".into())
            ]))
        );
        for source in [
            "let integer: Int = 1\nlet pair: (Float, String) = (integer, \"alice\")",
            "let pair: (Int, String) = (1, 2)",
            "let pair: (Int, String) = (1, \"alice\", 3)",
        ] {
            let program = crate::parse_program(source).expect("source parses");
            assert!(compile_with_manifest(&program, 0, &manifest).is_err());
        }
    }

    #[test]
    fn constants_and_computed_properties_use_shared_receivers() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
            const base = 2 * 3
            global const limit = base + 4
            type Player = .{ score: Int }
            extend Player {
                const A = 7
                var doubled: Int { self.score * 2 }
                var current: Int {
                    get { self.score }
                    set(value) { self.score = value }
                }
            }
            let alice = Player.{ score: 1 }
            let bob = alice
            bob.current = limit
            global var result = alice.doubled + Player.A
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("properties execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(27.0)));
    }

    #[test]
    fn callable_annotations_support_currying_and_unit_alias() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
            fn add(a: Int) -> (Int) -> Int { { b -> a + b } }
            let factory: (Int) -> (Int) -> Int = add
            let plusTwo: (Int) -> Int = factory(2)
            global var result = plusTwo(3)
            let consume: (Int) -> () = { value -> let copy = value }
            consume(1)
            let nothing: Unit = ()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("typed closures execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(5.0)));
    }

    #[test]
    fn nominal_structs_keep_identity_across_aliases_calls_closures_and_restore() {
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let bytecode = compile(
            r#"
            struct Player { score: Int }
            extend Player { fn add(self, n: Int) { self.score += n } }
            global var player = Player.{ score: 1 }
            let alias = player
            let modify = { alias.add(2) }
            checkpoint()
            modify()
            fn change(p: Player) { p.score += 4 }
            change(player)
            global var result = alias.score
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        loop {
            if matches!(
                vm.step().expect("checkpoint executes"),
                Some(VmEvent::Call(_))
            ) {
                break;
            }
        }
        let encoded = crate::hson::to_string(&vm.snapshot()).expect("snapshot serializes");
        let snapshot = crate::hson::from_str(&encoded).expect("snapshot deserializes");
        let mut vm = Vm::restore(bytecode, snapshot).expect("references restore");
        vm.objects.allocate(Value::Map(BTreeMap::new()));
        assert!(vm.collect_objects(&[]).expect("paused VM roots are traced") >= 1);
        vm.resume(Value::Unit).expect("host resumes");
        while !matches!(
            vm.step().expect("aliases execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(7.0)));
    }

    #[test]
    fn static_methods_use_type_names_without_a_receiver() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
            type Player = .{}
            extend Player {
                fn name() -> String { "Player" }
                fn add(a: Int, b: Int) -> Int { a + b }
                fn instance(self) -> Int { 7 }
            }
            global var label = Player.name()
            global var total = Player.add(2, 3)
            let player = Player.{}
            global var instance = player.instance()
            let nameFunction = Player.name
            global var indirect = nameFunction()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("static and instance calls execute"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("label"), Some(&Value::String("Player".into())));
        assert_eq!(vm.global("total"), Some(&Value::Number(5.0)));
        assert_eq!(vm.global("instance"), Some(&Value::Number(7.0)));
        assert_eq!(vm.global("indirect"), Some(&Value::String("Player".into())));
    }

    #[test]
    fn static_method_execution_can_be_restored_at_a_host_call() {
        let manifest = BuiltinManifest::new([("host", BuiltinId(1))]);
        let bytecode = compile(
            r#"
            type Player = .{}
            extend Player { fn score(n: Int) -> Int { host(n) n + 1 } }
            global var result = Player.score(4)
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Call(_)))));
        let mut vm = Vm::restore(bytecode, vm.snapshot()).expect("method frame restores");
        vm.resume(Value::Unit).expect("host resumes");
        while !matches!(
            vm.step().expect("restored method executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(5.0)));
    }

    #[test]
    fn primitive_methods_do_not_box_their_receiver() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile(
            r#"
            extend Int { fn doubled(self) -> Int { self * 2 } }
            let value: Int = 3
            global var result = value.doubled()
        "#,
            &manifest,
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("method executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::Number(6.0)));
    }

    #[test]
    fn panic_uses_a_runtime_string_and_core_helpers_are_ordinary_functions() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let bytecode = compile("let reason = \"failure\"\npanic(reason)", &manifest);
        assert!(
            bytecode
                .functions
                .iter()
                .any(|function| bytecode.symbols.resolve(function.name) == Some("panic"))
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        loop {
            match vm.step() {
                Ok(Some(VmEvent::Statement(_))) => {}
                Err(VmError::Panic { message, .. }) => {
                    assert_eq!(message, "failure");
                    break;
                }
                result => panic!("unexpected result: {result:?}"),
            }
        }
    }

    #[test]
    fn panic_locations_track_user_calls_not_core_library_offsets() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for (source, call) in [
            (
                "let name = \"élise\"\npanic(\"failure\")",
                "panic(\"failure\")",
            ),
            ("fn stop() -> Never { todo() }\nstop()", "todo()"),
            ("unreachable()", "unreachable()"),
        ] {
            let mut vm = Vm::new(compile(source, &manifest)).expect("VM initializes");
            loop {
                match vm.step() {
                    Ok(Some(VmEvent::Statement(_))) => continue,
                    Err(error @ VmError::Panic { span, .. }) => {
                        assert_eq!(&source[span.range()], call);
                        let mut sources = crate::SourceMap::new();
                        let id = sources.insert("entry.hks", source);
                        let rendered = crate::render_diagnostics(
                            &[error.diagnostic(id)],
                            &sources,
                            crate::RenderOptions::plain(),
                        );
                        assert!(rendered.contains("HKS-PANIC"));
                        assert!(rendered.contains("entry.hks"));
                        assert!(rendered.contains(call));
                        break;
                    }
                    result => panic!("expected located panic, got {result:?}"),
                }
            }
        }
    }

    #[test]
    fn panic_location_survives_a_saved_call_frame() {
        let source = "fn stop() -> Never { checkpoint(); panic(\"failure\") }\nstop()";
        let manifest = BuiltinManifest::new([("checkpoint", BuiltinId(1))]);
        let mut bytecode = compile(source, &manifest);
        bytecode.debug.source = Some(crate::debug::DebugSource {
            path: "entry.hks".into(),
            text: source.into(),
        });
        let mut vm = Vm::new(bytecode.clone()).expect("VM initializes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Call(_)))));
        let mut restored = Vm::restore(bytecode, vm.snapshot()).expect("call frame restores");
        restored.resume(Value::Unit).expect("checkpoint resumes");
        loop {
            match restored.step() {
                Ok(Some(VmEvent::Statement(_))) => continue,
                Err(error @ VmError::Panic { span, .. }) => {
                    assert_eq!(&source[span.range()], "panic(\"failure\")");
                    let report = error
                        .render_diagnostic(crate::RenderOptions::plain())
                        .expect("source is available");
                    assert!(report.contains("entry.hks:1:"));
                    assert!(report.contains("failure"));
                    break;
                }
                result => panic!("expected a located panic, got {result:?}"),
            }
        }
    }

    #[test]
    fn never_intrinsics_trap_and_never_host_returns_are_guarded() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        for source in [
            "unreachable()",
            "todo()",
            "fn stop() -> Never { todo() }\nstop()",
        ] {
            let mut vm = Vm::new(compile(source, &manifest)).expect("VM initializes");
            assert!(matches!(vm.step(), Err(VmError::Panic { .. })), "{source}");
        }

        let mut registry = crate::native::NativeRegistry::<()>::new();
        registry
            .register_fn(
                "stop",
                |_: &mut ()| -> Result<crate::native::Never, crate::native::NativeError> {
                    Err(crate::native::NativeError::message(
                        "host must transfer control",
                    ))
                },
            )
            .expect("native registers");
        registry
            .set_signature_for(
                "stop",
                crate::FunctionSignature {
                    receiver: None,
                    parameters: Vec::new(),
                    variadic: None,
                    result: crate::ScriptType::Never,
                },
            )
            .expect("signature registers");
        let bytecode = compile("stop()", &registry.manifest());
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|op| matches!(op, Instruction::Udf(_)))
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Call(_)))));
        vm.resume(Value::Unit)
            .expect("simulate a faulty host returning from Never");
        assert!(matches!(vm.step(), Err(VmError::UndefinedInstruction(_))));
    }

    #[test]
    fn inherited_globals_skip_initializers_and_preserve_null() {
        let manifest = BuiltinManifest::new([("initialize", BuiltinId(0))]).with_type_metadata(
            SymbolManifest::default(),
            BTreeMap::from([(
                BuiltinId(0),
                crate::FunctionSignature {
                    receiver: None,
                    parameters: vec![],
                    variadic: None,
                    result: crate::ScriptType::String,
                },
            )]),
            vec![],
        );
        for source in [
            "global let name: String? = initialize()",
            "global var name: String? = initialize()",
        ] {
            let code = compile(source, &manifest);
            let mut inherited = Vm::new(code.clone()).expect("VM initializes");
            inherited
                .set_global_values(vec![Value::Optional(None)])
                .expect("inherit initialized null");
            assert!(matches!(
                inherited.step().expect("initializer is skipped"),
                Some(VmEvent::Completed(_))
            ));
            assert_eq!(inherited.global("name"), Some(&Value::Optional(None)));
            let mut fresh = Vm::new(code).expect("fresh session");
            assert!(matches!(
                fresh.step().expect("initializer executes in fresh session"),
                Some(VmEvent::Call(_))
            ));
        }
    }

    #[test]
    fn global_declarations_do_not_replace_inherited_state_but_assignments_do() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile("global var score = 1\nscore += 1", &manifest);
        let mut vm = Vm::new(code).expect("VM initializes");
        vm.set_global_values(vec![Value::Number(40.0)])
            .expect("inherit score");
        while !matches!(vm.step().expect("execute"), Some(VmEvent::Completed(_))) {}
        assert_eq!(vm.global("score"), Some(&Value::Number(41.0)));
    }

    #[test]
    fn an_uninitialized_non_optional_global_fails_when_read() {
        let bytecode = compile(
            "global var name: String\nname",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        assert!(matches!(vm.step(), Err(VmError::UninitializedGlobal(_))));
    }

    #[test]
    fn generic_functions_are_monomorphic_at_type_checking_and_erased_in_bytecode() {
        let bytecode = compile(
            "fn identity<T>(value: T) -> T { value }\nglobal var result: String = identity(\"alice\")",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("generic call executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::String("alice".into())));
    }

    #[test]
    fn explicit_generic_function_arguments_are_erased_before_bytecode() {
        let bytecode = compile(
            "fn identity<T>(value: T) -> T { value }\nglobal var result: String = identity<String>(\"alice\")",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("generic call executes"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("result"), Some(&Value::String("alice".into())));
    }

    #[test]
    fn text_template_values_are_distinct_from_plain_strings_at_native_boundaries() {
        let narrate = BuiltinId(30);
        let log = BuiltinId(31);
        let manifest = BuiltinManifest::new([("narrate", narrate), ("log", log)])
            .with_type_metadata(
                crate::SymbolManifest::default(),
                BTreeMap::from([
                    (
                        narrate,
                        crate::FunctionSignature {
                            receiver: None,
                            parameters: vec![crate::ScriptType::TextTemplate],
                            variadic: None,
                            result: crate::ScriptType::Unit,
                        },
                    ),
                    (
                        log,
                        crate::FunctionSignature {
                            receiver: None,
                            parameters: vec![crate::ScriptType::String],
                            variadic: None,
                            result: crate::ScriptType::Unit,
                        },
                    ),
                ]),
                Vec::new(),
            );
        let bytecode = compile("narrate(\"Hello, ${name}\")\nlog(\"${1 + 2}\")", &manifest);
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        let Some(VmEvent::Call(call)) = vm.step().expect("narrate yields") else {
            panic!("expected narrate call")
        };
        assert_eq!(
            call.arguments[0].value,
            Value::TextTemplate("Hello, ${name}".into())
        );
        vm.resume(Value::Unit).expect("narrate resumes");
        assert!(matches!(vm.step(), Ok(Some(VmEvent::Statement(_)))));
        let Some(VmEvent::Call(call)) = vm.step().expect("log yields") else {
            panic!("expected log call")
        };
        assert_eq!(call.arguments[0].value, Value::String("3".into()));
    }

    #[test]
    fn text_template_rewrite_happens_before_expression_evaluation() {
        let bytecode = compile(
            "global var translatedName: String = \"Alice\"",
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(bytecode).expect("VM initializes");
        while !matches!(
            vm.step().expect("globals initialize"),
            Some(VmEvent::Completed(_))
        ) {}
        let rendered = vm
            .eval_template_with("Hello, ${sourceName}", |_| {
                Ok("Bonjour, ${translatedName}".to_string())
            })
            .expect("rewritten template evaluates against runtime values");
        assert_eq!(rendered, "Bonjour, Alice");
    }

    #[test]
    fn returned_template_retains_lexical_values_after_serialization_and_rewrite() {
        let code = compile(
            r#"
            fn message() -> TextTemplate {
                let name = "alice"
                let translated = "bob"
                "Hello ${name}"
            }
            global let text = message()
        "#,
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(code).expect("VM");
        while !matches!(
            vm.step().expect("template function runs"),
            Some(VmEvent::Completed(_))
        ) {}
        let bytes = crate::hson::to_vec(vm.global("text").expect("returned template"))
            .expect("template serializes");
        let Value::TextTemplate(template) =
            crate::hson::from_slice::<Value>(&bytes).expect("template restores")
        else {
            panic!("expected template")
        };
        assert_eq!(
            vm.eval_template_value_with(&template, |source| Ok(source.to_owned()))
                .expect("original lexical value"),
            "Hello alice"
        );
        assert_eq!(
            vm.eval_template_value_with(&template, |_| Ok("Hello ${translated}".into()))
                .expect("rewritten template can reference a different lexical value"),
            "Hello bob"
        );
    }

    #[test]
    fn script_builder_statement_commit_is_once_per_chain_and_restores() {
        let manifest = BuiltinManifest::new([("submit", BuiltinId(1))]);
        let code = compile(
            r#"
            struct Actor { name: String, x: Int, emotion: String }
            extend Actor {
                fn at(self, x: Int) -> Actor { self.x = x; self }
                fn e(self, emotion: String) -> Actor { self.emotion = emotion; self }
            }
            global var commits = 0
            @statementCommit
            fn onActorCommit(actor: Actor) -> Unit {
                commits += 1
                submit(actor.name, actor.x, actor.emotion)
                actor // A handler's own statements must not recursively submit.
            }
            let alice = Actor.{ name: "alice", x: 0, emotion: "normal" }
            alice.at(4).e("happy")
            alice.e("sad")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM");
        let mut emotions = Vec::new();
        loop {
            match vm.step().expect("builder executes") {
                Some(VmEvent::Call(call)) => {
                    assert_eq!(call.arguments[0].value, Value::String("alice".into()));
                    assert_eq!(
                        call.arguments[1].value,
                        Value::Number(if emotions.is_empty() { 0.0 } else { 4.0 })
                    );
                    emotions.push(call.arguments[2].value.clone());
                    vm = Vm::restore(code.clone(), vm.snapshot())
                        .expect("commit resumes after restore");
                    vm.resume(Value::Unit).expect("submit completes");
                }
                Some(VmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(
            emotions,
            vec![
                Value::String("normal".into()),
                Value::String("happy".into()),
                Value::String("sad".into())
            ]
        );
        assert_eq!(vm.global("commits"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn script_optional_parameters_are_filled_without_skipping_required_arguments() {
        let manifest = BuiltinManifest::new(Vec::<(String, BuiltinId)>::new());
        let code = compile(
            r#"
            fn label(name: String, suffix: String?) -> String { name + (suffix ?: "!") }
            let callback: (String, String?) -> String = label
            global let first = label("alice")
            global let second = callback("bob")
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code).expect("VM");
        while !matches!(
            vm.step().expect("optional arguments"),
            Some(VmEvent::Completed(_))
        ) {}
        assert_eq!(vm.global("first"), Some(&Value::String("alice!".into())));
        assert_eq!(vm.global("second"), Some(&Value::String("bob!".into())));
        let program = crate::parse_program("fn label(name: String, suffix: String?) {}\nlabel()")
            .expect("parse");
        assert!(compile_with_manifest(&program, 0, &manifest).is_err());
    }

    #[test]
    fn restored_global_initialization_does_not_repeat_statement_handler() {
        let manifest = BuiltinManifest::new([("submit", BuiltinId(1))]);
        let code = compile(
            r#"
            struct Item { name: String }
            @statementCommit fn commit(item: Item) { submit(item.name) }
            global let item = Item.{ name: "alice" }
        "#,
            &manifest,
        );
        let mut vm = Vm::new(code.clone()).expect("VM");
        let mut calls = 0;
        while let Some(event) = vm.step().expect("initialize") {
            match event {
                VmEvent::Call(_) => {
                    calls += 1;
                    vm.resume(Value::Unit).expect("submit");
                }
                VmEvent::Completed(_) => break,
                _ => {}
            }
        }
        assert_eq!(calls, 1);
        let values = vm
            .globals()
            .iter()
            .map(|value| vm.export_value(value).expect("export global"))
            .collect();
        let mut restarted = Vm::new(code).expect("VM");
        restarted
            .set_global_values(values)
            .expect("restore initialized globals");
        while let Some(event) = restarted.step().expect("skip initialization") {
            assert!(
                !matches!(event, VmEvent::Call(_)),
                "restored declaration must not submit again"
            );
            if matches!(event, VmEvent::Completed(_)) {
                break;
            }
        }
    }

    #[test]
    fn template_object_captures_keep_identity_through_collection_and_restore() {
        let code = compile(
            r#"
            fn message() -> TextTemplate {
                let player = .{ name: "alice" }
                let text: TextTemplate = "Hello ${player.name}"
                player.name = "bob"
                text
            }
            global let text = message()
        "#,
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(code.clone()).expect("VM");
        while !matches!(
            vm.step().expect("template captures record"),
            Some(VmEvent::Completed(_))
        ) {}
        vm.collect_objects(&[]).expect("template is a heap root");
        let bytes = crate::hson::to_vec(&vm.snapshot()).expect("snapshot");
        let vm = Vm::restore(
            code,
            crate::hson::from_slice(&bytes).expect("decode snapshot"),
        )
        .expect("restore template heap");
        let Some(Value::TextTemplate(template)) = vm.global("text") else {
            panic!("template")
        };
        assert_eq!(
            vm.eval_template_value_with(template, |source| Ok(source.to_owned()))
                .expect("captured record remains alive"),
            "Hello bob"
        );
    }

    #[test]
    fn string_statement_handler_receives_evaluated_text() {
        let code = compile(
            r#"
            global var output = ""
            @statementCommit
            fn onText(text: String) { output = text }
            let name = "alice"
            "Hello ${name}"
        "#,
            &BuiltinManifest::new(Vec::<(String, BuiltinId)>::new()),
        );
        let mut vm = Vm::new(code).expect("VM");
        while !matches!(vm.step().expect("string hook"), Some(VmEvent::Completed(_))) {}
        assert_eq!(
            vm.global("output"),
            Some(&Value::String("Hello alice".into()))
        );
    }
}
