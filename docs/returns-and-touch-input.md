# Return statements and direct-manipulation pointers

## Script returns

`return expression` exits the innermost executing function or closure.
`return` without a value returns `()`. A newline immediately after
`return` ends the statement; put a multiline returned expression in parentheses.
Top-level returns are compilation errors.

```hks
fn label(name: String) -> String {
    if name == "alice" { return "" }
    return name
}

let compute: (Int) -> Int = { value: Int ->
    if value < 0 { return 0 }
    value
}
```

Tail-expression returns remain supported. Explicit return expressions are
checked against the callable's declared result, or participate in result
inference when no result annotation is supplied. A non-Unit function must
provide a compatible result on every reachable exit path. A return inside a
closure never returns from the function that created the closure.

AST and typed HIR retain the return statement; MIR lowers it to the existing
VM return terminator. It is not a native function or an engine statement hook.
Return frames continue to use the ordinary VM snapshot/restore mechanism.

## Embedded input

Hosts send canvas-relative UV coordinates in `HirakuPointerInput`.
`HirakuPointerId::Pointer(id)` and `HirakuPointerId::Touch(id)` are separate
namespaces, including at the full u64 range. Both become custom Bevy picking
pointers on the embedded canvas; neither is fed back into host input.

The presentation plugin forwards physical mouse and touch picking identities
without collapsing all fingers into one cursor. Touch cancellation is forwarded
as well. The game uses this same presentation plugin.

For touch, the engine finds the nearest scrollable ancestor of the picked UI.
After an eight-canvas-pixel movement in a scrollable axis it cancels the pending
press, captures that scroll target, and converts further finger displacement
to pixel-scroll events. Releasing a scrolling finger does not generate a click.
Other fingers retain their own interactions. Below the threshold, an ordinary
tap keeps the normal press/release behavior.

Mouse wheel and touch drag both use the existing bounded scroll handler.
Offsets stay clamped to current content bounds, including after content shrinks.
There is no inertia in this implementation.
