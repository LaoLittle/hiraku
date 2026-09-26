//! Layer-local effects. UI isolation reuses Bevy's prepared UI phase and the
//! existing camera, preserving its viewport, picking target and draw commands.
use super::kawase::{BlurPipeline, BlurWorkspace};
use super::program::{EFFECT_UNIFORM_BINDING, EffectUniform, LayerEffectShaders};
use bevy::{asset::AssetId, shader::Shader};
use bevy::{
    core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d, upscaling::upscaling},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_phase::{SortedRenderPhase, ViewSortedRenderPhases},
        render_resource::{
            binding_types::{sampler, texture_2d},
            *,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        view::{ExtractedView, ViewTarget},
    },
    ui_render::{TransparentUi, UiCameraView},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Shared settings for material and fullscreen effects. Radius is in input
/// texture pixels; exposure is in stops. Colors are linear, not display sRGB.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType, Serialize, Deserialize)]
pub struct EffectParameters {
    pub blur_radius: f32,
    pub zoom: f32,
    pub exposure: f32,
    pub saturation: f32,
    pub tint: Vec4,
    /// Source-image byte-domain grayscale followed by per-channel gamma.
    /// A zero W disables this operation; RGB contain the gamma exponents.
    pub grayscale_gamma: Vec4,
    /// Strength, inner radius, outer radius, unused.
    pub vignette: Vec4,
    /// UV offset, view roll in radians, canvas aspect ratio (zero = identity).
    #[serde(default)]
    pub view_transform: Vec4,
}

impl Default for EffectParameters {
    fn default() -> Self {
        Self {
            blur_radius: 0.0,
            zoom: 1.0,
            exposure: 0.0,
            saturation: 1.0,
            tint: Vec4::ONE,
            grayscale_gamma: Vec4::ZERO,
            vignette: Vec4::new(0.0, 0.25, 0.75, 0.0),
            view_transform: Vec4::ZERO,
        }
    }
}

impl EffectParameters {
    pub fn is_enabled(&self) -> bool {
        *self != Self::default()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let scalar = [self.blur_radius, self.zoom, self.exposure, self.saturation];
        if scalar.into_iter().any(|v| !v.is_finite())
            || !self.tint.is_finite()
            || !self.grayscale_gamma.is_finite()
            || !self.vignette.is_finite()
            || !self.view_transform.is_finite()
            || !(0.0..=128.0).contains(&self.blur_radius)
            || self.zoom <= 0.0
            || !(-16.0..=16.0).contains(&self.exposure)
            || !(0.0..=8.0).contains(&self.saturation)
            || self.tint.min_element() < 0.0
            || self.tint.max_element() > 1.0
            || (self.grayscale_gamma.w != 0.0
                && (self.grayscale_gamma.w != 1.0
                    || self.grayscale_gamma.truncate().min_element() <= 0.0))
            || !(0.0..=1.0).contains(&self.vignette.x)
            || self.vignette.y < 0.0
            || self.vignette.z <= self.vignette.y
        {
            return Err("invalid post-process parameters");
        }
        Ok(())
    }
}

#[derive(
    Component,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    ExtractComponent,
    ShaderType,
    Serialize,
    Deserialize,
)]
#[extract_app(RenderApp)]
pub struct PostProcessSettings {
    #[serde(default)]
    pub background: EffectParameters,
    pub scene: EffectParameters,
    pub ui: EffectParameters,
    pub canvas: EffectParameters,
}

