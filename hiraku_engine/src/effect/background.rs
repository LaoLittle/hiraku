//! One background composition boundary within the existing Camera3d schedule.
use super::{
    kawase::{BlurPipeline, BlurWorkspace},
    post_process::{EffectPipeline, PostProcessSettings, PreparedEffectUniform, apply_effect},
    program::LayerEffectShaders,
};
use crate::scene::pictures::PictureView;
use bevy::{
    core_pipeline::blit::{BlitPipeline, BlitPipelineKey},
    core_pipeline::core_3d::{Transparent3d, main_opaque_pass_3d, main_transparent_pass_3d},
    core_pipeline::{Core3dSystems, schedule::Core3d},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        batching::NoAutomaticBatching,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_phase::{SortedRenderPhase, ViewSortedRenderPhases},
        render_resource::*,
        renderer::{RenderContext, ViewQuery},
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
    },
};
use std::collections::HashMap;

#[derive(Component, Clone, Copy, ExtractComponent)]
#[extract_app(RenderApp)]
#[require(NoAutomaticBatching)]
struct BackgroundComposition;

#[derive(Resource, Default)]
struct BackgroundPhases(HashMap<Entity, SortedRenderPhase<Transparent3d>>);

#[derive(Component)]
struct BackgroundWriteback(CachedRenderPipelineId);

fn prepare_writeback(
    mut commands: Commands,
    views: Query<(Entity, &ViewTarget, &Msaa), With<PostProcessSettings>>,
    mut pipelines: ResMut<SpecializedRenderPipelines<BlitPipeline>>,
    blit: Res<BlitPipeline>,
    cache: Res<PipelineCache>,
) {
    for (entity, target, msaa) in &views {
        if msaa.samples() > 1 {
            let id = pipelines.specialize(
                &cache,
                &blit,
                BlitPipelineKey {
                    target_format: target.main_texture_format(),
                    samples: msaa.samples(),
                    blend_state: None,
                    source_space: None,
                },
            );
            commands.entity(entity).insert(BackgroundWriteback(id));
        } else {
            commands.entity(entity).remove::<BackgroundWriteback>();
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<BackgroundComposition>::default())
        .add_systems(Last, inherit_background_view);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<BackgroundPhases>()
            .add_systems(Render, prepare_writeback.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                (
                    isolate_background.before(draw_background),
                    draw_background.before(main_opaque_pass_3d),
                    restore_background.after(main_transparent_pass_3d),
                )
                    .in_set(Core3dSystems::MainPass),
            );
    }
}

// Video surfaces are children of their story picture. Inherit membership on
// renderable meshes, not from asset names or numeric z depth. The batch barrier
// prevents a prepared batch from spanning two composition boundaries.
fn inherit_background_view(
    mut commands: Commands,
    meshes: Query<(Entity, Has<BackgroundComposition>), With<Mesh3d>>,
    parents: Query<&ChildOf>,
    views: Query<&PictureView>,
) {
    for (entity, previous) in &meshes {
        let mut cursor = entity;
        let background = loop {
            if let Ok(view) = views.get(cursor) {
                break *view == PictureView::Background;
            }
            let Ok(parent) = parents.get(cursor) else {
                break false;
            };
            cursor = parent.parent();
        };
        if background && !previous {
            commands.entity(entity).try_insert(BackgroundComposition);
        }
        if !background && previous {
            commands
                .entity(entity)
                .try_remove::<BackgroundComposition>();
        }
    }
}

fn isolate_background(
    view: ViewQuery<(&ExtractedView, &PostProcessSettings)>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    members: Query<(), With<BackgroundComposition>>,
    mut isolated: ResMut<BackgroundPhases>,
) {
    let entity = view.entity();
    let (extracted, _) = view.into_inner();
    let Some(phase) = phases.get_mut(&extracted.retained_view_entity) else {
        return;
    };
    let mut background = SortedRenderPhase::default();
    let mut foreground = SortedRenderPhase::default();
    for (key, item) in phase.items.drain(..) {
        if members.contains(item.entity.0) {
            background.items.insert(key, item);
        } else {
            foreground.items.insert(key, item);
        }
    }
    phase.items = foreground.items;
    isolated.0.insert(entity, background);
}

