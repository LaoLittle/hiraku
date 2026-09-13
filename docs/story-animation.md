# Story animation completion

Fluent animation builders commit at the end of their statement. Append `.await()`
to wait for the committed effects, using the same completion protocol as `seq`:

```hks
scene.curtain(0).fade(300).await()
camera().zoom(1.2).time(0.5).easing(.easeOut).await()
alice.offset(.pos(0, 20)).time(0.2).easing(.easeOut).await()
alice.hide(300).await()
scene.hideCharacters(300).await()
bg("background/room").fade(300).await()
voice("voice/alice").await()
sfx("sound/bell").await()
bgm("music/theme").fadeIn(500).await()
```

`fade`/`fadeIn` and character hide durations are milliseconds; `.time(seconds)`
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

## Independent duration and easing

`.time(seconds)` changes only duration; `.easing(curve)` changes only the curve.
Both orders are equivalent, and an omitted parameter retains the builder's default.
These modifiers apply to actor placement/offsets, picture transforms/exits, camera
transitions (including stage cameras), and UI nodes. They do not change waiting semantics.

```hks
alice.at(.right).time(0.6).easing(.easeOut)
camera().zoom(1.2).easing(.cubicBezier(0.25, 0.1, 0.25, 1)).time(0.8).await()
text("Hello").phaseAnimator([.rotation(-2), .rotation(2)])
    .time(0.7).easing(.easeInOut).repeatForever()
```

Predefined curves include `.linear`, `.easeIn`, `.easeOut`, `.easeInOut`,
`.smoothStep`, `.easeOutSine`, `.easeInOutSine`, `.bounce`, and `.easeOutBack`.
`.cubicBezier(x1, y1, x2, y2)` fixes the endpoints at (0, 0) and (1, 1).
X controls must be in 0..=1; finite Y controls in -100000..=100000 allow overshoot.
Sampling solves the curve's X coordinate before evaluating Y; curves are serialized
as data, with no callbacks or function pointers in saves.

Explicit durations must be finite and in 0..=3600 seconds. Persistent UI timelines
require a positive duration. `.repeatForever()` belongs to UI nodes, not finite
story transitions. The old `.animation(...)` modifier has been removed.

## Easing presets

All presets use camelCase selectors in HKS:

- `linear`, `smoothStep`, `spring`
- `easeInQuad`, `easeOutQuad`, `easeInOutQuad`
- `easeInCubic`, `easeOutCubic`, `easeInOutCubic`
- `easeInQuart`, `easeOutQuart`, `easeInOutQuart`
- `easeInQuint`, `easeOutQuint`, `easeInOutQuint`
- `easeInSine`, `easeOutSine`, `easeInOutSine`
- `easeInExpo`, `easeOutExpo`, `easeInOutExpo`
- `easeInCirc`, `easeOutCirc`, `easeInOutCirc`
- `easeInBounce`, `easeOutBounce`, `easeInOutBounce`
- `easeInBack`, `easeOutBack`, `easeInOutBack`
- `easeInElastic`, `easeOutElastic`, `easeInOutElastic`

The directional families follow the conventional [easings.net](https://easings.net/)
curves. Spring is an engine-defined, normalized damped step response:
`r(t) = 1 - exp(-6t) * (cos(12t) + 0.5*sin(12t))`, sampled as `r(t)/r(1)`.
It is a duration-bounded curve, not a physics simulation; changing `.time()`
stretches its timeline. Every preset returns exactly 0 and 1 at the endpoints.
Spring, Back and Elastic preserve overshoot; consumers must clamp constrained
properties such as opacity, rather than clamping the curve itself.

The existing `easeIn`, `easeOut`, `easeInOut` shorthand retains quadratic
behavior; `bounce` is the out-bounce curve, and `ease` is smoothstep.
