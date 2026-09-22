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

#[derive(Resource)]
pub struct ScriptDependencies {
    /// Budget for speculative uploads; visible scene/UI owners are independent.
    pub preload_budget_bytes: u64,
    pub lookahead_steps: usize,
    pub retain_steps: usize,
    manifests: BTreeMap<String, Option<DependencyManifest>>,
    handles: BTreeMap<String, UntypedHandle>,
    cpu_sources: BTreeSet<String>,
    requested: Option<BTreeSet<String>>,
    frontier: BTreeSet<(String, usize)>,
    positions: Vec<(String, usize)>,
    history: std::collections::VecDeque<Vec<String>>,
    desired: BTreeSet<String>,
    /// The current forward window, excluding retained historical windows.
    ahead: BTreeSet<String>,
    pub(crate) revision: u64,
    pub(crate) closed: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for ScriptDependencies {
    fn default() -> Self {
        Self {
            preload_budget_bytes: 128 * 1024 * 1024,
            lookahead_steps: 32,
            retain_steps: 4,
            manifests: BTreeMap::new(),
            handles: BTreeMap::new(),
            cpu_sources: BTreeSet::new(),
            requested: None,
            frontier: BTreeSet::new(),
            positions: Vec::new(),
            history: Default::default(),
            desired: BTreeSet::new(),
            ahead: BTreeSet::new(),
            revision: 0,
            closed: false,
            loading: false,
            error: None,
        }
    }
}

mod window;

/// Only execution progress changes cache ownership, never wall-clock time.
pub(crate) fn update_resource_window(
    runtime: Res<crate::script::ScriptRuntimeState>,
    vfs: Option<Res<crate::vfs::VfsResource>>,
    assets: Res<AssetServer>,
    mut state: ResMut<ScriptDependencies>,
) {
    if state.loading || state.error.is_some() {
        return;
    }
    let Some(vfs) = vfs else {
        return;
    };
    if !runtime
        .story
        .as_ref()
        .is_some_and(|story| story.has_executions())
        && runtime.call_stack.is_empty()
    {
        state.finish();
        return;
    }
    state.closed = false;
    let mut positions = Vec::new();
    if let Some(story) = &runtime.story {
        positions.extend(story.resource_positions());
    }
    for frame in &runtime.call_stack {
        positions.extend(frame.story.resource_positions());
    }
    if positions.is_empty() {
        return;
    }
    state.requested = None;
    if state.positions == positions {
        return;
    }
    state.positions = positions.clone();
    let mut frontier = BTreeSet::new();
    for (path, offset) in positions {
        if let Err(error) = state.ensure_manifest(&vfs.0, &path) {
            warn!("resource window remains lazy: {error}");
            continue;
        }
        let (root, relative) = package_path(&path);
        if let Some(Some(manifest)) = state.manifests.get(&root)
            && let Some(graph) = manifest.windows.get(&relative)
            && let Some(node) = window::locate(graph, offset)
        {
            frontier.insert((path, node));
        }
    }
    state.move_window(frontier, &assets);
}

impl ScriptDependencies {
    pub(crate) fn set_cpu_sources(&mut self, paths: BTreeSet<String>) {
        self.handles
            .retain(|path, _| self.cpu_sources.contains(path) == paths.contains(path));
        self.cpu_sources = paths;
    }
    fn finish(&mut self) {
        self.closed = true;
        self.handles.clear();
        self.history.clear();
        self.desired.clear();
        self.ahead.clear();
        self.frontier.clear();
        self.positions.clear();
        self.requested = None;
    }
    pub fn retained_images(&self) -> usize {
        self.handles.len()
    }
    pub(crate) fn protects(&self, path: &str) -> bool {
        self.desired.contains(path)
    }
    pub fn progress(&self, assets: &AssetServer) -> f64 {
        if self.handles.is_empty() {
            return 1.0;
        }
        self.handles
            .values()
            .filter(|h| matches!(assets.get_load_state(h.id()), Some(LoadState::Loaded)))
            .count() as f64
            / self.handles.len() as f64
    }
    fn ensure_manifest(&mut self, vfs: &HdpVfs, script: &str) -> Result<(), String> {
        let (root, _) = package_path(script);
        if self.manifests.contains_key(&root) {
            return Ok(());
        }
        let path = format!("{root}{DEPENDENCY_MANIFEST}");
        let manifest = if vfs.exists(&path) {
            let source = vfs.read_text(&path).map_err(|e| e.to_string())?;
            let manifest: DependencyManifest =
                hiraku_script::hson::from_str(&source).map_err(|e| format!("{path}: {e}"))?;
            if manifest.version != DEPENDENCY_VERSION {
                return Err(format!(
                    "unsupported dependency manifest version {}; rebuild the package",
                    manifest.version
                ));
            }
            Some(manifest)
        } else {
            None
        };
        self.manifests.insert(root, manifest);
        Ok(())
    }

