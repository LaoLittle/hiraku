//! Keep engine texture bindings derived, while allowing an author-owned uniform layout.
use super::*;
use crate::effect::kawase::{BlurPipeline, MaterialBlurCache};
use crate::effect::program::{EFFECT_UNIFORM_BINDING, EffectUniform, uniform_layout_entry};
use bevy::{
    ecs::system::SystemParamItem,
    render::{
        render_asset::RenderAssets,
        render_resource::*,
        renderer::{RenderDevice, RenderQueue},
        texture::GpuImage,
    },
};

#[derive(AsBindGroup)]
pub struct SpriteBindings {
    #[uniform(0)]
    geometry: WorldSpriteUniform,
    #[texture(1)]
    #[sampler(2)]
    image: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    dissolve: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    clip: Option<Handle<Image>>,
    #[uniform(15)]
    sampling: UVec4,
}

impl AsBindGroup for WorldSpriteMaterial {
    type Data = WorldSpriteKey;
    type Param = (
        <SpriteBindings as AsBindGroup>::Param,
        Res<'static, RenderAssets<GpuImage>>,
        Res<'static, PipelineCache>,
        Option<Res<'static, BlurPipeline>>,
        Res<'static, RenderQueue>,
        ResMut<'static, MaterialBlurCache>,
    );
    fn label() -> &'static str {
        "hiraku_world_sprite"
    }
    fn bind_group_data(&self) -> Self::Data {
        self.into()
    }
    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = SpriteBindings::bind_group_layout_entries(device, force_no_bindless);
        entries.push(uniform_layout_entry());
        entries
    }
    fn build_bind_group(
        &self,
        layout: &BindGroupLayout,
        device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
        output: &mut BindGroupBuilder,
    ) -> Result<(), AsBindGroupError> {
        let (base, images, pipelines, blur, queue, blurred_images) = param;
        let mut geometry: WorldSpriteUniform = self.into();
        let mut sampling = self.sampling;
        let filtered = if self.effect_shader.is_none()
            && self.post_process.blur_radius > 0.001
            && self.image.is_some()
        {
            let image = images
                .get(self.image.as_ref().expect("image checked"))
                .ok_or(AsBindGroupError::RetryNextUpdate)?;
            let size = UVec2::new(
                image.texture_descriptor.size.width,
                image.texture_descriptor.size.height,
            );
            let rect = if self.rect.z > 0.0 && self.rect.w > 0.0 {
                self.rect
            } else {
                Vec4::new(0.0, 0.0, size.x as f32, size.y as f32)
            };
            let view = blurred_images
                .get(
                    blur.as_ref().ok_or(AsBindGroupError::RetryNextUpdate)?,
                    device,
                    queue,
                    pipelines,
                    self.image.as_ref().expect("image checked").id(),
                    &image.texture_view,
                    size,
                    rect,
                    sampling.x,
                    self.post_process.blur_radius,
                )
                .ok_or(AsBindGroupError::RetryNextUpdate)?;
            geometry.rect = Vec4::ZERO;
            geometry.effects.x = 1.0; // Filtered input is already linear premultiplied RGBA.
            sampling = UVec4::ZERO;
            Some(view)
        } else {
            None
        };
        SpriteBindings {
            geometry,
            image: self.image.clone(),
            dissolve: self.dissolve_mask.clone(),
            clip: self.clip_mask.clone(),
            sampling,
        }
        .build_bind_group(layout, device, base, force_no_bindless, output)?;
        if let Some(view) = filtered {
            let (_, binding) = output
                .iter_mut()
                .find(|(binding, _)| *binding == 1)
                .expect("derived image binding");
            *binding = UnpreparedBindingResource::TextureView(TextureViewDimension::D2, view);
        }
        let uniform = self
            .effect_shader
            .as_ref()
            .map(|effect| effect.uniform.clone())
            .unwrap_or_else(|| {
                EffectUniform::new(&self.post_process).expect("standard effect uniform layout")
            });
        let buffer = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("hiraku_material_effect_uniform"),
            contents: uniform.bytes(),
            usage: BufferUsages::UNIFORM,
        });
        output.push((
            EFFECT_UNIFORM_BINDING,
            UnpreparedBindingResource::Buffer(buffer),
        ));
        Ok(())
    }
}
