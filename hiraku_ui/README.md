# hiraku-ui

Compiler-assisted HKS UI support. Depends on `hiraku-script`, not on
`hiraku-engine`, story effects, asset catalogs, or VN product features.

## Implemented first step

- The script compiler exposes an optional arena-backed typed-HIR pass, after
  ordinary type checking and before the existing MIR/bytecode pipeline.
- `UiCompiler` extracts dynamic arguments of host-declared property parameters
  into ordinary zero-argument closures. No second VM is introduced.
- `PropertyComputation` owns a compiled callable and its captured environment.
  It is one-way; changes are accepted through widget callbacks, never a setter.
- `UiDocument` owns project compilation, `@ui` entry validation, entry invocation,
  property plans, and mount-owned global discovery. Engine loading and offline
  validation both use this API.
- `compose` executes the shared script VM with a bounded instruction budget,
  read-only external state, and a host draft-commit callback. Engine code supplies
  widget primitives and materializes their drafts into Bevy UI; it does not run
  a second composition execution loop.
- Compilation records property/control-flow sites independently of debug info
  and conservatively identifies global reads outside property computations.
- Engine composition uses that information to avoid rebuilding children when
  only property inputs changed. Text updates avoid writing unchanged strings.

```hks
global var count = 1
canvas {
    text(count.toString())
    button { text("Next") }.onClick { count += 1 }
}
```

The legacy `binding(getter, setter)` engine API has been removed. The widget
showcase and the custom dialogue example use ordinary expressions.

## Migration boundaries

This is not yet a complete composition runtime:

- Host property signatures still use the existing `Union<T, Binding<T>>`
  metadata. The old explicit property syntax/type and Rust adapters remain
  until all consumers migrate. Generated properties use callable types.
- Engine property payloads use `PropertyComputation` directly. Some ECS system
  and component names still retain the old binding terminology during migration.
- Property evaluation still uses the existing model revision scheduler.
  Per-dependency read tracking through arbitrary calls is not implemented.
- Control-flow sites are candidates for region lowering, not Bevy entities.
  Structural changes still rebuild screen content. Per-region reconciliation
  and stable identities for repeated rows remain to be implemented.
- Native getters need dependency/effect metadata before arbitrary external
  state can safely drive composition.
- Captured locals are values, not automatically inferred derived state.
  Upstream structural reads conservatively retain the rebuild path.
- String interpolation is unchanged. Use an ordinary expression such as
  `text(time.elapsedSeconds.toString())`; compiled interpolation and the
  proposed `time()` API remain separate work.

No Gallery/achievement/unlock behavior belongs in this crate.
