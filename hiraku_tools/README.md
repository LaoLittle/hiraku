# Build-time dependency manifests

Use `hiraku-tools` as a **build dependency**, not a runtime dependency:

```rust,ignore
hiraku_tools::pack_directory(source, output, hiraku_hdp::PackOptions::default())?;
```

This streams original assets into HDP and injects exactly one
`dependencies.manifest.hson` at its root, in the bootstrap volume. It does not
encode UASTC, modify source descriptors, or write generated metadata into the
source directory. The HDP crate owns only the shared manifest schema; HKS
analysis remains here.

The analysis follows script function/global references, both control-flow
branches, closure bodies, and finite string sets passed through function
parameters/returns. It maps texture-region IDs to deduplicated image paths and
character references to their part images. `goto` destinations are separate
entries rather than transitive preload dependencies. UI entries contribute
their image sets to the resident set, including gallery content referenced by
UI. Runtime storage thumbnails are external images, not package dependencies.

Unknown computed catalog references conservatively include their resource
family and are recorded under `conservative` (and reported while building).
This is a static over-approximation, not execution of project scripts. Native
extensions that consume resource names need corresponding analysis support;
arbitrary externally generated paths still use normal on-demand asset loading.

Inspect a directory without packing or launching a game:

```sh
cargo run -p hiraku-tools --example dependencies -- ASSET_ROOT
```

## Runtime behavior

The engine holds strong preload handles for the target script and resident UI.
Calls additionally retain caller dependencies. Goto drops obsolete **preload
ownership**, not live scene/UI ownership; Bevy frees images when their last
owner releases them. No forced removal from `Assets<Image>` occurs.

Script entry waits for load completion. Failure reports an error and stops at
the loading gate rather than starting an incomplete scene. A package manifest
missing a script requires a rebuild. Loose examples without a manifest retain
on-demand loading.

The default loading view is black and blocks input. To customize later loads,
configure a UI role in startup (the initial bootstrap uses the black fallback):

```hks
ui.set("loading", "ui/loading.ui.hks")
```

The role receives one `Float` argument, the loaded image count divided by the
requested image count (not downloaded bytes). It rerenders on percentage
changes, not every frame. It must not perform navigation or wait for input:

```hks
import ui.widgets.*
@ui
global fn main(fraction: Float) -> UiNode {
    canvas {
        text("Loading…").at(.rel(50, 50))
    }
}
```

Hosts can alternatively observe `ScriptDependencies` and customize the
presentation with `LoadingScreen`/`LoadingScreenRoot`.
