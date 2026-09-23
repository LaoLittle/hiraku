//! Keep engine texture bindings derived, while allowing an author-owned uniform layout.
use super::*;
use crate::effect::program::{EFFECT_UNIFORM_BINDING, EffectUniform, uniform_layout_entry};
use bevy::{
    ecs::system::SystemParamItem,
    render::{render_resource::*, renderer::RenderDevice},
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
    type Param = <SpriteBindings as AsBindGroup>::Param;
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
        SpriteBindings {
            geometry: self.into(),
            image: self.image.clone(),
            dissolve: self.dissolve_mask.clone(),
            clip: self.clip_mask.clone(),
            sampling: self.sampling,
        }
        .build_bind_group(layout, device, param, force_no_bindless, output)?;
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
