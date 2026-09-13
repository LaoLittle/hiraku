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

This does **not** yet make the existing engine native API private. Its registrations
must be moved behind the ABI incrementally. Engine compilation currently embeds
some library code into user modules; privileged libraries must become separate
link units before grants can safely be applied to them. Snapshot restoration must
also receive freshly authorized link units rather than persisting grants.

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
access. Bytecode version is 20. UI compilation and asset/voice analysis traverse
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
4. Script operator declarations: lower `self` to a typed first parameter and
   dispatch by static signatures. Operators generate ordinary calls.
5. Script-owned statement handlers with explicit transaction boundaries and
   resumable frames. Do not run handlers on arguments, declaration initializers,
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
