# Story state and UI state

`global` belongs to an execution environment, not to every script in the app.

- Story globals are shared across `story.call` and ordinary navigation. A global
  initializer runs only when the binding is uninitialized. Repeating a declaration
  does not reset an existing value, but declarations should still have one owner.
- A mounted UI owns its declared globals as transient, per-instance state. A
  callback may update that state; it cannot write back into story globals. A
  stacked UI keeps its state while covered; opening a new instance initializes
  fresh state. Story values supplied to the UI are read-only inputs.
- `ui.set(role, path)` selects an engine capability's UI implementation. It does
  not declare a variable, create a lexical scope, open a window or initialize
  story state. Use `ui.open`/`ui.mount` to display the configured UI.

Prefer a common story initialization script to own durable declarations and
initialization. Scene scripts call it at the relevant point and configure UI
roles separately. Do not repeat global declarations in every scene as a way to
import state. The current host still exposes story inputs by name; explicit
read-only namespace imports would make collisions clearer in a future API.

## UI recomposition

Each mounted script UI retains its compiled program and initial inputs. When
its local state changes, the engine re-evaluates the document/entry function
using the current local globals. Ordinary `if` branches and unbound text/image
arguments therefore reflect the new state; global initializers are not rerun.
Constructors should describe UI, with mutations and effects in callbacks.

This currently rebuilds the content subtree, not a keyed node-by-node diff. The
modal root, stack position and waiting request remain intact. Recomposition is
deferred during an active pointer press or text-edit session to avoid destroying
its target. Existing reactive properties continue updating during that session.
Subtree-local presentation such as scroll position can reset on recomposition;
retaining per-node presentation is a separate reconciliation improvement.

No per-frame script evaluation or recompilation is introduced. UI-local changes
do not become story/save-state mutations, and the compiled composition itself
is not serialized into scene data.
