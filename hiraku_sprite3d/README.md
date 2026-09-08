# hiraku-sprite3d

An engine-independent Bevy renderer for flat, unlit images in a 3D scene.
`Sprite3d` and `Billboard` are separate components. No cameras are created.

```rust,no_run
use bevy::prelude::*;
use hiraku_sprite3d::{Sprite3d, Sprite3dPlugin, Billboard};

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    let camera = commands.spawn((Camera3d::default(),
        Transform::from_xyz(0.0, 0.0, 10.0))).id();
    commands.spawn((
        Sprite3d { custom_size: Some(Vec2::new(2.0, 3.0)),
            ..Sprite3d::from_image(assets.load("sprite.png")) },
        Billboard::new(camera),
    ));
}
```

Install `Sprite3dPlugin` alongside Bevy's rendering plugins. The plugin owns
`Mesh3d` and `MeshMaterial3d<Sprite3dMaterial>` on sprite entities. It reuses mesh
and material assets when authoring data changes, without rewriting Transform.
Removing Sprite3d removes these generated rendering components. Do not share or
edit the generated per-entity mesh/material handles between independently edited
sprites. Standard Bevy's mesh vertex shader transforms the quad; the custom
fragment shader handles atlas composition. Mesh picking hits the quad, not its
non-transparent pixels.

`cargo run -p hiraku-sprite3d --example layered` shows a procedural atlas with
animated group opacity, mask/multiply composition and all three billboard modes.
No external assets are needed.

`cargo run -p hiraku-sprite3d --example billboard` loads this crate's
`assets/bevy_bird.png` and rotates it about its Y axis at the screen center
(60 degrees per second), like a coin. Its reverse side is black, preserving
the image alpha silhouette via `backface_color`. This example intentionally
omits Billboard, which would keep the sprite facing the camera. The asset root is resolved from the
crate directory, independently of the launch directory.

## Layer composition

Bevy's `TextureAtlas` / `TextureAtlasLayout` are supported directly:

```rust,no_run
use bevy::prelude::*;
use hiraku_sprite3d::Sprite3d;

fn spawn_sheet(mut commands: Commands, assets: Res<AssetServer>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>) {
    let layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(32, 48), 4, 2, None, None));
    commands.spawn(Sprite3d::from_atlas(
        assets.load("sheet.png"), TextureAtlas { layout, index: 0 }));
}
```

Use `if let Some(atlas) = &mut sprite.texture_atlas { atlas.index = 1; }`
to switch cells. `SpriteLayer.texture_atlas`
can override the sprite's default selection. All layouts still reference the
same sprite image. Source cropping is defined exclusively by the selected
`TextureAtlasLayout` cell; there is no separate public `rect` field.

Layout edits/hot reload update resolved UVs and natural size even without a
Sprite3d change. Missing layouts defer rendering; invalid indices remove stale
rendering and report a diagnostic instead of sampling the entire atlas. The
`layered` example uses Bevy atlas layouts for every composed layer.

For explicit material construction with atlas references, use
`Sprite3dMaterial::from_sprite(&sprite, &layouts)`. The `TryFrom<&Sprite3d>`
convenience constructor only resolves sources that need no layout assets.

- One atlas per sprite; up to 32 layers, ordered back-to-front.
- Atlas rectangles use top-left pixel coordinates. Layer bounds use normalized
  top-left coordinates inside the quad. The quad is centered, with normal +Z.
- With no atlas selection, a layer samples the full image. A single full-quad atlas
  layer uses its source rectangle's natural size (`Sprite3d::from_atlas`). Multiple
  layers default to the atlas size; use `custom_size` for a composed sprite.
- Normal source-over and multiply blending preserve per-layer alpha. Multiply
  is **inside the group**, not against other scene objects.
- Masks have eight references, local to the sprite. Writers must precede readers.
  Writers union coverage using max; cutoff discards low-alpha coverage without
  converting surviving coverage to opaque. Writers can optionally also render.
  Read + multiply is supported. These are alpha masks, not stencil emulation.
  `StencilWrite` additionally supports binary coverage from untinted source
  alpha at an explicit cutoff, for sprite-based stencil-style masks; it does
  not allocate or use a GPU stencil attachment.
- Overall `Sprite3d.color` tint and alpha apply **after** composition, then the
  result is blended with the scene as premultiplied alpha. A translucent face
  remains translucent; fading the entire character does not expose its lower
  layers through an otherwise opaque upper layer.
- Textures should contain straight-alpha color. Use sRGB image formats for color
  atlases so sampling converts RGB to linear space. Do not premultiply atlas data.
- Single quad, single material, single atlas sampler. No additional camera or
  offscreen target. The bounded uniform data needs 2640 bytes. Invalid layer
  counts/geometry are rejected, never silently truncated. Sampling uses mip 0
  and clamps to texel centers to prevent adjacent atlas entries bleeding.

Layer iteration costs fragment work: fewer draw calls does not guarantee higher
performance for every character. This path is suitable for modest layered atlas
sprites. A cached offscreen compositor remains an alternative for complex,
mostly static characters, without changing the authoring layer model.

## Billboard

Billboard works on any mesh. It changes only Transform.rotation before transform
propagation, using current-frame camera/parent transforms. Specify the camera
entity explicitly: one mesh has one orientation, even with multiple views.

- `ScreenAligned`: parallel to the camera plane, including orthographic cameras.
- `Spherical`: +Z points at camera position, world +Y up.
- `CylindricalY`: rotates only around world +Y.
- `roll`: additional local Z rotation, radians.

Use positively and uniformly scaled, non-sheared parents. Billboard owns rotation;
put independent model rotation on a child or use `roll`. A missing camera or
degenerate direction leaves the previous rotation unchanged.

Hiraku's character renderer composes logical parts through this crate. The
engine owns scene snapshots and expression transitions; this crate does not
depend on those concepts. Unit/shader validation tests need no GPU; actual
visual verification remains separate.

## World-space clipping

`Sprite3d.clip = Some(ClipRect::new(center, size, degrees)?)` clips the final
composed surface against an oriented rectangle in world XY. `None` disables
clipping. Geometry must be finite and both dimensions positive. Reuse the same
rectangle for multiple sprites to give them a shared window, independently of
their transforms, atlas cells, intrinsic part alpha or stencil-style masks.
This is not a camera-space scissor and does not add a border or a render pass.
Picking still uses the whole quad.
