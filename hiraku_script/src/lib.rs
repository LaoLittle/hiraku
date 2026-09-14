//! HKS bindings distinguish rebinding from object updates:
//! `let` fixes a binding, while `var` allows reassignment. Both permit field updates.
//! Global variables use `global let` (initialized) or `global var` (optionally
//! initialized later). `global fn` exports a function.
//!
//! Unannotated numeric locals are constrained by their uses: `let a = 1; let b:
//! Float = a` infers both bindings as Float. Explicit annotations remain fixed.
//! An already typed Int requires `.toFloat()` before use as Float. Numeric
//! literals (including negative literals) can instead be inferred as Float.
//! `.toInt()` explicitly truncates a finite numeric value toward zero; implicit
//! Float-to-Int assignment is rejected. Values outside the exactly representable
//! integer range of the current numeric representation are rejected as well.
//!
//! `Never` is the bottom type. `todo()` and `unreachable()` trap; calls whose
//! result is Never are followed by a VM trap if a host incorrectly returns.
//! A value-returning named function's tail expression returns to its caller;
//! it does not emit a separate statement hook. Unit-returning procedures and
//! closure command lists continue to emit their statement hooks.
//! Any requires an explicit cast before assignment to a concrete script type;
//! native calls can accept dynamic values and validate them at the Rust boundary.
//!
//! `extend Player { fn score(self) -> Int { self.value } }` defines an instance
//! method using the same function/bytecode machinery as ordinary functions.
//! Methods without `self` are static: `extend Player { fn name() -> String {
//! "Player" } }` is called with `Player.name()`, without creating an instance.
//! Method lookup uses the canonical receiver type; primitive receivers are not
//! boxed. Records, including `self`, use execution-owned object references;
//! aliases and function arguments share field mutations. Primitive values are copied.
//! Generic extend blocks and bound-method values are not supported yet.
//!
//! Callable annotations use right-associative arrows: `(Int) -> (Int) -> ()`.
//! The unit type is spelled `()`; the standard prelude defines the transparent
//! alias `type Unit = ()`. Closure parameters can infer their types from an
//! expected callable signature. Native adapters may still use the erased
//! `Function` capability when a concrete signature is not available.
//!
//! `const` and `global const` require compile-time scalar initializers. Stored
//! objects and native calls are intentionally not constant expressions.
//! In an extend, `const A = 1` is accessed as `Player.A`. Computed properties use
//! `var score: Int { ... }` or `var score: Int { get { ... } set(value) { ... } }`.
//! Accessors lower to ordinary script functions; a setter parameter is mandatory.
//!
//! Object collection is non-moving mark-and-sweep. Shared-heap owners enumerate
//! all VM and host roots at safe points. IDs are never reused, and snapshots
//! retain the reachable graph's identity. Standalone VM users supply retained
//! host values to `Vm::collect_objects`; linked execution also collects at
//! allocation thresholds because its host boundary exports owned values.
//!
//! The bundled `std/core.hks` defines panic/todo/unreachable and numeric methods.
//! Only compiler operations in [`intrinsics`] receive dedicated lowering;
//! unused core definitions are omitted from the self-contained bytecode.

pub mod ast;
pub mod cst;
pub mod format;
pub mod source_text;
pub mod blocks;
pub mod hir;
pub mod hson;
pub mod intrinsics;
pub mod lex;
pub mod linked_vm;
pub mod linker;
pub mod mir;
pub mod native;
pub mod objects;
pub use objects::{ObjectHeap, ObjectId};
mod fingerprint;
pub use fingerprint::ProgramFingerprint;
pub mod debug;
pub mod project;
pub use project::{
    CompiledProject, ProjectError, ProjectLinkPolicy, ScriptSource, compile_project,
    compile_project_with_policy,
};
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
    HirWhenArm, LoweringError, ResolvedFunction, ScriptType, StatementValue, TypeId, TypeTable,
    lower_to_hir, normalize_program_symbols,
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
    LinkError, LinkPolicy, LinkedBytecode, LinkedFunction, LinkedModule, LinkedProgram, ModuleId,
    link_bytecode, link_named_modules, link_named_modules_with_policy, link_register_modules,
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
