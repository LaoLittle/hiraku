//! Package preload ownership, independent of scene ownership. Dropping a
//! preload handle never invalidates a texture still used by a visible entity.
use crate::vfs::HdpVfs;
use bevy::{asset::LoadState, prelude::*, ui::FocusPolicy};
use hiraku_hdp::dependencies::{DEPENDENCY_MANIFEST, DEPENDENCY_VERSION, DependencyManifest};
use std::collections::{BTreeMap, BTreeSet};

/// Hosts can replace the loading presentation while keeping the same gate.
/// The engine always installs an input-blocking full-canvas root underneath it.
#[derive(Resource, Default)]
pub struct LoadingScreen {
    /// Disable the built-in black background when providing a custom renderer.
    pub custom: bool,
}

#[derive(Resource, Default)]
pub struct ScriptDependencies {
    manifests: BTreeMap<String, Option<DependencyManifest>>,
    handles: BTreeMap<String, Handle<Image>>,
    resident: BTreeSet<String>,
    requested: Option<BTreeSet<String>>,
    pub loading: bool,
    pub error: Option<String>,
}

impl ScriptDependencies {
    pub fn progress(&self, assets: &AssetServer) -> f64 {
        if self.handles.is_empty() {
            return 1.0;
        }
        self.handles
            .values()
            .filter(|handle| matches!(assets.get_load_state(handle.id()), Some(LoadState::Loaded)))
            .count() as f64
            / self.handles.len() as f64
    }
    /// Polling is cheap after request construction. No blocking waits, locks,
    /// direct GPU eviction, or script execution while the dependency set loads.
    pub(crate) fn prepare(
        &mut self,
        vfs: &HdpVfs,
        assets: &AssetServer,
        scripts: &[String],
    ) -> Result<bool, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let request: BTreeSet<_> = scripts.iter().cloned().collect();
        if self.requested.as_ref() != Some(&request) {
            let mut needed = BTreeSet::new();
            for script in scripts {
                let (root, relative) = package_path(script);
                let manifest_path = format!("{root}{DEPENDENCY_MANIFEST}");
                if !self.manifests.contains_key(&root) {
                    let manifest = if vfs.exists(&manifest_path) {
                        let source = vfs.read_text(&manifest_path).map_err(|e| e.to_string())?;
                        let manifest: DependencyManifest =
                            hiraku_script::hson::from_str(&source)
                                .map_err(|e| format!("{manifest_path}: {e}"))?;
                        if manifest.version != DEPENDENCY_VERSION {
                            return Err(format!(
                                "unsupported dependency manifest version {}",
                                manifest.version
                            ));
                        }
                        Some(manifest)
                    } else {
                        None
                    }; // Loose examples without a build step remain lazy.
                    self.manifests.insert(root.clone(), manifest);
                }
                if let Some(manifest) = &self.manifests[&root] {
                    let paths = manifest.scripts.get(&relative).ok_or_else(|| {
                        format!(
                            "script `{script}` is missing from {manifest_path}; rebuild the package"
                        )
                    })?;
                    needed.extend(paths.iter().map(|p| asset_path(&root, p)));
                    self.resident
                        .extend(manifest.resident.iter().map(|p| asset_path(&root, p)));
                }
            }
            needed.extend(self.resident.iter().cloned());
            self.handles.retain(|path, _| needed.contains(path));
            for path in needed {
                self.handles
                    .entry(path.clone())
                    .or_insert_with(|| assets.load(path));
            }
            self.requested = Some(request);
            self.loading = self.handles.values().any(|handle| {
                !matches!(assets.get_load_state(handle.id()), Some(LoadState::Loaded))
            });
            // Yield only for real I/O. A metadata-only call/goto with already
            // resident dependencies must not flash a loading screen.
            if self.loading {
                return Ok(false);
            }
        }
        self.loading = false;
        for (path, handle) in &self.handles {
            match assets.get_load_state(handle.id()) {
                Some(LoadState::Loaded) => (),
                Some(LoadState::Failed(error)) => {
                    let error = format!("failed to preload `{path}`: {error}");
                    self.error = Some(error.clone());
                    self.loading = true;
                    return Err(error);
                }
                _ => self.loading = true,
            }
        }
        Ok(!self.loading)
    }
}

