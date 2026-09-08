# Hiraku examples

Every example owns an ordinary asset directory and selects
`RuntimeAssetMode::Directory`; no HDP package or build script is involved.

```sh
cargo run -p hiraku-app --example feature_showcase
cargo run -p hiraku-app --example save_restore
cargo run -p hiraku-app --example widget_showcase
```

- `feature_showcase` demonstrates script-defined screens, a custom dialogue
  component, a live overlay, glossary HSON, reactive expressions, and UI animation.
- `save_restore` replaces the dialogue role with a script-defined component,
  exposes named save/load slots through closure-only buttons, and demonstrates
  restoring VM, UI, and scene state.
  Its parameterized `slots.ui.hks` owns the slot identifiers, layout, thumbnail
  presentation and save/load policy; the engine only provides storage primitives.
- `widget_showcase` demonstrates text, buttons, images, picture toggles,
  checkboxes, sliders, single-line text inputs, progress, scrolling, layout,
  and animation. The two columns scroll independently. It includes accepted
  and ignored edits, explicit commits, disabled controls, and model resets.
  Its tiny color atlas is original example artwork; `generate_palette.py` can
  regenerate it but is not required to build or run the example.
  The image viewer demonstrates `ui.open("viewer.ui.hks", imageName, title)`
  from an `onClick` closure. Relative paths resolve against the calling UI file;
  registered role names work as well.

## Parameterized UI

```hks
@ui
global fn viewer(imageName: String, title: String) -> UiNode {
    canvas { image(imageName); text(title) }
}
```

Both story calls and UI callbacks can pass plain data to typed `@ui` parameters.
The target signature is checked when the document is invoked; dynamically
selected paths cannot be statically linked to one signature. UI callback opens
push a modal screen, and `ui.close()` returns to the previous screen; these
callbacks do not suspend waiting for a result. Story `ui.open` still waits for
the UI result.

Image arguments are texture catalog names, not GPU handles. Input transport
currently supports strings, booleans, numbers, lists, and anonymous records.
Unsupported optional/tuple/nominal values and live handles produce an error
instead of lossy conversion.
