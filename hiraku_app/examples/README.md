# Hiraku examples

Every example owns an ordinary asset directory and selects
`RuntimeAssetMode::Directory`; no HDP package or build script is involved.

```sh
cargo run -p hiraku-app --example feature_showcase
cargo run -p hiraku-app --example save_restore
```

- `feature_showcase` demonstrates script-defined screens, a custom dialogue
  component, a live overlay, glossary HSON, bindings, and UI animation.
- `save_restore` replaces the dialogue role with a script-defined component,
  exposes named save/load slots through closure-only buttons, and demonstrates
  restoring VM, UI, and scene state.
