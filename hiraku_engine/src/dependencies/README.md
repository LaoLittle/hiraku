# Execution-based texture windows

`hiraku-tools` generates source-indexed resource control flow in the package's
root dependency manifest. The graph is metadata, not a second executable IR.
`hiraku-script` exposes borrowed source positions for active VMs and suspended
call frames; it knows nothing about images, stories, or Bevy assets.

At runtime the engine follows graph edges with breadth-first lookahead:

- Both `if` branches, loop body/exit, closures, and statically resolved script
  function calls are possible paths. No condition or native function is run.
- Local functions take precedence over exported functions. Current story call
  frames also seed the window, retaining their return continuations.
- Defaults: 32 forward statement edges, at most 256 visited nodes per package,
  four previous window revisions, and 128 MiB of speculative image uploads.
- Near-future images get budget priority over older windows. Unknown sizes and
  computed resource names remain demand-loaded. The window currently covers
  texture assets, not audio/video streaming.
- Unchanged source positions do not rebuild the window. Waiting on a dialogue
  or UI for a long time never expires cached artwork.

Live render entities and mounted UI retain their own asset handles. Leaving the
window drops only speculative handles. Fully hidden character hierarchies are
retired once their images leave the window and their execution-based grace
window ends; visible actors, pending shows/restores, and fading actors remain
owned by the scene. There are no wall-clock eviction timers.

Navigation waits only for the target entry window. Normal forward prefetch is
asynchronous and does not block story progress. Loose projects without a
generated manifest continue to load on demand. Rebuild HDP packages after a
dependency schema change. UASTC packaging rewrites graph paths together with
texture descriptors and the aggregate dependency sets.

This is deliberately conservative: a closure may be prefetched before it is
invoked, and arbitrary dynamic function/resource expressions are not predicted.
The visit and byte budgets prevent such speculation from scanning or loading
an entire chapter. This is not speculative execution and cannot consume RNG,
advance timers, mutate globals, or write storage.