impl PostProcessSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.background.validate()?;
        self.scene.validate()?;
        self.ui.validate()?;
        self.canvas.validate()
    }
    pub fn layer_mut(&mut self, layer: crate::script::CameraEffectScope) -> &mut EffectParameters {
        match layer {
            crate::script::CameraEffectScope::Background => &mut self.background,
            crate::script::CameraEffectScope::World => &mut self.scene,
            crate::script::CameraEffectScope::Ui => &mut self.ui,
            crate::script::CameraEffectScope::Canvas => &mut self.canvas,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layer_effects_round_trip_without_merging_targets() {
        let mut scene = crate::state::SceneSnapshot::default();
        scene.post_process.scene.blur_radius = 12.0;
        scene.post_process.ui.saturation = 0.0;
        scene.post_process.canvas.exposure = -1.0;
        let encoded = crate::proto::SceneSnapshot::from(&scene);
        let decoded = crate::state::SceneSnapshot::try_from(encoded).expect("valid snapshot");
        assert_eq!(decoded.post_process, scene.post_process);
        assert!(decoded.post_process.validate().is_ok());
    }

    #[test]
    fn invalid_effects_are_rejected() {
        for radius in [-1.0, f32::NAN, f32::INFINITY, 129.0] {
            assert!(
                EffectParameters {
                    blur_radius: radius,
                    ..default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(!EffectParameters::default().is_enabled());
        assert!(
            EffectParameters {
                grayscale_gamma: Vec4::new(2.0, 1.1, 1.0, 1.0),
                ..default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            EffectParameters {
                grayscale_gamma: Vec4::new(2.0, 0.0, 1.0, 1.0),
                ..default()
            }
            .validate()
            .is_err()
        );
    }
}

pub struct PostProcessPlugin;

impl Plugin for PostProcessPlugin {
    fn build(&self, app: &mut App) {
        bevy::shader::load_shader_library!(app, "shaders/input.wesl");
        bevy::shader::load_shader_library!(app, "shaders/material.wesl");
        bevy::shader::load_shader_library!(app, "shaders/fullscreen.wesl");
        super::program::register_libraries(app);
        bevy::asset::embedded_asset!(app, "shaders/standard.wesl");
        bevy::asset::embedded_asset!(app, "shaders/blur.wesl");
        super::kawase::install(app);
        super::background::install(app);
        app.add_plugins((
            ExtractComponentPlugin::<PostProcessSettings>::default(),
            ExtractComponentPlugin::<LayerEffectShaders>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<IsolatedUiPhases>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                (prepare_custom_pipelines.in_set(RenderSystems::PrepareResources),),
            )
            .add_systems(
                Core3d,
                (
                    scene_pass
                        .after(Core3dSystems::PostProcess)
                        .before(isolate_ui),
                    isolate_ui.before(draw_isolated_ui),
                    draw_isolated_ui.before(bevy::ui_render::ui_pass),
                    restore_ui
                        .after(bevy::ui_render::ui_pass)
                        .before(canvas_pass),
                    canvas_pass.before(upscaling),
                ),
            );
    }
}

#[derive(Resource, Default)]
struct IsolatedUiPhases(HashMap<Entity, SortedRenderPhase<TransparentUi>>);

#[derive(Resource)]
pub(super) struct EffectPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    default_shader: Handle<Shader>,
    pipelines: HashMap<(TextureFormat, u32, AssetId<Shader>), CachedRenderPipelineId>,
}

pub(super) struct PreparedEffectUniform {
    data: EffectUniform,
    buffer: Buffer,
}

fn effect_layout_entries() -> Vec<BindGroupLayoutEntry> {
    let mut entries = BindGroupLayoutEntries::with_indices(
        ShaderStages::FRAGMENT,
        (
            (0, texture_2d(TextureSampleType::Float { filterable: true })),
            (1, sampler(SamplerBindingType::Filtering)),
            (3, texture_2d(TextureSampleType::Float { filterable: true })),
        ),
    )
    .to_vec();
    entries.push(super::program::uniform_layout_entry());
    entries
}

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new("hiraku_effects", &effect_layout_entries());
    let mut pipelines = HashMap::new();
    let default_shader = bevy::asset::load_embedded_asset!(&*server, "shaders/standard.wesl");
    for format in [
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Bgra8UnormSrgb,
        TextureFormat::Rgba8Unorm,
        TextureFormat::Bgra8Unorm,
        TextureFormat::Rgba16Float,
    ] {
        for stage in 0..4 {
            pipelines.insert(
                (format, stage, default_shader.id()),
                cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(format!("hiraku_effects_{format:?}_{stage}").into()),
                    layout: vec![layout.clone()],
                    vertex: fullscreen.to_vertex_state(),
                    fragment: Some(FragmentState {
                        shader: default_shader.clone(),
                        shader_defs: vec![
                            bevy::shader::ShaderDefVal::Bool("HIRAKU_MATERIAL".into(), false),
                            bevy::shader::ShaderDefVal::UInt("EFFECT_BINDING_GROUP".into(), 0),
                            bevy::shader::ShaderDefVal::UInt("EFFECT_STAGE".into(), stage),
                        ],
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        ..default()
                    }),
                    ..default()
                }),
            );
        }
    }
    commands.insert_resource(EffectPipeline {
        default_shader,
        layout,
        pipelines,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
    });
}

fn prepare_custom_pipelines(
    views: Query<(&ViewTarget, &LayerEffectShaders)>,
    mut pipeline: ResMut<EffectPipeline>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    for (target, programs) in &views {
        for stage in 0..4 {
            let Some(effect) = programs.get(stage) else {
                continue;
            };
            let shader = &effect.shader.shader;
            let format = target.main_texture_format();
            let key = (format, stage, shader.id());
            if pipeline.pipelines.contains_key(&key) {
                continue;
            }
            let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("hiraku_custom_effect".into()),
                layout: vec![pipeline.layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    shader_defs: vec![
                        bevy::shader::ShaderDefVal::Bool("HIRAKU_MATERIAL".into(), false),
                        bevy::shader::ShaderDefVal::UInt("EFFECT_BINDING_GROUP".into(), 0),
                        bevy::shader::ShaderDefVal::UInt("EFFECT_STAGE".into(), stage),
                    ],
                    targets: vec![Some(ColorTargetState {
                        format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            });
            pipeline.pipelines.insert(key, id);
        }
    }
}

impl EffectPipeline {
    fn key(
        &self,
        target: &ViewTarget,
        stage: u32,
        programs: Option<&LayerEffectShaders>,
    ) -> (TextureFormat, u32, AssetId<Shader>) {
        (
            target.main_texture_format(),
            stage,
            programs
                .and_then(|p| p.get(stage))
                .map_or(self.default_shader.id(), |e| e.shader.shader.id()),
        )
    }
}

fn scene_pass(
    view: ViewQuery<(
        &ViewTarget,
        &PostProcessSettings,
        Option<&LayerEffectShaders>,
    )>,
    pipeline: Option<Res<EffectPipeline>>,
    cache: Res<PipelineCache>,
    mut uniform: Local<Option<PreparedEffectUniform>>,
    blur: Res<BlurPipeline>,
    mut blur_work: Local<BlurWorkspace>,
    mut ctx: RenderContext,
) {
    let (target, settings, programs) = view.into_inner();
    if settings.scene.is_enabled() || programs.is_some_and(|p| p.scene.is_some()) {
        apply_effect(
            target,
            &settings.scene,
            0,
            programs,
            None,
            pipeline.as_deref(),
            &cache,
            &mut uniform,
            &blur,
            &mut blur_work,
            &mut ctx,
            None,
        );
    } else {
        *blur_work = BlurWorkspace::default();
    }
}

fn canvas_pass(
    view: ViewQuery<(
        &ViewTarget,
        &PostProcessSettings,
        Option<&LayerEffectShaders>,
    )>,
    pipeline: Option<Res<EffectPipeline>>,
    cache: Res<PipelineCache>,
    mut uniform: Local<Option<PreparedEffectUniform>>,
    blur: Res<BlurPipeline>,
    mut blur_work: Local<BlurWorkspace>,
    mut ctx: RenderContext,
) {
    let (target, settings, programs) = view.into_inner();
    if settings.canvas.is_enabled() || programs.is_some_and(|p| p.canvas.is_some()) {
        apply_effect(
            target,
            &settings.canvas,
            2,
            programs,
            None,
            pipeline.as_deref(),
            &cache,
            &mut uniform,
            &blur,
            &mut blur_work,
            &mut ctx,
            None,
        );
    } else {
        *blur_work = BlurWorkspace::default();
    }
}

/// Move, then restore the prepared phase around Bevy's default UI pass. This
/// does not reconstruct UI nodes or lose retained items between frames.
fn isolate_ui(
    view: ViewQuery<(
        &UiCameraView,
        &ViewTarget,
        &PostProcessSettings,
        Option<&LayerEffectShaders>,
    )>,
    ui_views: Query<&ExtractedView>,
    mut phases: ResMut<ViewSortedRenderPhases<TransparentUi>>,
    mut isolated: ResMut<IsolatedUiPhases>,
    pipeline: Option<Res<EffectPipeline>>,
    cache: Res<PipelineCache>,
    blur: Res<BlurPipeline>,
) {
    let (ui, target, settings, programs) = view.into_inner();
    if !settings.ui.is_enabled() && !programs.is_some_and(|p| p.ui.is_some()) {
        return;
    }
    let Some(pipeline) = pipeline else {
        return;
    };
    let Some(id) = pipeline.pipelines.get(&pipeline.key(target, 1, programs)) else {
        return;
    };
    if cache.get_render_pipeline(*id).is_none()
        || (programs.and_then(|p| p.ui.as_ref()).is_none()
            && settings.ui.blur_radius > 0.001
            && !blur.ready(&cache))
    {
        return;
    }
    let Ok(extracted) = ui_views.get(ui.0) else {
        return;
    };
    let Some(phase) = phases.get_mut(&extracted.retained_view_entity) else {
        return;
    };
    if phase.items.is_empty() {
        return;
    }
    isolated.0.insert(ui.0, std::mem::take(phase));
}

struct UiTexture {
    size: Extent3d,
    format: TextureFormat,
    _texture: Texture,
    view: TextureView,
}

fn draw_isolated_ui(
    world: &World,
    view: ViewQuery<(
        &UiCameraView,
        &ViewTarget,
        &PostProcessSettings,
        &ExtractedCamera,
        Option<&LayerEffectShaders>,
    )>,
    isolated: Res<IsolatedUiPhases>,
    pipeline: Res<EffectPipeline>,
    cache: Res<PipelineCache>,
    mut uniform: Local<Option<PreparedEffectUniform>>,
    blur: Res<BlurPipeline>,
    mut blur_work: Local<BlurWorkspace>,
    mut texture: Local<Option<UiTexture>>,
    mut ctx: RenderContext,
) {
    let (ui, target, settings, camera, programs) = view.into_inner();
    let Some(phase) = isolated.0.get(&ui.0) else {
        *texture = None;
        *blur_work = BlurWorkspace::default();
        return;
    };
    let size = target.main_texture().size();
    let format = target.main_texture_format();
    if texture
        .as_ref()
        .is_none_or(|t| t.size != size || t.format != format)
    {
        let image = ctx.render_device().create_texture(&TextureDescriptor {
            label: Some("hiraku_ui_effect_input"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = image.create_view(&TextureViewDescriptor::default());
        *texture = Some(UiTexture {
            size,
            format,
            _texture: image,
            view,
        });
    }
    let texture = texture.as_ref().expect("UI target allocated above");
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("hiraku_ui_isolated"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &texture.view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(LinearRgba::NONE.into()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(viewport) = camera.viewport.as_ref() {
            pass.set_camera_viewport(viewport);
        }
        if let Err(error) = phase.render(&mut pass, world, ui.0) {
            error!("failed to draw isolated UI: {error:?}");
        }
    }
    apply_effect(
        target,
        &settings.ui,
        1,
        programs,
        Some(&texture.view),
        Some(&pipeline),
        &cache,
        &mut uniform,
        &blur,
        &mut blur_work,
        &mut ctx,
        None,
    );
}

fn restore_ui(
    view: ViewQuery<&UiCameraView>,
    ui_views: Query<&ExtractedView>,
    mut phases: ResMut<ViewSortedRenderPhases<TransparentUi>>,
    mut isolated: ResMut<IsolatedUiPhases>,
) {
    let ui = view.into_inner();
    let Some(saved) = isolated.0.remove(&ui.0) else {
        return;
    };
    let Ok(extracted) = ui_views.get(ui.0) else {
        return;
    };
    if let Some(phase) = phases.get_mut(&extracted.retained_view_entity) {
        *phase = saved;
    }
}

/// Explicit composition targets. Neither target aliases the other; both use
/// the view's color format. This avoids flipping the main framebuffer when
/// processing a layer that has not yet been composed into it.
pub(super) struct EffectInput<'a> {
    pub source: &'a TextureView,
    pub destination: &'a TextureView,
    pub size: UVec2,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_effect(
    target: &ViewTarget,
    settings: &EffectParameters,
    stage: u32,
    programs: Option<&LayerEffectShaders>,
    ui: Option<&TextureView>,
    pipeline: Option<&EffectPipeline>,
    cache: &PipelineCache,
    uniform: &mut Option<PreparedEffectUniform>,
    blur: &BlurPipeline,
    blur_work: &mut BlurWorkspace,
    ctx: &mut RenderContext,
    input: Option<EffectInput<'_>>,
) -> bool {
    let Some(pipeline) = pipeline else {
        return false;
    };
    let Some(id) = pipeline
        .pipelines
        .get(&pipeline.key(target, stage, programs))
    else {
        return false;
    };
    let Some(compiled) = cache.get_render_pipeline(*id) else {
        return false;
    };
    let data = programs
        .and_then(|p| p.get(stage))
        .map(|e| e.uniform.clone())
        .unwrap_or_else(|| EffectUniform::new(settings).expect("standard effect uniform layout"));
    if uniform
        .as_ref()
        .is_none_or(|prepared| prepared.data != data)
    {
        *uniform = Some(PreparedEffectUniform {
            buffer: ctx
                .render_device()
                .create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("hiraku_effect_uniform"),
                    contents: data.bytes(),
                    usage: BufferUsages::UNIFORM,
                }),
            data,
        });
    }
    let buffer = &uniform.as_ref().expect("effect uniform prepared").buffer;
    let use_blur = programs.and_then(|p| p.get(stage)).is_none() && settings.blur_radius > 0.001;
    if use_blur && !blur.ready(cache) {
        return false;
    }
    let textures;
    let input = match input {
        Some(input) => input,
        None => {
            textures = target.post_process_write();
            let size = target.main_texture().size();
            EffectInput {
                source: textures.source,
                destination: textures.destination,
                size: UVec2::new(size.width, size.height),
            }
        }
    };
    let blurred = if use_blur {
        let device = ctx.render_device().clone();
        let size = input.size;
        Some(
            blur.render(
                &device,
                cache,
                ctx.command_encoder(),
                ui.unwrap_or(input.source),
                size,
                Vec4::new(0.0, 0.0, size.x as f32, size.y as f32),
                0,
                true,
                settings.blur_radius,
                blur_work,
            )
            .expect("blur pipelines ready"),
        )
    } else {
        *blur_work = BlurWorkspace::default();
        None
    };
    let scene_source = if stage == 1 {
        input.source
    } else {
        blurred.as_ref().unwrap_or(input.source)
    };
    let ui_source = blurred
        .as_ref()
        .filter(|_| stage == 1)
        .or(ui)
        .unwrap_or(input.source);
    let group = ctx.render_device().create_bind_group(
        "hiraku_effects",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::with_indices((
            (0, scene_source),
            (1, &pipeline.sampler),
            (3, ui_source),
            (EFFECT_UNIFORM_BINDING, buffer.as_entire_binding()),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("hiraku_post_process"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: input.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(compiled);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..3, 0..1);
    true
}