fn asset_path(root: &str, path: &str) -> String {
    if path.contains("://") {
        path.into()
    } else {
        format!("{root}{path}")
    }
}
fn package_path(script: &str) -> (String, String) {
    if let Some(path) = script.strip_prefix("hdp://") {
        if let Some((archive, path)) = path.split_once('/') {
            return (format!("hdp://{archive}/"), path.into());
        }
    }
    (String::new(), script.trim_start_matches('/').into())
}

#[derive(Component)]
pub struct LoadingScreenRoot;

pub(crate) fn loading_screen(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    state: Res<ScriptDependencies>,
    config: Res<LoadingScreen>,
    roots: Query<Entity, With<LoadingScreenRoot>>,
) {
    if state.loading && state.error.is_none() { redraw.request(); }
    if !state.loading {
        for root in &roots {
            commands.entity(root).try_despawn();
        }
    } else if roots.is_empty() {
        commands.spawn((
            LoadingScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..Default::default()
            },
            BackgroundColor(if config.custom {
                Color::NONE
            } else {
                Color::BLACK
            }),
            FocusPolicy::Block,
            Pickable::default(),
            GlobalZIndex(i32::MAX - 1),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_paths_preserve_archive_identity() {
        assert_eq!(
            package_path("hdp://story.hdp/scenes/first.hks"),
            ("hdp://story.hdp/".into(), "scenes/first.hks".into())
        );
        assert_eq!(
            package_path("scenes/first.hks"),
            ("".into(), "scenes/first.hks".into())
        );
        assert_eq!(
            asset_path("hdp://story.hdp/", "textures/alice.png"),
            "hdp://story.hdp/textures/alice.png"
        );
    }

    #[test]
    fn navigation_releases_only_preload_ownership_and_pins_ui() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Image>();
        let assets = app.world().resource::<AssetServer>();
        let manifest = DependencyManifest {
            version: DEPENDENCY_VERSION,
            scripts: BTreeMap::from([
                ("alice.hks".into(), BTreeSet::from(["alice.png".into()])),
                ("bob.hks".into(), BTreeSet::from(["bob.png".into()])),
            ]),
            resident: BTreeSet::from(["ui.png".into()]),
            conservative: BTreeMap::new(),
        };
        let mut state = ScriptDependencies::default();
        state.manifests.insert(String::new(), Some(manifest));
        let vfs = HdpVfs::new("unused-test-root"); // Cached manifest; no filesystem reads.
        assert!(
            !state
                .prepare(&vfs, assets, &["alice.hks".into()])
                .expect("request alice")
        );
        let scene_owner = state.handles["alice.png"].clone();
        let ui = state.handles["ui.png"].id();
        assert!(
            !state
                .prepare(&vfs, assets, &["alice.hks".into(), "bob.hks".into()])
                .expect("call retains caller")
        );
        assert!(state.handles.contains_key("alice.png"));
        assert!(
            !state
                .prepare(&vfs, assets, &["bob.hks".into()])
                .expect("goto bob")
        );
        assert!(!state.handles.contains_key("alice.png"));
        assert!(scene_owner.is_strong());
        assert_eq!(state.handles["ui.png"].id(), ui);
        assert!(state.handles.contains_key("bob.png"));
    }

    #[test]
    fn missing_manifest_stays_lazy_but_missing_entry_is_an_error() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Image>();
        let assets = app.world().resource::<AssetServer>();
        let vfs = HdpVfs::new("unused-test-root");
        let mut state = ScriptDependencies::default();
        state.manifests.insert(String::new(), None);
        assert!(
            state
                .prepare(&vfs, assets, &["alice.hks".into()])
                .expect("loose example")
        );
        state.manifests.insert(
            String::new(),
            Some(DependencyManifest {
                version: DEPENDENCY_VERSION,
                scripts: BTreeMap::new(),
                resident: BTreeSet::new(),
                conservative: BTreeMap::new(),
            }),
        );
        assert!(
            state
                .prepare(&vfs, assets, &["bob.hks".into()])
                .expect_err("stale manifest")
                .contains("rebuild")
        );
    }
}
