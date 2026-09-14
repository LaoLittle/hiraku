# Script-owned standard library migration

## Implemented foundation

- `struct Player<T> { name: T, score: Int }` is a nominal record declaration.
  It uses existing `ScriptType::Struct`, reference identity, generic checking,
  method lowering, and snapshot support. A different declaration is not
  assignable solely because its fields have the same shape.
- `Player.{ ... }` checks its fields against the declared schema before assigning
  a nominal type. Contextual construction remains `let p: Player<String> = .{ ... }`.
- Native registrations may require an opaque capability using
  `NativeRegistry::require_capability`. Requirements are part of the ABI hash.
- `link_named_modules_with_policy` accepts host-owned, module-local `LinkPolicy`
  grants. Existing linking entry points grant nothing. Grants are deliberately
  not serialized and never inherited from the caller.
- Taking a protected intrinsic as a function value is rejected, even in an
  authorized module. Export a script wrapper instead. Script functions may not
  occupy the reserved `intrinsics` namespace.

This does **not** yet make every engine native API private. Its registrations
are moving behind the ABI incrementally. Actor and dialogue now use a separate
privileged link unit. Snapshot restoration receives freshly authorized link units
rather than persisting grants. Remaining prelude families still need migration.

## Enum and when support

Implemented:

```hks
enum Packet<T> {
    data(T),
    empty,
}

let packet: Packet<Int> = .data(42)
let result = when packet {
    .data(value) -> {
        value + 1
    }
    .empty -> 0
}
let empty = Packet.empty<Int>()
```

Variants have nominal enum identity and typed positional payloads. Contextual
`.data(...)` / `.empty` constructors use the expected enum; qualified constructors
use `Packet.data<Int>(42)` / `Packet.empty<Int>()`. Raw generic types are rejected.
Payload bindings are immutable, scoped to their arm, and may be captured by closures.
Use `_` to discard an individual payload.

Matching evaluates the subject once and executes only the chosen arm. It checks
coverage, duplicate/unknown variants, payload arity and branch result types.
Arm bodies execute inline, not as hidden closure invocations. Their final expression
is the match result, not an extra statement commit. Ordinary calls and checkpoints
inside an arm retain the existing VM wait/snapshot behavior.

Enum construction uses a register slice and the existing typed/tagged value
representation. Matching lowers to basic blocks with discriminant tests and payload
access. Bytecode version is 21. UI compilation and asset/voice analysis traverse
match arms as conditional regions.

Current limits: recursive enum schemas, nested/destructuring patterns, guards and
wildcard whole-variant arms are not implemented. List every variant explicitly.
The schema currently expands concrete generic arguments; recursive types require
declaration references rather than infinite expansion.

## Remaining sequence

1. Extend nominal enum schemas to declaration references for recursive types.
2. Extend patterns as required without weakening static exhaustiveness.
3. Finish migrating optional operations through general enum construction/matching.
   The declarations now live in std; Optional still has a specialized native ABI.
   Remove special Optional type/value/VM paths only after
   generic semantics cover nested options, casts, safe access and narrowing.
4. Script protocols now lower `self` to a typed first parameter and
   dispatch operators by static conformance signatures. Extend this to imported extension interfaces
   before moving engine-owned operators into separate library modules.
5. Script-owned statement handlers with explicit transaction boundaries and
   resumable frames. Do not run handlers on arguments,
   intermediate fluent values, or returned tail values. Suppress recursive
   handler invocation while retaining normal host waits.
6. Separate privileged engine prelude modules; migrate dialogue, then Actor
   state and incremental patches, then UI wrappers. Delete redundant public
   native registrations only after their callers have migrated.

## Optional semantics

The bundled `std/core.hks` definitions are:

```hks
enum Optional<T> {
    some(T),
    none,
}

enum Result<T, E> {
    success(T),
    error(E),
}
```

Variant declarations require commas, including across newlines; a trailing comma
is allowed. These types are provided by std rather than declared by applications.
Result uses the generic enum representation. Optional retains the specialized
Rust Option/native ABI and ScriptType::Optional/Value::Optional internally during
migration. Its constructor and pattern schemas are read from the std declaration.
Both explicit Optional<T> and T? use that same representation, including nested
payloads, native calls, snapshot restoration and exhaustive when matching.
This is not yet removal of the specialized Optional runtime.

For an optional subject, `a ?: fallback()` now lowers to the same HIR control flow as:

