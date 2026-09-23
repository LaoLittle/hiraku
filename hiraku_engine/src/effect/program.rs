//! Author fragment shaders shared by material and fullscreen pipelines.
//! Pipeline shader definitions specialize the library, not the author's source.
use bevy::render::render_resource::{ShaderType, encase};
use bevy::{
    prelude::*,
    render::{RenderApp, extract_component::ExtractComponent},
    shader::Shader,
};
use std::sync::Arc;

/// The only author uniform slot. Other bindings in the target group are owned
/// by the rendering library. Uniform field layout is entirely shader-owned.
pub const EFFECT_UNIFORM_BINDING: u32 = 7;

pub(crate) fn uniform_layout_entry() -> bevy::render::render_resource::BindGroupLayoutEntry {
    use bevy::render::render_resource::*;
    BindGroupLayoutEntry {
        binding: EFFECT_UNIFORM_BINDING,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectUniform(Arc<[u8]>);

impl EffectUniform {
    pub fn new<T: ShaderType + encase::internal::WriteInto>(
        value: &T,
    ) -> Result<Self, encase::internal::Error> {
        let mut buffer = encase::UniformBuffer::new(Vec::new());
        buffer.write(value)?;
        let mut bytes = buffer.into_inner();
        bytes.resize(bytes.len().max(16).next_multiple_of(16), 0);
        Ok(Self(bytes.into()))
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Retain public shader modules with their author-facing import paths. Target
/// libraries use ordinary embedded assets; these aliases never change per mount.
#[derive(Resource)]
struct ShaderLibraries {
    _handles: Vec<Handle<Shader>>,
}

pub(super) fn register_libraries(app: &mut App) {
    let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
    let handles = [
        ("render", include_str!("shaders/render.wesl")),
        ("effects/kawase", include_str!("shaders/kawase.wesl")),
        (
            "effects/color_grade",
            include_str!("shaders/color_grade.wesl"),
        ),
    ]
    .into_iter()
    .map(|(name, source)| {
        shaders.add(Shader::from_wesl(
            source,
            format!("embedded://hiraku/{name}.wesl"),
        ))
    })
    .collect();
    app.insert_resource(ShaderLibraries { _handles: handles });
}

#[derive(Clone, Debug)]
pub struct EffectShader {
    pub(crate) shader: Handle<Shader>,
}

impl EffectShader {
    /// The unchanged author source; update it through Assets<Shader> for live editing.
    pub fn source(&self) -> &Handle<Shader> {
        &self.shader
    }
    /// Register once, then clone the returned handles onto any number of targets.
    /// `source` declares `@fragment fn fragment(VertexOutput) -> FragmentOutput`
    /// using `hiraku::render`. Use `finish` to preserve clipping/compositing.
    /// Additional libraries must be loaded
    /// by the caller, just like ordinary Bevy shader libraries.
    pub fn from_wesl(shaders: &mut Assets<Shader>, source: impl Into<String>) -> Self {
        // Unique module identity prevents two independently registered effects
        // from replacing each other's imports. IDs are not persisted in saves.
        let name = format!("effect_{}", uuid::Uuid::new_v4().simple());
        let path = format!("embedded://hiraku_effects/{name}.wesl");
        Self::new(shaders.add(Shader::from_wesl(source.into(), path)))
    }

    /// Mount a shader loaded through AssetServer (including hot-reloaded files).
    pub fn new(shader: Handle<Shader>) -> Self {
        Self { shader }
    }

    pub fn instance<T: ShaderType + encase::internal::WriteInto>(
        &self,
        uniform: &T,
    ) -> Result<EffectInstance, encase::internal::Error> {
        Ok(EffectInstance {
            shader: self.clone(),
            uniform: EffectUniform::new(uniform)?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct EffectInstance {
    pub shader: EffectShader,
    /// Encoded using WGSL uniform layout, without a fixed parameter schema.
    pub uniform: EffectUniform,
}

/// Attach to the engine camera alongside PostProcessSettings. Each layer can
/// override the standard program independently. Removing an override restores
/// that layer's saved standard configuration. Programs are host-owned assets.
#[derive(Component, Clone, Debug, Default, ExtractComponent)]
#[extract_app(RenderApp)]
#[require(super::post_process::PostProcessSettings)]
pub struct LayerEffectShaders {
    pub scene: Option<EffectInstance>,
    pub ui: Option<EffectInstance>,
    pub canvas: Option<EffectInstance>,
}

impl LayerEffectShaders {
    pub(crate) fn get(&self, stage: u32) -> Option<&EffectInstance> {
        match stage {
            0 => self.scene.as_ref(),
            1 => self.ui.as_ref(),
            2 => self.canvas.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(ShaderType)]
    struct Controls {
        strength: f32,
        tint: Vec4,
        thresholds: [Vec4; 4],
    }

    #[test]
    fn author_uniforms_have_wgsl_padding_and_no_three_vector_limit() {
        let value = EffectUniform::new(&Controls {
            strength: 0.5,
            tint: Vec4::splat(2.0),
            thresholds: [Vec4::ONE; 4],
        })
        .expect("valid uniform");
        assert_eq!(value.bytes().len(), 96);
        assert_eq!(&value.bytes()[0..4], &0.5_f32.to_le_bytes());
        assert_eq!(&value.bytes()[4..16], &[0; 12]);
        assert_eq!(&value.bytes()[16..20], &2.0_f32.to_le_bytes());
        assert_eq!(
            EffectUniform::new(&3.0_f32)
                .expect("scalar uniform")
                .bytes()
                .len(),
            16
        );
    }

    #[test]
    fn custom_uniforms_do_not_overwrite_standard_layer_configuration() {
        let mut shaders = Assets::<Shader>::default();
        let shader = EffectShader::from_wesl(&mut shaders, "// fixture");
        let mut world = World::new();
        let entity = world
            .spawn(LayerEffectShaders {
                ui: Some(shader.instance(&Vec4::splat(3.0)).expect("uniform")),
                ..default()
            })
            .id();
        assert_eq!(
            world
                .get::<super::super::post_process::PostProcessSettings>(entity)
                .expect("required settings"),
            &super::super::post_process::PostProcessSettings::default()
        );
    }
}
