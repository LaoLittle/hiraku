# Hiraku examples

Every example owns an ordinary asset directory and selects
`RuntimeAssetMode::Directory`; no HDP package or build script is involved.

```sh
cargo run -p hiraku-app --example feature_showcase
cargo run -p hiraku-app --example save_restore
cargo run -p hiraku-app --example widget_showcase
```

- `feature_showcase` demonstrates script-defined screens, a custom dialogue
  component, a live overlay, glossary HSON, bindings, and UI animation.
- `save_restore` replaces the dialogue role with a script-defined component,
  exposes named save/load slots through closure-only buttons, and demonstrates
  restoring VM, UI, and scene state.
- `widget_showcase` demonstrates text, buttons, images, picture toggles,
  checkboxes, sliders, single-line text inputs, progress, scrolling, layout,
  and animation. The two columns scroll independently. It includes accepted
  and ignored edits, explicit commits, disabled controls, and model resets.
  Its tiny color atlas is original example artwork; `generate_palette.py` can
  regenerate it but is not required to build or run the example.
