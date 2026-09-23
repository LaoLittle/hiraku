//! GPU scheduling for blur.wesl. Sampling algorithms live entirely in that shader.
use super::program::EffectUniform;
use bevy::{
    core_pipeline::FullscreenShader,
    prelude::*,
    render::{
        Extract, ExtractSchedule, RenderApp, RenderStartup,
        render_resource::{
            binding_types::{sampler, texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderDevice, RenderQueue},
    },
};
use std::collections::HashMap;

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        invalidate_blur_sprites.before(crate::render::world_sprite::sync_world_sprites),
    );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<MaterialBlurCache>()
            .add_systems(ExtractSchedule, invalidate_cached_images)
            .add_systems(RenderStartup, initialize);
    }
}

fn invalidate_blur_sprites(
    mut events: MessageReader<AssetEvent<Image>>,
    mut sprites: Query<&mut crate::render::world_sprite::WorldSprite>,
) {
    let changed: std::collections::HashSet<_> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    for mut sprite in &mut sprites {
        if sprite.post_process.blur_radius > 0.001
            && sprite
                .image
                .as_ref()
                .is_some_and(|image| changed.contains(&image.id()))
        {
            sprite.set_changed();
        }
    }
}

fn invalidate_cached_images(
    mut events: Extract<MessageReader<AssetEvent<Image>>>,
    mut cache: ResMut<MaterialBlurCache>,
) {
    for event in events.read() {
        if let AssetEvent::Modified { id } | AssetEvent::Removed { id } = event {
            cache.images.retain(|key, _| key.image != *id);
        }
    }
    cache.bytes = cache.images.values().map(|image| image.bytes).sum();
}

const FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const CACHE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Resource)]
pub struct BlurPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    passes: [CachedRenderPipelineId; 3],
}

#[derive(Clone, Copy, ShaderType)]
struct BlurUniform {
    source_rect: Vec4,
    source_size: Vec2,
    destination_size: Vec2,
    sampling: u32,
    premultiplied: u32,
    blend: f32,
    padding: f32,
}

