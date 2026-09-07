# Story animation completion

Fluent animation builders commit at the end of their statement. Append `.await()`
to wait for the committed effects, using the same completion protocol as `seq`:

```hks
scene.curtain(0).fade(300).await()
camera().zoom(1.2).animation(.easeOut(0.5)).await()
alice.offset(.pos(0, 20)).animation(.easeOut(0.2)).await()
alice.hide(300).await()
scene.hideCharacters(300).await()
bg("background/room").fade(300).await()
voice("voice/alice").await()
sfx("sound/bell").await()
bgm("music/theme").fadeIn(500).await()
```

`fade`/`fadeIn` and character hide durations are milliseconds; `AnimationSpec`
durations are seconds. BGM completion means its fade-in has completed, not that
looping music has stopped. Voice and SFX completion means playback has ended.
Missing catalog entries complete their pending request with a diagnostic.

- Ordinary animation statements launch without blocking.
- `seq {}` launches a non-blocking sequence and waits for each statement's effects
  before executing the next. Dialogue reveals and advances automatically.
- `par {}` launches effects without per-statement waits. Its handle joins every
  outstanding effect, regardless of completion order. Dialogue is not allowed.
- An explicit `.await()` within `par` adds a barrier to that execution.
- Normal voice calls are exclusive; voices in seq/par use independent playback.
- Loading a save completes transient voice/SFX waits without replaying the audio.

Builders are statement-scoped: call `.await()` in the same fluent statement.
For a reusable completion handle, retain a sequence/parallel handle:

```hks
let playback = par {
    voice("voice/alice")
    voice("voice/bob")
}
// Other story work may run here.
playback.await()
```

There is no separate `awaitCompletion` API or keyframe-list scheduler.
Completion is driven by ECS playback/animation state, never by an estimated sleep.