```hks
when a {
    .some(value) -> value
    .none -> fallback()
}
```

Evaluate `a` once. Evaluate `fallback()` only in the `none` arm. `null` constructs
`none` contextually; `.some(null)` remains distinct from outer `none` for nested
optionals. Repeated `?` retains the existing normalization warning, while explicit
`Optional<Optional<T>>` retains nesting.

Elvis chains associate to the right: `a ?: b ?: c` means `a ?: (b ?: c)`.
A literal `null ?: expression` has the type of that expression, including Never.
A non-nullable left operand is returned without executing its fallback (the
fallback is still type checked). A nullable fallback preserves optionality:
`String? ?: null` is String?, while `String? ?: Never` is String.
The old eager SelectNonNull instruction and HIR Elvis node have been removed;
optional branches use the normal when/control-flow representation.

Existing record-shaped `type` declarations retain their current semantics during
this step; the new `struct` spelling does not silently reinterpret existing assets.

## Type extensions

`extend Type { ... }` adds instance methods, static functions, constants and
computed properties to a named type. The former script `impl` keyword is rejected
with a migration diagnostic. Rust implementation blocks are unaffected.
Extensions lower to ordinary typed functions; they introduce neither a wrapper
object nor an additional runtime dispatch layer.

```hks
protocol Colon<Rhs> {
    type Output
    fn colon(self, rhs: Rhs) -> Self.Output
}

// Colon is already supplied by std; applications only write this extension.
extend String: Colon<TextTemplate> {
    type Output = Unit
    fn colon(self, text: TextTemplate) {
        say(self, text)
    }
}

"alice": "Hello ${player.name}"
```

`operator fn` has been removed. Colon resolution first checks the statically
known receiver's Colon conformance.
Its right operand receives the declared parameter context, so TextTemplate
literals remain unevaluated. Other expressions are checked against that type.
The operator executes in an ordinary resumable function frame, including host
waits and snapshot restoration. Protocol arguments, required methods, associated
type definitions and implementation signatures are checked before method lowering.
`Output` and `Self.Output` resolve in the implementation signature's scope.
Missing/duplicate requirements and incompatible associated return types are errors.

Protocol methods also participate in ordinary receiver lookup: `value.test()`
and calls such as `self.test()` inside another protocol implementation resolve
statically. Inherent extension methods take precedence; otherwise exactly one
protocol implementation must provide the member. Multiple protocol candidates
produce an ambiguity diagnostic rather than selecting by declaration/import order.
Taking a bound protocol method as a value requires a closure for now. The core
library's demand-driven loader recognizes both operator use and explicit method
names such as `number.add(other)`.

Std defines Add, Subtract, Multiply, Divide, Equal, NotEqual, Less, LessEqual,
Greater, GreaterEqual, Negate and Not alongside Colon. Int/Float implement
arithmetic, comparison and negation; String implements concatenation and equality;
Bool implements equality and negation. Their bodies call typed compiler
intrinsics, which lower directly to existing arithmetic/comparison instructions.
Primitive operator expressions resolve through the standard library's protocol
functions. Those functions carry `@inline`; the MIR optimizer can remove their
call frames without boxed values. Custom record implementations use the same
ordinary resumable call machinery.
Logical `&&`/`||` retain control-flow lowering and short-circuit evaluation.

Remaining limitations: imported protocol interfaces, multiple RHS implementations of one protocol for one receiver,
associated types in method-local declarations, recursive associated type
projections, default methods and protocol objects. These are not yet a complete
protocol type system. Nullable/structural equality retains its existing lowering.
The engine's native colon registration remains a fallback during migration.

`compile_project_with_policy` accepts a host-owned `ProjectLinkPolicy` with grants
keyed by exact source path. Grants resolve after deterministic module sorting;
unknown paths are errors. Ordinary project compilation grants nothing. The host
must supply authenticated library contents; a privileged-looking filename alone
does not authorize code. Policies are not serialized in snapshots. This adds the
project compiler entry point for the existing capability-aware linker, not an
automatic privilege grant to the engine's current embedded prelude.

## Generic protocol constraints

```hks
protocol Test {
    fn test(self) -> String
}

extend String: Test {
    fn test(self) -> String { self }
}

fn what<T: Test>(value: T) { value.test() }
fn forward<T: Test>(value: T) -> String { what(value) }

let result = forward("alice")
```

