# Model-owned input widgets

Input widgets use one-way values and explicit callbacks. They do not invoke a
binding setter. `onChange` proposes a value; the script can accept, transform,
or ignore it. Reapplying a model value does not emit an input event.

Declare transient input state in the UI document itself, not in the story:

```hks
global var playerName: String = "alice"
global var musicVolume: Float = 0.5
global var notifications: Bool = false
```

Continue the same UI document with:

```hks
import ui.widgets.*

screen {
    column {
        textInput(${playerName})
            .placeholder("Player name")
            .onChange { value: String -> playerName = value }

        slider(${musicVolume}, 0.0, 1.0)
            .onChange { value: Float -> musicVolume = value }

        checkbox(${notifications})
            .onChange { value: Bool -> notifications = value }
    }
}
```

`${expression}` is currently a **read-only** reactive input. Supplying a plain
value instead takes a snapshot. An ignored edit leaves the model unchanged.
Existing picture-based `toggle(value) { image(...) }.checked { image(...) }`
also supports `.onChange { value: Bool -> ... }`, and no longer flips its own
value. Pass a reactive value if the images should follow script changes.

`slider` and single-line `textInput` support `onCommit` with the same argument
type: slider pointer release and text submission, respectively. Script function
references can be passed in place of closures. Explicit callback parameter
annotations are currently required; callback signatures are validated during UI
materialization, before interaction. The required return type is `Unit` (or
`Never`). Receiver-driven inference for `{ value -> ... }` is not yet implemented.

New controls support the existing layout and text/background style modifiers.
Slider ranges must be finite and increasing. Checkbox activation uses Bevy's
press-then-release-on-the-same-target `Pointer<Click>` semantics.

## Host integration

Engine input is `HirakuTextInput`, not physical keyboard events. It supports
committed text, IME preedit, deletion, cursor movement, select-all, submission,
and cancellation. Preedit does not invoke `onChange`. `HirakuTextFocus` tells the
host when a text control owns keyboard input. App and editor use the optional
`hiraku_app::text_input::HirakuTextInputPlugin` to forward Bevy keyboard/IME events.
Other hosts can send text-edit messages themselves. Clipboard adapters and mobile
soft-keyboard presentation are not implemented yet.

## Current scope

These are basic Bevy UI controls, not a complete text editor or widget toolkit.
Multiline editing, partial selection, grapheme-aware cursor movement, dropdowns,
accessibility integration, and retaining interaction state across tree rebuilds
remain follow-up work. Text input currently places its cursor at the end on click.
Dependency-driven tree reconciliation and Clock subtrees are separate work; this
change retains the existing one-way Binding mechanism.

## State ownership

Story globals and engine models are read-only in UI execution. Rebinding or
mutating their object fields (including aliases) raises a VM error; callbacks do
not write a copy of all globals back to the story. Native UI effects remain
explicit capabilities, such as closing a screen or requesting a save.

Globals declared by a UI module belong to each mounted screen. They are initialized
before its `@ui` entrypoint, shared by that screen's callbacks and reactive controls,
and discarded on unmount. They are not persistent story state. Do not shadow a
story global with a UI declaration. Reopening or restoring a UI initializes its
drafts again. Cross-story callback transport and dependency-driven reconstruction
are not yet implemented; do not use a UI callback to mutate story globals directly.