    fn move_window(&mut self, frontier: BTreeSet<(String, usize)>, assets: &AssetServer) {
        self.closed = false;
        if self.frontier == frontier {
            return;
        }
        self.frontier = frontier;
        self.revision = self.revision.saturating_add(1);
        let mut ahead = Vec::new();
        let mut costs = BTreeMap::new();
        for (root, manifest) in &self.manifests {
            let Some(manifest) = manifest else {
                continue;
            };
            let seeds = self.frontier.iter().filter_map(|(path, node)| {
                let (owner, relative) = package_path(path);
                (owner == *root).then_some((relative, *node))
            });
            ahead.extend(
                window::collect(manifest, seeds, self.lookahead_steps, 256)
                    .into_iter()
                    .map(|path| asset_path(root, &path)),
            );
            costs.extend(
                manifest
                    .image_bytes
                    .iter()
                    .map(|(path, bytes)| (asset_path(root, path), *bytes)),
            );
        }
        // Near future wins the budget; recently traversed windows are lower priority.
        self.ahead = ahead.iter().cloned().collect();
        self.history.push_front(ahead.clone());
        self.history.truncate(self.retain_steps.saturating_add(1));
        let candidates: Vec<_> = self.history.iter().flatten().cloned().collect();
        self.desired = candidates.iter().cloned().collect();
        let mut admitted = BTreeSet::new();
        let mut remaining = self.preload_budget_bytes;
        for path in candidates {
            if admitted.contains(&path) {
                continue;
            }
            let Some(&cost) = costs.get(&path) else {
                continue;
            };
            if cost > remaining {
                continue;
            }
            remaining -= cost;
            admitted.insert(path);
        }
        self.handles.retain(|path, _| admitted.contains(path));
        for path in admitted {
            self.handles.entry(path.clone()).or_insert_with(|| {
                if self.cpu_sources.contains(&path) {
                    assets
                        .load::<crate::scene::character_composite::source::AtlasSource>(path)
                        .untyped()
                } else {
                    crate::texture::load_static_image(assets, path).untyped()
                }
            });
        }
    }

    /// Navigation waits for the target's entry window, not the whole script.
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
            let mut frontier = BTreeSet::new();
            if let Some(script) = scripts.first() {
                self.ensure_manifest(vfs, script)?;
                let (root, relative) = package_path(script);
                if let Some(Some(manifest)) = self.manifests.get(&root) {
                    let graph = manifest.windows.get(&relative).ok_or_else(|| format!("script `{script}` is missing from the dependency manifest; rebuild the package"))?;
                    if let Some(entry) = graph.entry {
                        frontier.insert((script.clone(), entry));
                    }
                }
            }
            self.move_window(frontier, assets);
            self.requested = Some(request);
        }
        self.loading = false;
        for (path, handle) in &self.handles {
            if !self.ahead.contains(path) {
                continue;
            }
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
    if state.loading && state.error.is_none() {
        redraw.request();
    }
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
mod tests;
