use bevy::{
    asset::Handle,
    pbr::{Material, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
};

/// Authoring data for a flat image rendered by Hiraku's 3D world camera.
///
/// The component deliberately owns image-space concerns while `Transform`
/// remains available for story-level position, scale and animation.
#[derive(Component, Clone, Debug)]
pub struct WorldSprite {
    pub image: Option<Handle<Image>>,
    /// Source rectangle as `[left, top, width, height]` in pixels.
    pub rect: Option<[f32; 4]>,
    pub color: Color,
    pub custom_size: Option<Vec2>,
    resolved_size: Option<Vec2>,
}

impl WorldSprite {
    pub fn from_image(image: Handle<Image>) -> Self {
        Self {
            image: Some(image),
            rect: None,
            color: Color::WHITE,
            custom_size: None,
            resolved_size: None,
        }
    }

    pub fn from_color(color: Color, size: Vec2) -> Self {
        Self {
            image: None,
            rect: None,
            color,
            custom_size: Some(size),
            resolved_size: Some(size),
        }
    }

    pub fn with_rect(mut self, rect: Option<[f32; 4]>) -> Self {
        self.rect = rect;
        self.resolved_size = rect.map(|rect| Vec2::new(rect[2], rect[3]));
        self
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, WorldSpriteUniform)]
pub struct WorldSpriteMaterial {
    #[texture(1)]
    #[sampler(2)]
    pub image: Option<Handle<Image>>,
    pub tint: Vec4,
    pub rect: Vec4,
}

#[derive(Clone, Debug, ShaderType)]
pub struct WorldSpriteUniform {
    tint: Vec4,
    rect: Vec4,
}

impl From<&WorldSpriteMaterial> for WorldSpriteUniform {
    fn from(material: &WorldSpriteMaterial) -> Self {
        Self {
            tint: material.tint,
            rect: material.rect,
        }
    }
}

impl Material for WorldSpriteMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://hiraku_engine/render/shaders/world_sprite.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_shadows() -> bool {
        false
    }
}

pub fn world_sprite_render_components(
    sprite: &WorldSprite,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<WorldSpriteMaterial>,
) -> (Mesh3d, MeshMaterial3d<WorldSpriteMaterial>) {
    let size = sprite.resolved_size.unwrap_or(Vec2::ONE).max(Vec2::ONE);
    let mesh = meshes.add(Rectangle::new(size.x, size.y));
    let material = materials.add(material_from_sprite(sprite));
    (Mesh3d(mesh), MeshMaterial3d(material))
}

fn material_from_sprite(sprite: &WorldSprite) -> WorldSpriteMaterial {
    WorldSpriteMaterial {
        image: sprite.image.clone(),
        tint: sprite.color.to_linear().to_f32_array().into(),
        rect: sprite.rect.map(Vec4::from_array).unwrap_or(Vec4::ZERO),
    }
}

/// Resolves natural image sizes lazily and mirrors authoring changes into GPU
/// assets. No transform is rewritten, so animation state stays independent.
pub fn sync_world_sprites(
    mut commands: Commands,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut sprites: Query<(
        Entity,
        &mut WorldSprite,
        Option<&mut Mesh3d>,
        Option<&MeshMaterial3d<WorldSpriteMaterial>>,
    )>,
) {
    for (entity, mut sprite, mesh, material_handle) in &mut sprites {
        let size = sprite.custom_size.or_else(|| {
            sprite
                .rect
                .map(|rect| Vec2::new(rect[2], rect[3]))
                .or_else(|| {
                    sprite
                        .image
                        .as_ref()
                        .and_then(|handle| images.get(handle))
                        .map(|image| image.size_f32())
                })
        });

        if let Some(size) = size
            && (sprite.resolved_size != Some(size) || mesh.is_none())
        {
            let mesh_handle = meshes.add(Rectangle::new(size.x.max(1.0), size.y.max(1.0)));
            if let Some(mut mesh) = mesh {
                mesh.0 = mesh_handle;
            } else {
                commands.entity(entity).try_insert(Mesh3d(mesh_handle));
            }
            sprite.resolved_size = Some(size);
        }

        if let Some(material_handle) = material_handle {
            if sprite.is_changed()
                && let Some(mut material) = materials.get_mut(&material_handle.0)
            {
                *material = material_from_sprite(&sprite);
            }
        } else {
            let material = materials.add(material_from_sprite(&sprite));
            commands.entity(entity).try_insert(MeshMaterial3d(material));
        }
    }
}

pub fn install(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/world_sprite.wgsl");
    app.add_plugins(MaterialPlugin::<WorldSpriteMaterial>::default())
        // Story systems mutate authoring state in `Update`; mirror it once,
        // immediately before render extraction, to avoid displaying stale
        // material alpha for a frame during expression changes.
        .add_systems(PostUpdate, sync_world_sprites);
}