Function bounds support multiple protocols (`T: Label + Append<Int>`) and
generic protocol arguments. They also apply to generic methods in concrete
extensions. Unconstrained method access, missing conformances and ambiguous
methods are compile errors. The return type can be inferred from the bound's
method signature. Generic forwarding must carry the required bounds itself.

The compiler adds typed, implicit implementation-function parameters. Call sites
prove conformance and provide these parameters; forwarding reuses the caller's
evidence. Bodies remain type-erased: no monomorphization, string lookup, native
function pointer serialization or protocol-specific VM opcode is introduced.
Closures capture the implicit function parameters using ordinary captures, and
host waits/snapshots preserve them using existing script function values.

Current explicit restrictions:

- Constrained function exports require a cross-module witness interface and are
  rejected until that interface is implemented, rather than losing constraints.
- Taking a constrained generic function/method as a value is rejected. A typed
  closure can call it with concrete arguments and capture the required evidence.
- Bounds on type aliases, structs, enums and generic extension targets remain
  unsupported; protocols with associated types or no methods cannot yet be used
  as generic bounds. Associated projections require an additional constraint
  representation, not an Any fallback.
- Protocol requirements cannot themselves have generic parameters yet.

Next priorities are cross-module protocol/conformance identity and witness
signatures, associated-type projections/equality constraints, then constrained
generic types and generic extensions. These must share the same conformance
checker rather than introducing separate runtime checks per syntax form.

## Intrinsic namespace and authorization

Compiler operations now use `intrinsics.panic`, `intrinsics.floatToInt`,
`intrinsics.intToFloat`, `intrinsics.toString`, and typed operation namespaces
such as `intrinsics.int.add`. Legacy `__builtin_*` calls are rejected.
Only injected core functions receive compiler-intrinsic authorization during
normalization. Parsed functions always start unprivileged; source attributes,
shadowing a std function, and incoming AST privilege flags cannot grant access.
Authorization is checked before lowering an intrinsic to inline bytecode, where
there would no longer be a native relocation for the linker to inspect.

Rust-provided module members use native capability requirements and the defining
module's link policy. `engine` has no compiler-defined meaning: registration of
`intrinsics.audio.invoke`, `intrinsics.graphics.invoke`, or `host.services.invoke`
uses exactly the same member resolution and authorization. Qualified native
function lookup respects local/global value bindings shadowing a module root.
Project tests exercise an
independent authorized HKS wrapper calling `intrinsics.engine.say` and reject
the same call from an untrusted entry module. Compiler permission does not grant
host permission, and permission is not inherited by a wrapper's callers.

Engine API registrations have **not** all been renamed or moved into script
wrappers yet. The embedded `script/std/dialogue.hks` now exports `say` and
`Colon<TextTemplate>` implementations for Actor, String and Ellipsis. The Rust
operator dispatch branch has been removed. Ellipsis has a concrete language type;
it no longer masquerades as Any. The three dialogue primitives are ordinary native
functions with a link-time `dialogue.write` requirement. Only the authenticated
embedded module receives this grant, in project, standalone and restore paths.

Actor itself is still a native handle with Rust-owned pending builder state.
Moving those fields and fluent methods into a script-defined struct is the next
migration, not completed by moving Colon. It requires sharing nominal type
declarations across library interfaces and preserving alias/clone, animation,
await and snapshot behavior. Do not replace it with a script struct that merely
wraps the same native builder and claim the ownership migration is complete.

The cross-module prerequisite is now implemented: `global struct Actor { ... }`
exports a nominal type, and `global fn` inside an extension exports its instance
method. Public structs are collected before any function interfaces, independently
of file ordering. Their identities are explicitly recorded in bytecode metadata;
the linker shares only declared public identities, rather than treating every
same-named private struct as the same type. Duplicate public types and a private
declaration shadowing a public type are compile errors. This first export form
covers structs, not exported enums or aliases.

An end-to-end project test uses a script-only Actor, script-owned fields and dirty
flag, exported `.at()` / `.e()` methods and an exported statement handler. The
only native operation receives final primitive arguments. It verifies shared
object identity, chain-level commits, field retention, numeric inference and
snapshot restore while the host submission is suspended. Numeric re-lowering
retains all imported types, receiver metadata and statement handlers.

