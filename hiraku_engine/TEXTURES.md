# Texture semantics and storage

Texture descriptors can declare what their pixels mean:

```hson
.{
    name: "background/room",
    image: "room.png",
    textureType: "color",
    colorSpace: "srgb"
}
```

`textureType` defaults to `color`. `mask` is scalar coverage, and `data` preserves
independent channels. Color textures default to sRGB; mask/data default to linear.
`colorSpace` can be `srgb` or `linear` for color and mask textures. Data textures
always expose raw channel values. Conflicting metadata for the same image path
is rejected, including when multiple descriptors reference its atlas regions.

Grayscale is a storage layout, not a texture purpose. The decoder determines
R/RG/RGBA storage automatically. Color sampling reconstructs luminance and alpha
in the shader and decodes sRGB luminance without applying gamma to alpha. Mask
sampling reconstructs scalar luminance without gamma by default. Data sampling
does not replicate channels. Dedicated mask/rule and video plane samplers retain
their explicitly defined data semantics.

PNG loading uses Bevy's image loader: R8/RG8 pixels remain compact in CPU and GPU
memory. World images, Sprite3D, standard UI (including nine-sliced/tiled images),
and built-in transition materials use the sampling metadata. A runtime character
atlas combining different source formats still needs a common format; only that
packing step uses Bevy's TextureAtlasBuilder format conversion.

Custom UI shaders are responsible for interpreting their own sampled data. WESL
shaders can import `hiraku_sprite3d::sampling::decode_color`; Hiraku's custom UI
material provides sampling modes at group 1, binding 15 (`vec4<u32>`, main image
in `.x`). This does not change the layouts of existing bindings.
