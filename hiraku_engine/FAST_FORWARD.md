# Fast-forward

Fast-forward is transient playback control, not saved story state. The initial
implementation skips all dialogue; it does not implement a read-text database.

Hosts send platform-independent `HirakuActionInput` messages:

- `FastForwardHeld(true/false)` for press/release. Always forward release on
  focus loss. The standard `hiraku-app` presenter maps this to either Ctrl key.
- `ToggleFastForward` for a latched control.
- `NextDialogue` or `Back` cancels fast-forward. Clicking the dialogue surface
  also cancels it and follows the ordinary reveal/advance behavior.

Script-authored UI reads `dialogue.fastForwardEnabled` and calls
`preferences.setFastForward(enabled)` in its `onClick` callback. This changes
runtime pacing, not persisted user preferences. Auto and fast-forward are
mutually exclusive. Buttons continue using ordinary Bevy UI hit testing.

Fast-forward uses 32x story delta for camera, actor, picture, transition, fade
and sleep systems. It does not modify Bevy's application clock, UI animation
time, clock/time values exposed to scripts, BGM pitch, or movie playback.
Dialogue progress is bounded to one wait per update, with an 80 ms interval;
normal response processing still records progression for replay and history.
Voices and awaited SFX finish through their normal completion tokens, including
parallel voices, so `.await()` and sequence joins do not hang.

Choices, modal UI, movies and text editing stop fast-forward. Holding Ctrl
through such a boundary does not restart it: release and press again. Asset
loading temporarily suspends pacing while still consuming release events.
Restoring a scene or resetting presentation clears fast-forward; ordinary
story navigation without reset can retain it across asset loading.

No unbounded VM loop, OS-specific input listener or blocking wait is added to
the engine. Reactive desktop runners receive redraw requests while fast-forward
is active. Runtime visuals should be tested by the host application.
