//! Keep reactive runners awake until the requested stage frame reaches rendering.
//! CPU asset readiness alone does not cover GPU uploads or pipeline compilation.
use bevy::render::{
    Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
    erased_render_asset::ErasedRenderAssets, mesh::RenderMesh, render_asset::RenderAssets,
    render_resource::PipelineCache,
};
use bevy::{ecs::system::SystemParam, prelude::*};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Resource, Default)]
struct PendingFrame {
    requested: u64,
    completed: Arc<AtomicU64>,
}

impl PendingFrame {
    fn request(&mut self) {
        self.requested = self
            .requested
            .checked_add(1)
            .expect("stage frame generation exhausted");
    }
    fn pending(&self) -> bool {
        self.completed.load(Ordering::Acquire) < self.requested
    }
}

#[derive(SystemParam)]
pub(crate) struct StageRedraw<'w, 's> {
    redraw: crate::redraw::Redraw<'w, 's>,
    pending: Option<ResMut<'w, PendingFrame>>,
}

impl StageRedraw<'_, '_> {
    pub fn request(&mut self) {
        self.redraw.request();
        if let Some(pending) = self.pending.as_mut() {
            pending.request();
        }
    }
}

#[derive(Resource)]
struct ExtractedFrame {
    generation: u64,
    completed: Arc<AtomicU64>,
    meshes: Vec<AssetId<Mesh>>,
    materials: Vec<bevy::asset::UntypedAssetId>,
    surfaces_ready: bool,
}

fn extract(
    mut commands: Commands,
    pending: Extract<Res<PendingFrame>>,
    surfaces: Extract<
        Query<
            (Option<&Mesh3d>, Option<&MeshMaterial3d<StandardMaterial>>),
            With<super::runtime::StageSurface>,
        >,
    >,
    views: Extract<
        Query<
            (
                Option<&Mesh3d>,
                Option<&MeshMaterial3d<crate::render::world_sprite::WorldSpriteMaterial>>,
            ),
            With<super::views::ViewSurface>,
        >,
    >,
) {
    let mut meshes = Vec::new();
    let mut materials = Vec::new();
    let mut surfaces_ready = true;
    if pending.pending() {
        for (mesh, material) in &surfaces {
            if let Some(mesh) = mesh {
                meshes.push(mesh.id());
            }
            if let Some(material) = material {
                materials.push(material.id().untyped());
            }
        }
        // The offscreen target is not visible until its canvas sprite has
        // acquired a mesh/material too; this can occur a schedule later.
        for (mesh, material) in &views {
            match (mesh, material) {
                (Some(mesh), Some(material)) => {
                    meshes.push(mesh.id());
                    materials.push(material.id().untyped());
                }
                _ => surfaces_ready = false,
            }
        }
    }
    commands.insert_resource(ExtractedFrame {
        generation: pending.requested,
        completed: pending.completed.clone(),
        meshes,
        materials,
        surfaces_ready,
    });
}

fn acknowledge(
    frame: Option<Res<ExtractedFrame>>,
    pipelines: Res<PipelineCache>,
    meshes: Option<Res<RenderAssets<RenderMesh>>>,
    materials: Option<Res<ErasedRenderAssets<bevy::pbr::PreparedMaterial>>>,
) {
    let Some(frame) = frame else { return };
    if !frame.surfaces_ready
        || pipelines.waiting_pipelines().next().is_some()
        || frame.meshes.iter().any(|id| {
            meshes
                .as_ref()
                .is_none_or(|assets| assets.get(*id).is_none())
        })
        || frame.materials.iter().any(|id| {
            materials
                .as_ref()
                .is_none_or(|assets| assets.get(*id).is_none())
        })
    {
        return;
    }
    // A pipelined render frame may be older than the main world's request.
    // Acknowledge only the generation actually extracted into this frame.
    frame
        .completed
        .fetch_max(frame.generation, Ordering::Release);
}

fn keep_awake(pending: Res<PendingFrame>, mut redraw: crate::redraw::Redraw) {
    if pending.pending() {
        redraw.request();
    }
}

pub(super) fn register(app: &mut App) {
    // No render sub-app means a headless embedding: never create an unfulfillable wait.
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    app.init_resource::<PendingFrame>()
        .add_systems(Last, keep_awake);
    let render = app.sub_app_mut(RenderApp);
    render
        .add_systems(ExtractSchedule, extract)
        .add_systems(Render, acknowledge.in_set(RenderSystems::Cleanup));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_registration_does_not_wait_for_a_render_world() {
        let mut app = App::new();
        register(&mut app);
        app.update();
        assert!(!app.world().contains_resource::<PendingFrame>());
    }

    #[test]
    fn older_render_frames_cannot_acknowledge_new_stage_work() {
        let mut pending = PendingFrame::default();
        assert!(!pending.pending());
        pending.request();
        let extracted = pending.requested;
        pending.request();
        pending.completed.store(extracted, Ordering::Release);
        assert!(pending.pending());
        pending
            .completed
            .store(pending.requested, Ordering::Release);
        assert!(!pending.pending());
    }

    #[test]
    fn redraws_stop_after_render_acknowledgement_without_pointer_input() {
        let mut app = App::new();
        app.init_resource::<PendingFrame>()
            .add_message::<bevy::window::RequestRedraw>()
            .add_systems(Last, keep_awake);
        let mut cursor =
            bevy::ecs::message::MessageCursor::<bevy::window::RequestRedraw>::default();
        app.world_mut().resource_mut::<PendingFrame>().request();
        for _ in 0..3 {
            app.update();
            assert_eq!(
                cursor
                    .read(
                        app.world()
                            .resource::<Messages<bevy::window::RequestRedraw>>()
                    )
                    .count(),
                1
            );
        }
        let state = app.world().resource::<PendingFrame>();
        state.completed.store(state.requested, Ordering::Release);
        app.update();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<Messages<bevy::window::RequestRedraw>>()
                )
                .count(),
            0
        );
    }
}
