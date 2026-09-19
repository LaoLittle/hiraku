//! Keep Bevy's native UI geometry, clipping, nine-slicing and batching. Only
//! specialize the fragment shader of completed image batches when necessary.
use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_phase::{PhaseItem, ViewSortedRenderPhases},
        render_resource::*,
    },
    shader::ShaderDefVal,
    ui_render::{UiBatch, prepare_uinodes, render_pass::TransparentUi},
};
use hiraku_sprite3d::sampling::ImageSampling;

pub fn install(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/color_ui.wesl");
    bevy::asset::embedded_asset!(app, "shaders/color_ui_slice.wesl");
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            specialize_batches
                .after(prepare_uinodes)
                .after(bevy::ui_render::ui_texture_slice_pipeline::prepare_ui_slices)
                .in_set(RenderSystems::PrepareBindGroups),
        );
    }
}

fn specialize_batches(
    sampling: Res<ImageSampling>,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    batches: Query<&UiBatch>,
    sliced: Query<&bevy::ui_render::ui_texture_slice_pipeline::UiTextureSlicerBatch>,
    mut phases: ResMut<ViewSortedRenderPhases<TransparentUi>>,
    mut variants: Local<
        std::collections::HashMap<(CachedRenderPipelineId, u32), CachedRenderPipelineId>,
    >,
) {
    for phase in phases.values_mut() {
        for (_, item) in &mut phase.items {
            let (image, slices) = if let Ok(batch) = batches.get(item.entity()) {
                (batch.image, false)
            } else if let Ok(batch) = sliced.get(item.entity()) {
                (batch.image, true)
            } else {
                continue;
            };
            let mode = sampling.0.get(&image).copied().unwrap_or(0);
            let original = variants
                .iter()
                .find_map(|((base, _), variant)| (*variant == item.pipeline).then_some(*base))
                .unwrap_or(item.pipeline);
            if mode == 0 {
                item.pipeline = original;
                continue;
            }
            let pipeline = *variants.entry((original, mode)).or_insert_with(|| {
                let mut descriptor = cache.get_render_pipeline_descriptor(original).clone();
                if let Some(fragment) = &mut descriptor.fragment {
                    fragment.shader = if slices {
                        bevy::asset::load_embedded_asset!(&*server, "shaders/color_ui_slice.wesl")
                    } else {
                        bevy::asset::load_embedded_asset!(&*server, "shaders/color_ui.wesl")
                    };
                    fragment
                        .shader_defs
                        .push(ShaderDefVal::UInt("COLOR_SAMPLING".into(), mode));
                }
                cache.queue_render_pipeline(descriptor)
            });
            item.pipeline = pipeline;
        }
    }
}
