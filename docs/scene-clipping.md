# Shared scene clipping

Named rectangles clip pictures and composed characters in the same world XY
coordinate system. They do not allocate additional cameras or render textures.

```hks
scene.clipRect("window", 400, 800).at(.pos(100, 20)).rotation(-10)
scene.clipPicture("room", "window")
scene.picture("room", "textures/room")
char("alice").clip("window").show()
```

Dimensions are canvas world units. The default center is `(0, 0)`; rotation is
counterclockwise in degrees. `.at` currently requires `.pos`, avoiding implicit
dependence on a fixed canvas resolution. Builders commit at statement end and
must not be reused later. Redefining a region updates every associated target.
Attaching to an unknown region is an error, not an implicit unclipped fallback.

```hks
char("alice").clip(null)        // Detach this display identity only.
scene.clipPicture("room", null)
scene.removeClip("window")     // Remove the region and detach all its targets.
```

Associations survive hide/show and texture replacement. Actor aliases share a
display identity; clones have independent associations. Region definitions and
target associations are included in scene saves (format version 24), independent
of VM snapshots. Old save versions are rejected explicitly.

The clip operates on the final surface, after per-part atlas/mask composition.
It neither changes UI clipping nor replaces the internal alpha-mask pipeline.
The rectangle itself has hard edges, no animated geometry and no
`.await()`/`.fade()` method. Optional borders are independent scene pictures:

```hks
scene.picture("border", "textures/border")
    .size(440, 840).slice(20, 20, 20, 20)
    .tint(255, 255, 255, 204).fade(300)
seq {
    scene.hidePicture("border").fade(200).await()
    scene.removeClip("window")
}
```

`size` is measured in local canvas units, before the picture's transform scale.
`slice(left, top, right, bottom)` preserves source-pixel corner sizes while
stretching the center; an undersized destination proportionally shrinks the
border widths. Atlas sampling stays inside the selected source rectangle.
`tint` accepts four u8-range integers in RGBA order. These fields commit with the
picture at statement end and survive scene saves. Explicit size bypasses
background auto-fit; ordinary unsized pictures retain their existing behavior.

Clipping edits are immediate ordered state changes, including inside seq/par.
Only the picture fade participates in the animation completion protocol. A
saved sequence resumes the pending fade before executing the clip removal.
