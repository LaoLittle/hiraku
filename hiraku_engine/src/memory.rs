//! Opt-in, platform-independent asset ownership diagnostics. These counters are
//! not process RSS: GPU allocations and allocator retention require host tools.
use bevy::prelude::*;

#[derive(Resource)]
pub struct MemoryDiagnostics {
    pub enabled: bool,
    pub interval_seconds: f32,
}
impl Default for MemoryDiagnostics {
    fn default() -> Self {
        Self {
            enabled: std::env::var_os("HIRAKU_MEMORY_DIAGNOSTICS").is_some(),
            interval_seconds: 10.0,
        }
    }
}

pub(crate) fn sample(
    config: Res<MemoryDiagnostics>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    images: Res<Assets<Image>>,
    assets: Res<AssetServer>,
    entities: Query<Entity>,
    dependencies: Res<crate::dependencies::ScriptDependencies>,
    archive: Option<Res<crate::vfs::HdpArchiveStore>>,
    runtime: Option<Res<crate::script::ScriptRuntimeState>>,
) {
    if !config.enabled {
        return;
    }
    *elapsed += time.delta_secs();
    if *elapsed < config.interval_seconds.max(1.0) {
        return;
    }
    *elapsed = 0.0;
    let mut allocations: Vec<_> = images
        .iter()
        .map(|(id, image)| (id, image.data.as_ref().map_or(0, Vec::len)))
        .collect();
    let total: usize = allocations.iter().map(|(_, bytes)| bytes).sum();
    allocations.sort_unstable_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    info!(target: "hiraku_memory", images = images.len(), image_cpu_mib = mib(total),
        hdp_compressed_mib = mib(archive.as_ref().map_or(0, |a| a.resident_bytes())),
        preloaded = dependencies.retained_images(), entities = entities.iter().count(),
        script = ?runtime.as_ref().and_then(|r| r.current_script.as_deref()), "memory sample");
    for (id, bytes) in allocations
        .into_iter()
        .filter(|(_, bytes)| *bytes >= 16 * 1024 * 1024)
        .take(8)
    {
        info!(target: "hiraku_memory", cpu_mib = mib(bytes), path = ?assets.get_path(id), ?id, "large image");
    }
}
