//! One background composition boundary within the existing Camera3d schedule.
use super::{
    kawase::{BlurPipeline, BlurWorkspace},
    post_process::{
        EffectInput, EffectPipeline, PostProcessSettings, PreparedEffectUniform, apply_effect,
    },
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
        renderer::{RenderContext, RenderDevice, ViewQuery},
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
struct BackgroundComposite(CachedRenderPipelineId);

fn prepare_composite(
    mut commands: Commands,
    views: Query<(Entity, &ViewTarget, &Msaa), With<PostProcessSettings>>,
    mut pipelines: ResMut<SpecializedRenderPipelines<BlitPipeline>>,
    blit: Res<BlitPipeline>,
    cache: Res<PipelineCache>,
) {
    for (entity, target, msaa) in &views {
        let id = pipelines.specialize(
            &cache,
            &blit,
            BlitPipelineKey {
                target_format: target.main_texture_format(),
                samples: msaa.samples(),
                blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                source_space: None,
            },
        );
        commands.entity(entity).insert(BackgroundComposite(id));
    }
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<BackgroundComposition>::default())
        .add_systems(Last, inherit_background_view);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<BackgroundPhases>()
            .add_systems(Render, prepare_composite.in_set(RenderSystems::Prepare))
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

/// Reused layer-local input/output, allocated only while an effect needs it.
/// MSAA is resolved before sampling; no pass samples its own attachment.
struct BackgroundTargets {
    key: (Extent3d, TextureFormat, u32),
    input: TextureView,
    output: TextureView,
    multisampled: Option<TextureView>,
}

impl BackgroundTargets {
    fn new(device: &RenderDevice, key: (Extent3d, TextureFormat, u32)) -> Self {
        let create = |samples, label| {
            device
                .create_texture(&TextureDescriptor {
                    label: Some(label),
                    size: key.0,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: TextureDimension::D2,
                    format: key.1,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default())
        };
        Self {
            key,
            input: create(1, "hiraku_background_input"),
            output: create(1, "hiraku_background_effect"),
            multisampled: (key.2 > 1).then(|| create(key.2, "hiraku_background_msaa")),
        }
    }

    fn attachment(&self) -> RenderPassColorAttachment<'_> {
        RenderPassColorAttachment {
            view: self.multisampled.as_ref().unwrap_or(&self.input),
            resolve_target: self.multisampled.as_ref().map(|_| &*self.input),
            depth_slice: None,
            ops: Operations {
                load: LoadOp::Clear(LinearRgba::NONE.into()),
                store: StoreOp::Store,
            },
        }
    }
}

fn draw_background(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &PostProcessSettings,
        Option<&LayerEffectShaders>,
        Option<&BackgroundComposite>,
        &Msaa,
    )>,
    isolated: Res<BackgroundPhases>,
    pipeline: Option<Res<EffectPipeline>>,
    cache: Res<PipelineCache>,
    blit: Res<BlitPipeline>,
    mut uniform: Local<Option<PreparedEffectUniform>>,
    blur: Res<BlurPipeline>,
    mut blur_work: Local<BlurWorkspace>,
    mut textures: Local<Option<BackgroundTargets>>,
    mut ctx: RenderContext,
) {
    let entity = view.entity();
    let (camera, target, depth, settings, programs, composite, msaa) = view.into_inner();
    let Some(phase) = isolated.0.get(&entity) else {
        return;
    };
    if phase.items.is_empty() {
        *textures = None;
        *blur_work = BlurWorkspace::default();
        return;
    }
    let effect_active =
        settings.background.is_enabled() || programs.is_some_and(|p| p.background.is_some());
    // A compiling pipeline must not make the background vanish. Until the
    // compositing pipeline is ready, use the ordinary direct draw.
    let compiled = effect_active
        .then(|| composite.and_then(|id| cache.get_render_pipeline(id.0)))
        .flatten();
    if compiled.is_some() {
        let key = (
            target.main_texture().size(),
            target.main_texture_format(),
            msaa.samples(),
        );
        if textures.as_ref().is_none_or(|t| t.key != key) {
            *textures = Some(BackgroundTargets::new(ctx.render_device(), key));
        }
    } else {
        *textures = None;
        *blur_work = BlurWorkspace::default();
    }
    {
        let attachment = textures.as_ref().map_or_else(
            || target.get_color_attachment(),
            BackgroundTargets::attachment,
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("hiraku_background"),
            color_attachments: &[Some(attachment)],
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
    let (Some(textures), Some(compiled)) = (textures.as_ref(), compiled) else {
        return;
    };
    let processed = apply_effect(
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
        Some(EffectInput {
            source: &textures.input,
            destination: &textures.output,
            size: UVec2::new(textures.key.0.width, textures.key.0.height),
        }),
    );
    // Both paths contain only background RGBA, never the main clear color or
    // another camera's contribution. Premultiplied source-over happens once.
    let image = if processed {
        &textures.output
    } else {
        &textures.input
    };
    let group = blit.create_bind_group(ctx.render_device(), image, &cache);
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("hiraku_background_composite"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(compiled);
    pass.set_bind_group(0, &group, &[]);
    if let Some(viewport) = &camera.viewport {
        pass.set_scissor_rect(
            viewport.physical_position.x,
            viewport.physical_position.y,
            viewport.physical_size.x,
            viewport.physical_size.y,
        );
    }
    pass.draw(0..3, 0..1);
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