The embedded library now defines Actor and its fluent mutations in HKS. An Actor
contains a native identity and a script-owned incremental patch. Native state is
the authoritative committed presentation (including alias display identity and
motion revision), not a second fluent builder. Empty fields retain prior state;
clip uses nested Optional to distinguish unchanged from explicitly cleared.
Only submitActor applies the patch and flushes that identity. The actor-wide Rust
statement flush has been removed. Animation handles and hide cancellation remain
owned by the existing effect lifecycle, not by a second script scheduler.

Actor/String/Ellipsis Colon and Stage.place use ordinary script methods. Actor
intrinsics require scene.actor; dialogue intrinsics require dialogue.write. The
authenticated embedded module receives these grants at link time. Standalone
compilation seeds the library's symbol table from its caller, preserving nominal
type identity without exporting capabilities or user source into the library.

Patch fields are detached and cleared before submission. Recorded animation
steps own these values independently of subsequent builder mutations. Public
actor names, optional arguments and time units are preserved by this migration.
Other fluent families and the Rust bare-string statement consumer remain to be
migrated; this is not yet a complete engine prelude conversion.

### Recorded sequence and parallel plans

`seq` and `par` now evaluate their closure synchronously into an engine-owned
`AnimationPlan`. Variables, conditions and templates are evaluated while building
the plan. The temporary execution is removed before the first effect is returned
to ECS; playback does not retain or resume that closure's stack.

Sequence playback submits one statement batch at a time and advances on effect
completion (including automatic dialogue completion). Parallel playback submits
all batches up to an explicit animation `.await()` barrier. Voice effects use
concurrent playback; dialogue in `par` is warned about and skipped. The returned
handle joins playback, not closure evaluation. Snapshots store remaining batches
and active effects, and restoration never repeats build-time variable mutations.

Each recorded actor offset retains its own revision for ECS reattachment. The
plan separately remembers the final authored revision per actor for cancellation
checks; sharing one revision between all steps would incorrectly treat later
steps as restored instances of the first tween.

Construction has a finite instruction budget. Interactive host requests and
nested plan construction are currently rejected explicitly, not silently queued;
hierarchical plan composition remains a follow-up. Choice callbacks still use
ordinary resumable script executions because they can ask the player for input.

## Script statement handlers

`@statementCommit fn onActorCommit(actor: Actor) -> Unit` is now compiled into an
ordinary function call for a matching expression statement. An exported handler
also works through project interfaces. It takes one concrete typed argument and
returns Unit; duplicate local handlers for the same type are rejected. Fluent
chains commit once, not once per member call. Initialized let/global bindings
also dispatch their value's typed handler after storing the original value, so
`let actor = char("alice").show()` still shows the actor. A restored global skips
both initialization and its handler. Declarations do not emit bare-string dialogue
events. Value-returning function tails still
return values, rather than committing builders. A handler's own expression
statements do not implicitly invoke statement handlers again. Explicit function
calls retain ordinary call semantics.

String handlers receive eagerly evaluated strings. A TextTemplate-only handler
receives a lazy template; when both types are provided, String is preferred.
Tests use a fully script-defined Actor builder and verify shared field updates,
one commit per chain, and suspension/restoration inside the handler. The engine's
existing Rust statement consumer has not yet been deleted.

Lazy templates retain their lexical environment when passed through wrappers.
Primitive bindings are captured by value, records retain shared heap identity;
template captures are GC roots and participate in heap relocation and snapshots.
The environment is shared when cloning a template, not recursively copied for
each wrapper. Localization runs before evaluating the template against its
captured scope, and may use a different captured variable. Computed String
arguments remain strings and are not interpreted a second time.

## Inline hints and runtime calls

`@inline` is a compile-time optimization hint on functions and extension methods.
An initial bounded MIR pass handles small straight-line value functions containing
parameter reads, constants, moves, arithmetic, negation and string conversion.
Argument evaluation remains in the caller, once each in source order, including
unused parameters. Local registers are remapped rather than sharing callee locals.
Four bounded bottom-up rounds allow small annotated wrappers around inline helpers.

Host calls, recursive calls, statement commits, writes, closures, control flow,
generic substitutions, Unit/Never-returning functions and trackCaller functions
are not currently inline candidates. The hint falls back to an ordinary call;
it does not grant intrinsic access or change waiting semantics. Host native
capabilities are checked at link time, not every time the VM executes a Call.
Compiler-only operations are authorized before lowering to machine-like bytecode;
this is separate from runtime native dispatch. No runtime capability opcode was
introduced.

Cross-module inlining and inline-frame debug metadata remain future work. Exported
or address-taken function bodies remain available even when direct calls inline.
