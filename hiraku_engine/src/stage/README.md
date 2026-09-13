# Spatial stages

Implemented: `.stage.hson` asset loading, relative scene dependencies,
validated camera presets and anchors, script handles, resumable camera transitions,
composed-character placement, independent named views and one owned ECS model
hierarchy. This module does not implement trial rules or evidence choices.

```hks
.{
    scene: "court.glb#Scene0",
    exposure: 1.0,
    defaultCamera: "wide",
    anchors: .{
        alice: .{ position: (1, 0, -2), rotation: (0, 90, 0) }
    },
    cameras: .{
        wide: .{
            pose: .{ position: (0, 2, 10) },
            projection: .{ kind: "perspective", fov: 60, near: 0.1, far: 100 }
        }
    }
}
```

Coordinates are Bevy stage-local units (+Y up, unrotated camera looks down -Z).
`exposure` is an optional positive linear exposure multiplier applied only to
this stage's offscreen cameras. `1.0` gives neutral exposure; omission retains
Bevy's physical-camera default. It does not rewrite light intensities or change
the outer application's camera/UI. Imported non-physical lighting needs an
explicit exposure convention, not an arbitrary global brightness boost.

`ambientBrightness` optionally overrides white ambient illumination for stage
cameras only (cd/m², finite and non-negative). Omission inherits the host's
global ambient light. Configure it alongside exposure to avoid overexposure.
XYZ rotation and FOV are degrees. Tuples represent spatial coordinates, not
lists. Camera scale must be one; zoom belongs to the projection. Unknown fields,
missing default cameras, invalid clipping planes and non-finite poses fail
validation. Unity conversion belongs to import tooling, not this runtime.

The loader retains the scene as an AssetServer dependency. `StageDefinition::spawn`
creates a `StageRoot`, a `WorldAssetRoot` child and named `StageAnchor` children.
Despawn the root to release its hierarchy. Presets are data, not Camera entities.
Use `StageAnchor` + `GlobalTransform` for runtime spatial references after Bevy
transform propagation. `StageDefinition::anchor` returns a stage-local pose.

## Script integration

```hks
let trial = stage.open("stages/trial.stage.hson")
let alice = char("alice")
trial.place(alice, "alice")
alice.at(.pos(0, 0)).scale(1).show()
trial.camera("wide").time(1.2).easing(.easeOut).await()
scene.hideCharacters(0).await()
trial.close()
```

- `stage.open` waits for scene dependencies and Bevy World instantiation and returns a typed stage handle;
  it must report loading failure without leaving a half-mounted stage.
- A stage creates one offscreen camera per configured view, not per preset.
  Camera transitions are ordinary awaitable presentation tasks with seq/par rules.
- `place(actor, anchorName)` attaches the existing composed plane to a named
  anchor. Actor position/motion stays in local pixel coordinates; use the anchor
  scale to convert pixels to world units. Part masks and opacity remain composed
  before the final plane is transformed. Actors keep their own lifetime; close
  removes the model/anchors, not the project's characters.
- Snapshots store asset identity, each view's camera/pose/opacity, anchor assignments and
  animation progress, never ECS Entity IDs or renderer-owned handles.
- Restore rebuilds the owned hierarchy and resolves names before resuming tasks.
- Views support full-frame composition, independent fades and diagonal half-plane
  clipping: `trial.clipView("overlay", .left(25, 10))`. Position is a percentage
  across the canvas; angle is degrees from vertical. `.right(...)` keeps the other
  half and `.full` removes clipping. UVs and the camera framing are unchanged.
- Future named model/camera animation clips reuse the task completion protocol.

An orbit camera uses `pose: .{ pivot: (0, 2, 0), rotation: (0, 30, 0),
distance: 4, outward: false }` instead of a fixed position. Orbit rotation is
pitch/yaw/roll in degrees, composed YXZ. Interpolation follows angles and radius,
not a chord through the scene. Each angle takes the shortest arc across 0/360;
explicit full revolutions require intermediate authored camera shots.
`outward: true` looks away from the pivot.
Changing rig kinds uses ordinary world-pose interpolation.

`unlit: true` supports pre-lit models by cloning their materials per stage.
Original model/material assets are never mutated. Model surfaces, anchored actors
and lights enter spatial layer 4. Offscreen cameras render only this layer.
The primary camera composes their images beneath curtains and UI in canvas
coordinates. Hidden views continue their timeline but disable GPU rendering.
View entities and image handles are released when the stage closes or changes.

```hks
trial.camera("wide").track("main").await()
trial.camera("closeup").track("overlay").await()
trial.camera("closeupEnd").track("overlay").time(4).easing(.easeOutSine)
trial.showView("overlay").time(0.35).easing(.linear).await()
trial.hideView("overlay").time(0.35).easing(.linear).await()
```

`main` starts visible. Other views start hidden; their order is their creation
order. Showing a view requires configuring its camera first. Moving a hidden
view does not show it, and hiding a view does not reset its camera. The
presentation surfaces ignore picking and cannot swallow dialogue input.

Engine primitives intentionally do not encode trial rules. The game currently
uses them for its Trial00 entrance and first close-up transitions; later diagonal shots and
trial interactions remain a separate migration task.