fn draw_background(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &PostProcessSettings,
        Option<&LayerEffectShaders>,
        Option<&BackgroundWriteback>,
    )>,
    isolated: Res<BackgroundPhases>,
    pipeline: Option<Res<EffectPipeline>>,
    cache: Res<PipelineCache>,
    blit: Res<BlitPipeline>,
    mut uniform: Local<Option<PreparedEffectUniform>>,
    blur: Res<BlurPipeline>,
    mut blur_work: Local<BlurWorkspace>,
    mut ctx: RenderContext,
) {
    let entity = view.entity();
    let (camera, target, depth, settings, programs, writeback) = view.into_inner();
    let Some(phase) = isolated.0.get(&entity) else {
        return;
    };
    if phase.items.is_empty() {
        *blur_work = BlurWorkspace::default();
        return;
    }
    {
        // This is the first main color attachment use: Bevy clears once here,
        // then its opaque/transparent scene passes load the composed background.
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("hiraku_background"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(viewport) = camera.viewport.as_ref() {
            pass.set_camera_viewport(viewport);
        }
        if let Err(error) = phase.render(&mut pass, world, entity) {
            error!("background composition failed: {error:?}");
        }
    }
    if settings.background.is_enabled() || programs.is_some_and(|p| p.background.is_some()) {
        // Never flip the resolved target until we can seed the MSAA attachment:
        // the following scene pass would otherwise resolve its stale contents.
        let msaa_pipeline = if target.sampled_main_texture_view().is_some() {
            let Some(compiled) = writeback.and_then(|id| cache.get_render_pipeline(id.0)) else {
                return;
            };
            Some(compiled)
        } else {
            None
        };
        apply_effect(
            target,
            &settings.background,
            3,
            programs,
            None,
            pipeline.as_deref(),
            &cache,
            &mut uniform,
            &blur,
            &mut blur_work,
            &mut ctx,
        );
        if let (Some(compiled), Some(sampled)) = (msaa_pipeline, target.sampled_main_texture_view())
        {
            let output = target.post_process_write();
            let bind_group = blit.create_bind_group(ctx.render_device(), output.source, &cache);
            let mut pass = ctx
                .command_encoder()
                .begin_render_pass(&RenderPassDescriptor {
                    label: Some("hiraku_background_msaa_writeback"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: sampled,
                        depth_slice: None,
                        resolve_target: Some(output.destination),
                        ops: Operations {
                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                            store: StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            pass.set_pipeline(compiled);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    } else {
        *blur_work = BlurWorkspace::default();
    }
}

fn restore_background(
    view: ViewQuery<(&ExtractedView, &PostProcessSettings)>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    mut isolated: ResMut<BackgroundPhases>,
) {
    let entity = view.entity();
    let (extracted, _) = view.into_inner();
    let Some(background) = isolated.0.remove(&entity) else {
        return;
    };
    if let Some(phase) = phases.get_mut(&extracted.retained_view_entity) {
        phase.items.extend(background.items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_membership_inherits_view_and_updates_without_replacing_mesh() {
        let mut app = App::new();
        app.add_systems(Update, inherit_background_view);
        let root = app.world_mut().spawn(PictureView::Background).id();
        let mesh = app
            .world_mut()
            .spawn((Mesh3d::default(), ChildOf(root)))
            .id();
        let scene_mesh = app
            .world_mut()
            .spawn((Mesh3d::default(), PictureView::Scene))
            .id();
        app.update();
        assert!(app.world().get::<BackgroundComposition>(mesh).is_some());
        assert!(app.world().get::<NoAutomaticBatching>(mesh).is_some());
        assert!(
            app.world()
                .get::<BackgroundComposition>(scene_mesh)
                .is_none()
        );
        app.world_mut().entity_mut(root).insert(PictureView::Scene);
        app.update();
        assert!(app.world().get::<BackgroundComposition>(mesh).is_none());
        assert!(app.world().get::<Mesh3d>(mesh).is_some());
    }
}