fn initialize(
    mut commands: Commands,
    device: Res<RenderDevice>,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "hiraku_kawase",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (0, texture_2d(TextureSampleType::Float { filterable: true })),
                (1, sampler(SamplerBindingType::Filtering)),
                (2, texture_2d(TextureSampleType::Float { filterable: true })),
                (7, uniform_buffer::<BlurUniform>(false)),
            ),
        ),
    );
    let shader = bevy::asset::load_embedded_asset!(&*server, "shaders/blur.wesl");
    let passes = ["prepare", "downsample", "upsample"].map(|entry| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(format!("hiraku_kawase_{entry}").into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                shader_defs: vec![bevy::shader::ShaderDefVal::UInt(
                    "EFFECT_BINDING_GROUP".into(),
                    0,
                )],
                targets: vec![Some(ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    });
    commands.insert_resource(BlurPipeline {
        layout,
        passes,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
    });
}

#[derive(Debug, PartialEq)]
struct Plan {
    sizes: Vec<UVec2>,
    blend: f32,
}

fn plan(size: UVec2, radius: f32) -> Plan {
    let radius = if radius.is_finite() {
        radius.clamp(0.0, 128.0)
    } else {
        0.0
    };
    let levels = ((radius + 1.0).log2().ceil() as usize).min(8);
    let mut sizes = vec![size.max(UVec2::ONE)];
    for _ in 0..levels {
        let previous = *sizes.last().expect("base level");
        if previous == UVec2::ONE {
            break;
        }
        sizes.push(UVec2::new(previous.x.div_ceil(2), previous.y.div_ceil(2)));
    }
    let depth = sizes.len() - 1;
    let width = 2_f32.powi(depth.saturating_sub(1) as i32);
    Plan {
        sizes,
        blend: ((radius - (width - 1.0)) / width).clamp(0.0, 1.0),
    }
}

struct Target {
    _texture: Texture,
    view: TextureView,
}
#[derive(Default)]
pub(crate) struct BlurWorkspace {
    sizes: Vec<UVec2>,
    down: Vec<Target>,
    up: Vec<Target>,
}

fn target(device: &RenderDevice, size: UVec2) -> Target {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("hiraku_kawase_level"),
        size: Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    Target {
        _texture: texture,
        view,
    }
}

impl BlurPipeline {
    pub(crate) fn ready(&self, cache: &PipelineCache) -> bool {
        self.passes
            .iter()
            .all(|id| cache.get_render_pipeline(*id).is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &self,
        device: &RenderDevice,
        cache: &PipelineCache,
        encoder: &mut CommandEncoder,
        image: &TextureView,
        image_size: UVec2,
        rect: Vec4,
        sampling: u32,
        premultiplied: bool,
        radius: f32,
        work: &mut BlurWorkspace,
    ) -> Option<TextureView> {
        if !self.ready(cache) {
            return None;
        }
        let plan = plan(
            UVec2::new(
                rect.z.round().max(1.0) as u32,
                rect.w.round().max(1.0) as u32,
            ),
            radius,
        );
        if work.sizes != plan.sizes {
            work.down = plan.sizes.iter().map(|&s| target(device, s)).collect();
            work.up = plan.sizes[..plan.sizes.len() - 1]
                .iter()
                .map(|&s| target(device, s))
                .collect();
            work.sizes = plan.sizes.clone();
        }
        let mut uniform = BlurUniform {
            source_rect: rect,
            source_size: image_size.as_vec2(),
            destination_size: plan.sizes[0].as_vec2(),
            sampling,
            premultiplied: u32::from(premultiplied),
            blend: 1.0,
            padding: 0.0,
        };
        self.draw(
            device,
            cache,
            encoder,
            0,
            image,
            image,
            &work.down[0].view,
            uniform,
        );
        for level in 1..plan.sizes.len() {
            uniform.destination_size = plan.sizes[level].as_vec2();
            self.draw(
                device,
                cache,
                encoder,
                1,
                &work.down[level - 1].view,
                &work.down[level - 1].view,
                &work.down[level].view,
                uniform,
            );
        }
        let last = plan.sizes.len() - 1;
        let mut previous = &work.down[last].view;
        for level in (0..last).rev() {
            uniform.destination_size = plan.sizes[level].as_vec2();
            uniform.blend = if level == last - 1 { plan.blend } else { 1.0 };
            self.draw(
                device,
                cache,
                encoder,
                2,
                previous,
                &work.down[level].view,
                &work.up[level].view,
                uniform,
            );
            previous = &work.up[level].view;
        }
        Some(previous.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        device: &RenderDevice,
        cache: &PipelineCache,
        encoder: &mut CommandEncoder,
        mode: usize,
        source: &TextureView,
        baseline: &TextureView,
        destination: &TextureView,
        settings: BlurUniform,
    ) {
        let data = EffectUniform::new(&settings).expect("blur uniform layout");
        let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("hiraku_kawase_uniform"),
            contents: data.bytes(),
            usage: BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(
            "hiraku_kawase",
            &cache.get_bind_group_layout(&self.layout),
            &BindGroupEntries::with_indices((
                (0, source),
                (1, &self.sampler),
                (2, baseline),
                (7, uniform.as_entire_binding()),
            )),
        );
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("hiraku_kawase"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: destination,
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
        pass.set_pipeline(
            cache
                .get_render_pipeline(self.passes[mode])
                .expect("pipelines checked before rendering"),
        );
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[derive(Hash, PartialEq, Eq)]
struct MaterialKey {
    image: bevy::asset::AssetId<Image>,
    source: TextureViewId,
    rect: [u32; 4],
    radius: u32,
    sampling: u32,
}
struct CachedMaterial {
    view: TextureView,
    bytes: u64,
    used: u64,
}
/// Bound by memory, not time. Fades/tints reuse the filtered image, and source
/// texture replacement changes the key. Intermediate pyramid levels are transient.
#[derive(Resource, Default)]
pub struct MaterialBlurCache {
    images: HashMap<MaterialKey, CachedMaterial>,
    bytes: u64,
    access: u64,
}

impl MaterialBlurCache {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get(
        &mut self,
        pipeline: &BlurPipeline,
        device: &RenderDevice,
        queue: &RenderQueue,
        cache: &PipelineCache,
        image_id: bevy::asset::AssetId<Image>,
        image: &TextureView,
        size: UVec2,
        rect: Vec4,
        sampling: u32,
        radius: f32,
    ) -> Option<TextureView> {
        let key = MaterialKey {
            image: image_id,
            source: image.id(),
            rect: rect.to_array().map(f32::to_bits),
            radius: radius.to_bits(),
            sampling,
        };
        self.access += 1;
        if let Some(cached) = self.images.get_mut(&key) {
            cached.used = self.access;
            return Some(cached.view.clone());
        }
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("hiraku_material_blur"),
        });
        let view = pipeline.render(
            device,
            cache,
            &mut encoder,
            image,
            size,
            rect,
            sampling,
            false,
            radius,
            &mut BlurWorkspace::default(),
        )?;
        queue.submit([encoder.finish()]);
        let bytes = rect.z.round().max(1.0) as u64 * rect.w.round().max(1.0) as u64 * 8;
        if bytes <= CACHE_BYTES {
            while self.bytes + bytes > CACHE_BYTES {
                let oldest = self
                    .images
                    .iter()
                    .min_by_key(|(_, v)| v.used)
                    .map(|(k, _)| MaterialKey {
                        image: k.image,
                        source: k.source,
                        rect: k.rect,
                        radius: k.radius,
                        sampling: k.sampling,
                    });
                let Some(oldest) = oldest else {
                    break;
                };
                self.bytes -= self
                    .images
                    .remove(&oldest)
                    .expect("existing cache entry")
                    .bytes;
            }
            self.bytes += bytes;
            self.images.insert(
                key,
                CachedMaterial {
                    view: view.clone(),
                    bytes,
                    used: self.access,
                },
            );
        }
        Some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPU reference of blur.wesl's bilinear kernels, using generated pixels.
    // This catches the disconnected four-copy response of the former filter.
    fn resample(pixels: &[f32], size: UVec2, output: UVec2, up: bool) -> Vec<f32> {
        let sample = |uv: Vec2| {
            let p = (uv * size.as_vec2() - Vec2::splat(0.5))
                .clamp(Vec2::ZERO, (size - UVec2::ONE).as_vec2());
            let x = p.x.floor() as u32;
            let y = p.y.floor() as u32;
            let value =
                |x: u32, y: u32| pixels[(y.min(size.y - 1) * size.x + x.min(size.x - 1)) as usize];
            let a = value(x, y) * (1.0 - p.x.fract()) + value(x + 1, y) * p.x.fract();
            let b = value(x, y + 1) * (1.0 - p.x.fract()) + value(x + 1, y + 1) * p.x.fract();
            a * (1.0 - p.y.fract()) + b * p.y.fract()
        };
        let mut result = Vec::new();
        for y in 0..output.y {
            for x in 0..output.x {
                let uv = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / output.as_vec2();
                let d = Vec2::splat(0.5) / size.as_vec2();
                let corners = sample(uv + d)
                    + sample(uv - d)
                    + sample(uv + Vec2::new(d.x, -d.y))
                    + sample(uv + Vec2::new(-d.x, d.y));
                result.push(if up {
                    (2.0 * corners
                        + sample(uv + Vec2::new(2.0 * d.x, 0.0))
                        + sample(uv - Vec2::new(2.0 * d.x, 0.0))
                        + sample(uv + Vec2::new(0.0, 2.0 * d.y))
                        + sample(uv - Vec2::new(0.0, 2.0 * d.y)))
                        / 12.0
                } else {
                    (4.0 * sample(uv) + corners) / 8.0
                });
            }
        }
        result
    }

    fn reference(input: Vec<f32>, size: UVec2, radius: f32) -> Vec<f32> {
        let plan = plan(size, radius);
        let mut levels = vec![input];
        for i in 1..plan.sizes.len() {
            levels.push(resample(
                &levels[i - 1],
                plan.sizes[i - 1],
                plan.sizes[i],
                false,
            ));
        }
        let last = levels.len() - 1;
        let mut current = levels[last].clone();
        for i in (0..last).rev() {
            current = resample(&current, plan.sizes[i + 1], plan.sizes[i], true);
            if i == last - 1 {
                for (pixel, original) in current.iter_mut().zip(&levels[i]) {
                    *pixel = *original * (1.0 - plan.blend) + *pixel * plan.blend;
                }
            }
        }
        current
    }

    #[test]
    fn impulse_is_centered_smooth_and_flat_colors_are_preserved() {
        let size = UVec2::splat(65);
        let mut input = vec![0.0; 65 * 65];
        input[32 * 65 + 32] = 1.0;
        let output = reference(input, size, 8.0);
        let center = output[32 * 65 + 32];
        assert!(center > 0.0 && center < 0.1);
        for offset in 0..32 {
            let a = output[32 * 65 + 32 + offset];
            let b = output[32 * 65 + 32 - offset];
            assert!((a - b).abs() < 1e-6, "symmetric response");
            assert!(
                a + 1e-6 >= output[32 * 65 + 33 + offset],
                "no separated ghost peaks"
            );
        }
        assert!(
            reference(vec![0.375; 65 * 65], size, 8.0)
                .iter()
                .all(|v| (v - 0.375).abs() < 1e-6)
        );
        let below = reference(vec![0.375; 65 * 65], size, 0.0);
        assert_eq!(below, vec![0.375; 65 * 65]);
    }

    #[test]
    fn pyramid_handles_odd_and_tiny_images() {
        assert_eq!(
            plan(UVec2::new(7, 3), 128.0).sizes,
            vec![
                UVec2::new(7, 3),
                UVec2::new(4, 2),
                UVec2::new(2, 1),
                UVec2::ONE
            ]
        );
        assert_eq!(plan(UVec2::ONE, 128.0).sizes, vec![UVec2::ONE]);
        assert_eq!(plan(UVec2::new(1920, 1080), 0.0).sizes.len(), 1);
    }
    #[test]
    fn new_levels_start_at_zero_weight() {
        for radius in [1.0, 3.0, 7.0, 15.0, 31.0, 63.0] {
            let previous = plan(UVec2::splat(2048), radius);
            let next = plan(UVec2::splat(2048), radius + 0.001);
            assert_eq!(previous.blend, 1.0);
            assert_eq!(next.sizes.len(), previous.sizes.len() + 1);
            assert!(next.blend < 0.002);
        }
    }
}
