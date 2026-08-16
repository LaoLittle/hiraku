use bevy::{
    core_pipeline::{Core2dSystems, FullscreenShader, schedule::Core2d},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_resource::{
            binding_types::{sampler, texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        view::ViewTarget,
    },
};

pub struct BlurEffectPlugin;

impl Plugin for BlurEffectPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "shaders/blur_effect.wgsl");
        app.add_plugins((
            ExtractComponentPlugin::<BlurSettings>::default(),
            UniformComponentPlugin::<BlurSettings>::default(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.add_systems(RenderStartup, init_blur_pipeline);
        render_app.add_systems(Core2d, blur_pass.in_set(Core2dSystems::PostProcess));
    }
}

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub struct BlurSettings {
    radius: Vec4,
}

impl BlurSettings {
    pub fn new(radius: f32) -> Self {
        let mut settings = Self::default();
        settings.set_radius(radius);
        settings
    }

    pub fn set_radius(&mut self, radius: f32) {
        self.radius.x = radius.max(0.0);
    }
}

impl Default for BlurSettings {
    fn default() -> Self {
        Self { radius: Vec4::ZERO }
    }
}

#[derive(Default)]
struct BlurBindGroupCache {
    cached: Option<(TextureViewId, BindGroup)>,
}

fn blur_pass(
    view: ViewQuery<(
        &ViewTarget,
        &BlurSettings,
        &DynamicUniformIndex<BlurSettings>,
    )>,
    pipeline: Option<Res<BlurPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    settings_uniforms: Res<ComponentUniforms<BlurSettings>>,
    mut bind_group_cache: Local<BlurBindGroupCache>,
    mut render_context: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(pipeline.pipeline_id) else {
        return;
    };
    let Some(settings_binding) = settings_uniforms.uniforms().binding() else {
        return;
    };

    let (view_target, _, settings_index) = view.into_inner();
    let post_process = view_target.post_process_write();
    let bind_group = match &mut bind_group_cache.cached {
        Some((texture_id, bind_group)) if *texture_id == post_process.source.id() => bind_group,
        cached => {
            let bind_group = render_context.render_device().create_bind_group(
                "blur_effect_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((
                    post_process.source,
                    &pipeline.sampler,
                    settings_binding.clone(),
                )),
            );
            let (_, bind_group) = cached.insert((post_process.source.id(), bind_group));
            bind_group
        }
    };

    let mut render_pass =
        render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("blur_effect_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: post_process.destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

    render_pass.set_pipeline(render_pipeline);
    render_pass.set_bind_group(0, bind_group, &[settings_index.index()]);
    render_pass.draw(0..3, 0..1);
}

#[derive(Resource)]
struct BlurPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    pipeline_id: CachedRenderPipelineId,
}

fn init_blur_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
    fullscreen_shader: Res<FullscreenShader>,
    pipeline_cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "blur_effect_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<BlurSettings>(true),
            ),
        ),
    );

    let sampler = render_device.create_sampler(&SamplerDescriptor::default());
    let pipeline_id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("blur_effect_pipeline".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen_shader.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: bevy::asset::load_embedded_asset!(&*asset_server, "shaders/blur_effect.wgsl"),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });

    commands.insert_resource(BlurPipeline {
        layout,
        sampler,
        pipeline_id,
    });
}
