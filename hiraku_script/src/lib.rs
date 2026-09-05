//! HKS bindings distinguish rebinding from object updates:
//! `let` fixes a binding, while `var` allows reassignment. Both permit field updates.
//! Global variables use `global let` (initialized) or `global var` (optionally
//! initialized later). `global fn` exports a function.
//!
//! Unannotated numeric locals are constrained by their uses: `let a = 1; let b:
//! Float = a` infers both bindings as Float. Explicit annotations remain fixed.
//! `.toInt()` explicitly truncates a finite numeric value toward zero; implicit
//! Float-to-Int assignment is rejected. Values outside the exactly representable
//! integer range of the current numeric representation are rejected as well.
//!
//! `Never` is the bottom type. `todo()` and `unreachable()` trap; calls whose
//! result is Never are followed by a VM trap if a host incorrectly returns.
//! Any requires an explicit cast before assignment to a concrete script type;
//! native calls can accept dynamic values and validate them at the Rust boundary.

pub mod ast;
pub mod blocks;
pub mod hir;
pub mod hson;
pub mod lex;
pub mod linked_vm;
pub mod linker;
pub mod mir;
pub mod native;
pub mod parse;
pub mod register;
pub mod runtime;
pub mod span;
pub mod string_pool;
pub mod symbol;
pub mod template;
pub mod vm;

pub use ast::{
    Argument, BinaryOp, Block, CastMode, Expr, ExprKind, FunctionParameter, MapField, NumberUnit,
    Program, Stmt, SyntaxWarning, TypeExpr, TypeExprKind, TypeField,
};
pub use blocks::{BlockDocument, BlockDocumentError, BlockId, SourceBlock, parse_block_document};
pub use hir::{
    HirArena, HirArgument, HirBlock, HirExpr, HirExprKind, HirFunction, HirFunctionId, HirGlobal,
    HirGlobalId, HirLiteral, HirLocal, HirLocalId, HirPlace, HirProgram, HirStmt, HirStmtKind,
    LoweringError, ResolvedFunction, ScriptType, StatementValue, TypeId, TypeTable, lower_to_hir,
    normalize_program_symbols,
};
pub use hiraku_errors::{
    Diagnostic, DiagnosticLabel, RenderOptions, Severity, SourceId, SourceMap,
    emit_rendered_diagnostic, render_diagnostics, write_diagnostics, write_rendered_diagnostic,
};
pub use hiraku_script_derive::{HksHandle, hks_define, hks_module};
pub use linked_vm::{
    LinkedVm, LinkedVmError, LinkedVmEvent, LinkedVmFrameSnapshot, LinkedVmSnapshot,
};
pub use linker::{
    LinkError, LinkedBytecode, LinkedFunction, LinkedModule, LinkedProgram, ModuleId,
    link_bytecode, link_named_modules, link_register_modules,
};
pub use mir::{
    MirBasicBlock, MirBlockId, MirConstant, MirFunction, MirInstruction, MirLoweringError,
    MirProgram, MirTerminator, VirtualRegister, lower_hir_to_mir,
};
pub use native::{Never, TextTemplate};
pub use parse::{ParseError, parse_program};
pub use register::{
    InvalidRegister, Register, RegisterAllocation, RegisterAllocationError, RegisterFrame,
    allocate_registers,
};
pub use runtime::{
    BuiltinCall, BuiltinId, BuiltinManifest, CallArgument, FunctionSignature, StaticMember,
    StaticMemberKind, Value,
};
pub use span::Span;
pub use string_pool::{StringId, StringPool};
pub use symbol::{SymbolId, SymbolInterner, SymbolManifest};
pub use template::{TemplateCallArgument, TemplateContext, TemplateError, eval_template};
pub use vm::{
    BYTECODE_VERSION, Bytecode, CompileError, Constant, Instruction, SymbolCall, Vm, VmError,
    VmEvent, VmSnapshot, VmStatus, compile_with_manifest,
};
extern crate self as hiraku_script;
