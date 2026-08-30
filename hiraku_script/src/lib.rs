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
pub mod symbol;
pub mod template;
pub mod vm;

pub use ast::{
    Argument, BinaryOp, Block, Expr, ExprKind, FunctionParameter, MapField, NumberUnit, Program,
    Stmt, TypeExpr, TypeExprKind, TypeField,
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
pub use symbol::{SymbolId, SymbolInterner, SymbolManifest};
pub use template::{TemplateCallArgument, TemplateContext, TemplateError, eval_template};
pub use vm::{
    BYTECODE_VERSION, Bytecode, CompileError, Constant, Instruction, SymbolCall, Vm, VmError,
    VmEvent, VmSnapshot, VmStatus, compile_with_manifest,
};
extern crate self as hiraku_script;
