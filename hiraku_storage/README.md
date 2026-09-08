# Storage backends

`AsyncByteStorage` and `AsyncPlatformStorage` provide awaitable binary storage.
On native targets file operations execute on a blocking pool. On browser targets
the implementation uses IndexedDB via Rust/web-sys, with no JavaScript source
files. All target-specific code and dependencies remain inside platform modules.

```rust,ignore
use hiraku_storage::{AsyncByteStorage, AsyncPlatformStorage};
let storage = AsyncPlatformStorage::new("saves", "my-game.saves", "sav");
storage.write("alice", &payload).await?;
let restored = storage.read("alice").await?;
```

Browser data uses one database per namespace (`hiraku.idb.<namespace>`), a
versioned `bytes` object store, and Uint8Array values. Include a stable project
identifier in the namespace. A successful write/remove awaits transaction
completion, not request success. Errors are propagated. Dropping an unfinished
transaction future attempts to abort it; already completed commits cannot be
undone. An open blocked by another tab stays pending; abandoned open requests
retain cleanup handlers and close any resulting unclaimed connection.

## Engine storage

Engine callers use `BufferedStorage`. Native writes complete synchronously;
browser writes enter a serialized IndexedDB queue. `WriteQueued` is not durable
completion: the engine waits for `RuntimeStorageStatus::Ready` before running
dependent story/UI effects. Errors enter `Failed` and stop those effects.
Browser startup initializes the namespace caches before scripts run. Currently
all records are cached, including snapshots and PNGs; on-demand loading is not
implemented yet. Failed writes leave speculative cache values, but the engine
does not resume or report success. Reloading reconstructs the durable state.

The browser imports old localStorage domain records once into missing IndexedDB
keys, then records a migration marker. Source data is retained. Historical
localStorage domains did not identify a project, so imported records cannot be
reliably attributed to a project. The engine no longer recognizes old single-file
save slots without independent metadata; importing bytes does not make them
compatible saves.

`enqueue_generation` publishes immutable data records plus a final mutable index.
Native writes use fresh files and replace the index by same-directory rename;
IndexedDB uses one transaction. Engine slots contain `slot-<hash>.hson`,
`snapshot-<generation>.sav`, and optionally `thumbnail-<generation>.png`.
Metadata and PNG parsing do not depend on the snapshot schema. Missing metadata
hides a save. Unreadable snapshots do not prevent previews, and damaged PNGs
are displayed as transparent placeholders. Old generations are retained for now;
garbage collection and full power-loss durability guarantees are not provided.

Browser runtime behavior still needs integration tests; a wasm compilation check
does not validate browser transactions, quota failures, or multi-tab behavior.
