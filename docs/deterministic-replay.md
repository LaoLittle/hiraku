# Deterministic replay and restore

## Restore contract

A save schema version, VM ABI version, executable fingerprint, and replay
journal version are different compatibility domains. A schema mismatch must
not be treated as permission to deserialize an incompatible VM snapshot.

1. Matching VM ABI and linked fingerprints for every active/caller module:
   restore PC/registers/heap and the scene snapshot directly.
2. Otherwise, only a complete, supported journal may drive replay. Decode its
   independent envelope without decoding old VM internals.
3. Legacy saves without a complete recording cannot acquire missing history
   retrospectively. Report incompatibility; never guess choices.

Replay runs in an isolated candidate session. It must not write saves, change
user settings/unlocks, exit the app, open real modal screens, or play media.
Commit the candidate only after its event sequence and destination validate.
On failure preserve the live session and expose the divergent event.

## Journal

The v1 journal stores the entry script, seed, ordered external inputs,
compressed dialogue runs and the pending destination. Adjacent dialogue
continuations merge into a count plus a BLAKE3 chain of semantic boundaries.
This saves space without treating four arbitrary clicks as four matching lines.
The chain validates at the run's end, so replay requires isolated state.

Choice signatures include prompt, options and enabled flags. A saved choice
must still exist and be enabled. Labels alone do not uniquely identify repeated
choices: ordering and the pending boundary are also checked. Translation or
script edits that change these signatures currently require migration rather
than silent acceptance. Future compiler-authored semantic IDs may relax this.

Randomness must record both the initial generator seed/algorithm and each
observable result, with call identity and bounds. Seeds alone are insufficient
when edits change the number of draws. Time and other host reads use the same
typed external-input channel. Replay consumes results; it never samples live
providers. UI results must preserve types, including Unit and named records.

Record accepted story continuations, not physical pointer events. Revealing
the remaining characters and advancing a dialogue are different operations.
An unanswered choice at save time is a destination, not a recorded choice.

## Presentation policy

Share logical command evaluation across Live/FastForward/Replay. Replay applies
character slots, visibility, final placement, camera, clipping, picture layers,
UI roles and BGM identity while skipping durations/audio/video output. Merely
dropping animation commands would lose their state changes. Task/seq/par waits
must still complete in deterministic logical order.

A future ECS load coordinator should use explicit phases:
Read -> Compile/Validate -> Restore or Replay -> Resolve Assets -> Commit.
Work is budgeted over frames (or moved to appropriate task pools); a loading
screen cannot become visible if all work blocks in the click handler. Progress
comes from completed work, not an invented elapsed-time percentage. Input stays
blocked until commit or error recovery.

## Implementation status

Implemented: typed journal/cursor, dialogue run checks, ordered choice/random/
time result replay at the tape level, HSON/protobuf persistence (new tag 20),
and partial live recording of accepted host choices/dialogue continuations.
The saved pending boundary survives direct snapshot restoration.

Not yet implemented: complete native entropy interception, lossless arbitrary
UI result recording, logical scheduling replay, isolated scene reconstruction,
automatic version-mismatch fallback, or the asynchronous load coordinator.
Current recordings intentionally remain `complete = false`. Existing version
and fingerprint rejection is retained. Random seed zero in a newly created
partial journal is not claimed to represent a live RNG provider.

The game includes a recovered LoadingUI script/atlas, with a Float progress
argument. It is not yet automatically shown by load operations. Source bounds
are 2560x1440, icon center (2416,1312), scale 0.25, progress bar
(2176,1372,320,20). Underlay alpha is the serialized 128/255; track alpha is
64/255. The one-second icon spin is provisional, not a recovered Animator curve.
