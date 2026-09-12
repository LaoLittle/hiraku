use bevy::{
    asset::Handle,
    pbr::{Material, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
};

#[cfg(test)]
mod dissolve_shader_tests {
    #[test]
    fn sprite_dissolve_shader_parses_and_validates_without_a_gpu() {
        let source = include_str!("shaders/world_sprite.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) world_position: vec4<f32>, };")
            .replace("#{MATERIAL_BIND_GROUP}", "2");
        let module = naga::front::wgsl::parse_str(&source).expect("sprite WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("sprite WGSL validates");
    }
}

/// Authoring data for a flat image rendered by Hiraku's 3D world camera.
///
/// The component deliberately owns image-space concerns while `Transform`
/// remains available for story-level position, scale and animation.
#[derive(Component, Clone, Debug)]
pub struct WorldSprite {
    /// Canvas/world XY plane: keep dot(normal, position) <= distance.
    pub clip_plane: Option<Vec3>,
    /// Nine-slice borders in source pixels: left, top, right, bottom.
    pub slice: Option<[f32; 4]>,
    pub clip: Option<hiraku_sprite3d::ClipRect>,
    /// Sampling radius in source-image pixels; independent of camera effects.
    pub blur_radius: f32,
    /// Noise frame (zero disables), grid width, grid height.
    pub noise: Vec3,
    pub dissolve: Option<DissolveMask>,
    pub image: Option<Handle<Image>>,
    /// Source rectangle as `[left, top, width, height]` in pixels.
    pub rect: Option<[f32; 4]>,
    pub color: Color,
    pub custom_size: Option<Vec2>,
    resolved_size: Option<Vec2>,
}

#[derive(Clone, Debug)]
pub struct DissolveMask {
    pub reversed: bool,
    pub canvas_size: Vec2,
    pub image: Handle<Image>,
    pub path: String,
    pub softness: f32,
}

impl WorldSprite {
    pub fn from_image(image: Handle<Image>) -> Self {
        Self {
            clip_plane: None,
            slice: None,
            clip: None,
            blur_radius: 0.0,
            noise: Vec3::ZERO,
            dissolve: None,
            image: Some(image),
            rect: None,
            color: Color::WHITE,
            custom_size: None,
            resolved_size: None,
        }
    }

    pub fn from_color(color: Color, size: Vec2) -> Self {
        Self {
            clip_plane: None,
            slice: None,
            clip: None,
            blur_radius: 0.0,
            noise: Vec3::ZERO,
            dissolve: None,
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
    pub clip_plane: Vec4,
    pub slice_borders: Vec4,
    pub slice_size: Vec4,
    pub clip_bounds: Vec4,
    pub clip_axes: Vec4,
    pub effects: Vec4,
    #[texture(3)]
    #[sampler(4)]
    pub dissolve_mask: Option<Handle<Image>>,
    pub dissolve: Vec4,
    #[texture(1)]
    #[sampler(2)]
    pub image: Option<Handle<Image>>,
    pub tint: Vec4,
    pub rect: Vec4,
}

#[derive(Clone, Debug, ShaderType)]
pub struct WorldSpriteUniform {
    clip_plane: Vec4,
    slice_borders: Vec4,
    slice_size: Vec4,
    clip_bounds: Vec4,
    clip_axes: Vec4,
    effects: Vec4,
    dissolve: Vec4,
    tint: Vec4,
    rect: Vec4,
}

impl From<&WorldSpriteMaterial> for WorldSpriteUniform {
    fn from(material: &WorldSpriteMaterial) -> Self {
        Self {
            clip_plane: material.clip_plane,
            slice_borders: material.slice_borders,
            slice_size: material.slice_size,
            clip_bounds: material.clip_bounds,
            clip_axes: material.clip_axes,
            effects: material.effects,
            dissolve: material.dissolve,
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
        clip_plane: sprite
            .clip_plane
            .map_or(Vec4::ZERO, |plane| plane.extend(1.0)),
        slice_borders: sprite.slice.map(Vec4::from_array).unwrap_or(Vec4::ZERO),
        slice_size: sprite.slice.map_or(Vec4::ZERO, |_| {
            sprite
                .resolved_size
                .unwrap_or(Vec2::ONE)
                .extend(1.0)
                .extend(0.0)
        }),
        clip_bounds: sprite
            .clip
            .map_or(Vec4::ZERO, |clip| clip.shader_parameters()[0]),
        clip_axes: sprite
            .clip
            .map_or(Vec4::ZERO, |clip| clip.shader_parameters()[1]),
        effects: Vec4::new(
            sprite.blur_radius,
            sprite.noise.x,
            sprite.noise.y,
            sprite.noise.z,
        ),
        dissolve_mask: sprite.dissolve.as_ref().map(|mask| mask.image.clone()),
        dissolve: sprite.dissolve.as_ref().map_or(Vec4::ZERO, |mask| {
            Vec4::new(
                if mask.reversed { -1.0 } else { 1.0 },
                mask.softness,
                mask.canvas_size.x,
                mask.canvas_size.y,
            )
        }),
        image: sprite.image.clone(),
        tint: sprite.color.to_linear().to_f32_array().into(),
        rect: sprite.rect.map(Vec4::from_array).unwrap_or(Vec4::ZERO),
    }
}

/// Resolves natural image sizes lazily and mirrors authoring changes into GPU
/// assets. No transform is rewritten, so animation state stays independent.
pub(crate) fn sync_world_sprites(
    mut commands: Commands,
    canvas: Option<Res<crate::HirakuCanvas>>,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut sprites: Query<
        (
            Entity,
            &mut WorldSprite,
            Option<&mut Mesh3d>,
            Option<&MeshMaterial3d<WorldSpriteMaterial>>,
            Has<crate::scene::BackgroundLayer>,
        ),
        Without<crate::scene::character_composite::LogicalCharacterPart>,
    >,
) {
    for (entity, mut sprite, mesh, material_handle, background) in &mut sprites {
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
        // Fit only the default natural size. Explicit sizing and the entity's
        // story transform remain independent. Resolving here also covers lazy
        // image loads, background transitions, and snapshot reconstruction.
        let size = size.map(|size| {
            if background && sprite.custom_size.is_none() {
                canvas
                    .as_ref()
                    .map_or(size, |canvas| fit_background(size, canvas.size.as_vec2()))
            } else {
                size
            }
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

fn fit_background(source: Vec2, viewport: Vec2) -> Vec2 {
    if !source.is_finite()
        || source.min_element() <= 0.0
        || !viewport.is_finite()
        || viewport.min_element() <= 0.0
    {
        return Vec2::ONE;
    }
    source * (viewport / source).min_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_fit_contains_image_without_distortion() {
        let viewport = Vec2::new(1920.0, 1080.0);
        assert_eq!(
            fit_background(Vec2::new(3840.0, 2160.0), viewport),
            viewport
        );
        assert_eq!(
            fit_background(Vec2::new(4096.0, 2048.0), viewport),
            Vec2::new(1920.0, 960.0)
        );
        assert_eq!(
            fit_background(Vec2::new(100.0, 200.0), viewport),
            Vec2::new(540.0, 1080.0)
        );
    }

    #[test]
    fn background_fit_handles_invalid_dimensions() {
        assert_eq!(fit_background(Vec2::ZERO, Vec2::ONE), Vec2::ONE);
        assert_eq!(fit_background(Vec2::ONE, Vec2::splat(f32::NAN)), Vec2::ONE);
    }

    #[test]
    fn background_sync_preserves_story_transform_and_explicit_size() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<WorldSpriteMaterial>>()
            .insert_resource(crate::HirakuCanvas {
                image: Handle::default(),
                size: UVec2::new(1920, 1080),
            })
            .add_systems(Update, sync_world_sprites);
        let source =
            WorldSprite::from_image(Handle::default()).with_rect(Some([0.0, 0.0, 4096.0, 2048.0]));
        let transform = Transform::from_xyz(12.0, 34.0, -10.0).with_scale(Vec3::splat(2.0));
        let background = app
            .world_mut()
            .spawn((
                source.clone(),
                crate::scene::BackgroundLayer {
                    path: "background/room".into(),
                },
                transform,
            ))
            .id();
        let actor = app.world_mut().spawn(source.clone()).id();
        let mut sized = source;
        sized.custom_size = Some(Vec2::new(320.0, 180.0));
        let explicit = app
            .world_mut()
            .spawn((
                sized,
                crate::scene::BackgroundLayer {
                    path: "background/detail".into(),
                },
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<WorldSprite>(background)
                .expect("background")
                .resolved_size,
            Some(Vec2::new(1920.0, 960.0))
        );
        assert_eq!(
            *app.world()
                .get::<Transform>(background)
                .expect("story transform"),
            transform
        );
        assert_eq!(
            app.world()
                .get::<WorldSprite>(actor)
                .expect("actor")
                .resolved_size,
            Some(Vec2::new(4096.0, 2048.0))
        );
        assert_eq!(
            app.world()
                .get::<WorldSprite>(explicit)
                .expect("explicit size")
                .resolved_size,
            Some(Vec2::new(320.0, 180.0))
        );
        app.world_mut().resource_mut::<crate::HirakuCanvas>().size = UVec2::new(960, 540);
        app.update();
        assert_eq!(
            app.world()
                .get::<WorldSprite>(background)
                .expect("resized background")
                .resolved_size,
            Some(Vec2::new(960.0, 480.0))
        );
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
