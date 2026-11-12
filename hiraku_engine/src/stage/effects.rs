use bevy::{
    mesh::MeshVertexBufferLayoutRef,
    pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    render::render_resource::{
        AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, CompareFunction,
        RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageEffect {
    pub shader: String,
    pub textures: Vec<String>,
    pub quads: Vec<StageEffectQuad>,
    #[serde(skip)]
    pub(super) shader_handle: Option<Handle<Shader>>,
    #[serde(skip)]
    pub(super) texture_handles: Vec<Handle<Image>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageEffectQuad {
    pub pose: super::StagePose,
    pub size: (f32, f32),
    pub pivot: (f32, f32),
    pub color: [f32; 4],
    #[serde(default)]
    pub emission: Option<StageEmission>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageEmission {
    pub rate: f32,
    pub lifetime: f32,
    pub cycle: f32,
    pub burst: u32,
    pub prewarm: bool,
    pub max_particles: u32,
}
impl StageEmission {
    fn valid(&self) -> bool {
        self.rate.is_finite()
            && self.rate >= 0.0
            && self.lifetime.is_finite()
            && self.lifetime > 0.0
            && self.cycle.is_finite()
            && self.cycle > 0.0
            && self.max_particles > 0
    }
    fn capacity(&self) -> usize {
        ((f64::from(self.rate) * f64::from(self.lifetime)).ceil()
            + f64::from(self.burst) * (f64::from(self.lifetime / self.cycle).ceil() + 1.0))
            .min(f64::from(self.max_particles)) as usize
    }
    fn ages(&self, elapsed: f64) -> Vec<f32> {
        let time = elapsed
            + if self.prewarm {
                f64::from(self.cycle)
            } else {
                0.0
            };
        let lifetime = f64::from(self.lifetime);
        let mut ages = Vec::with_capacity(self.capacity());
        let mut birth = (time * f64::from(self.rate)).floor();
        if self.rate > 0.0 {
            while birth >= 1.0 && ages.len() < self.capacity() {
                let age = time - birth / f64::from(self.rate);
                if age >= lifetime {
                    break;
                }
                ages.push(age as f32);
                birth -= 1.0;
            }
        }
        let mut cycle = (time / f64::from(self.cycle)).floor();
        while self.burst > 0 && cycle >= 0.0 && ages.len() < self.capacity() {
            let age = time - cycle * f64::from(self.cycle);
            if age >= lifetime {
                break;
            }
            for _ in 0..self.burst {
                if ages.len() == self.capacity() {
                    break;
                }
                ages.push(age as f32);
            }
            cycle -= 1.0;
        }
        ages
    }
}

impl StageEffect {
    pub(super) fn validate(&self) -> bool {
        !self.shader.trim().is_empty()
            && self.textures.len() <= 8
            && self.textures.iter().all(|s| !s.trim().is_empty())
            && !self.quads.is_empty()
            && self.quads.iter().all(|quad| {
                quad.pose.validate()
                    && Vec2::from(quad.size).is_finite()
                    && Vec2::from(quad.size).min_element() > 0.0
                    && Vec2::from(quad.pivot).is_finite()
                    && quad.color.into_iter().all(|c| c.is_finite() && c >= 0.0)
                    && quad.emission.as_ref().is_none_or(StageEmission::valid)
            })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StageEffectPlayback {
    pub enabled: bool,
    pub elapsed: f64,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(StageEffectKey)]
pub(super) struct StageEffectMaterial {
    #[uniform(0)]
    pub clock: Vec4,
    #[uniform(1)]
    pub color: Vec4,
    #[texture(2)]
    #[sampler(3)]
    pub texture0: Option<Handle<Image>>,
    #[texture(4)]
    pub texture1: Option<Handle<Image>>,
    #[texture(5)]
    pub texture2: Option<Handle<Image>>,
    #[texture(6)]
    pub texture3: Option<Handle<Image>>,
    #[texture(7)]
    pub texture4: Option<Handle<Image>>,
    #[texture(8)]
    pub texture5: Option<Handle<Image>>,
    #[texture(9)]
    pub texture6: Option<Handle<Image>>,
    #[texture(10)]
    pub texture7: Option<Handle<Image>>,
    pub shader: Handle<Shader>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct StageEffectKey(Handle<Shader>);
impl From<&StageEffectMaterial> for StageEffectKey {
    fn from(material: &StageEffectMaterial) -> Self {
        Self(material.shader.clone())
    }
}
impl Material for StageEffectMaterial {
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = &mut descriptor.fragment {
            fragment.shader = key.bind_group_data.0;
            fragment.shader_defs.push(bevy::shader::ShaderDefVal::UInt(
                "EFFECT_BINDING_GROUP".into(),
                bevy::pbr::MATERIAL_BIND_GROUP_INDEX as u32,
            ));
            let blend = BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            };
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState {
                    color: blend,
                    alpha: BlendComponent {
                        src_factor: BlendFactor::One,
                        ..blend
                    },
                });
            }
        }
        if let Some(depth) = &mut descriptor.depth_stencil {
            depth.depth_write_enabled = Some(false);
            depth.depth_compare = Some(CompareFunction::Greater);
        }
        Ok(())
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
}

#[derive(Component)]
struct EffectQuad {
    name: String,
    particle: usize,
    emission: Option<StageEmission>,
}

pub(super) fn spawn(
    definition: &super::StageDefinition,
    root: Entity,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StageEffectMaterial>,
) {
    for (name, effect) in &definition.effects {
        let shader = effect
            .shader_handle
            .as_ref()
            .expect("validated loaded effect shader");
        for quad in &effect.quads {
            let mut mesh = Mesh::from(Rectangle::new(quad.size.0, quad.size.1));
            mesh.translate_by(Vec3::new(
                (0.5 - quad.pivot.0) * quad.size.0,
                (0.5 - quad.pivot.1) * quad.size.1,
                0.0,
            ));
            let mesh = meshes.add(mesh);
            for particle in 0..quad.emission.as_ref().map_or(1, StageEmission::capacity) {
                let texture = |i| effect.texture_handles.get(i).cloned();
                let material = materials.add(StageEffectMaterial {
                    clock: Vec4::ZERO,
                    color: Vec4::from_array(quad.color),
                    texture0: texture(0),
                    texture1: texture(1),
                    texture2: texture(2),
                    texture3: texture(3),
                    texture4: texture(4),
                    texture5: texture(5),
                    texture6: texture(6),
                    texture7: texture(7),
                    shader: shader.clone(),
                });
                commands.spawn((
                    EffectQuad {
                        name: name.clone(),
                        particle,
                        emission: quad.emission.clone(),
                    },
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    quad.pose.transform(),
                    Visibility::Hidden,
                    ChildOf(root),
                    super::views::spatial_layer(),
                    Pickable::IGNORE,
                    super::runtime::StageSurface,
                ));
            }
        }
    }
}

pub(super) fn sync(
    runtime: Res<super::runtime::StageRuntime>,
    shared: Res<crate::state::SceneSharedState>,
    parents: Query<&ChildOf>,
    mut quads: Query<(
        Entity,
        &EffectQuad,
        &MeshMaterial3d<StageEffectMaterial>,
        &mut Visibility,
    )>,
    mut materials: ResMut<Assets<StageEffectMaterial>>,
    mut redraw: super::redraw::StageRedraw,
) {
    let Some(root) = runtime.root else { return };
    let Some(stage) = &shared.0.spatial_stage else {
        return;
    };
    if !runtime.ready(stage) {
        return;
    }
    for (entity, quad, handle, mut visibility) in &mut quads {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            continue;
        }
        let playback = stage.effects.get(&quad.name);
        let elapsed = playback.map_or(0.0, |p| p.elapsed);
        let age = quad.emission.as_ref().map_or(Some(0.0), |emission| {
            emission.ages(elapsed).get(quad.particle).copied()
        });
        let enabled = playback.is_some_and(|p| p.enabled) && age.is_some();
        let target = if enabled {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != target {
            *visibility = target;
            redraw.request();
        }
        if enabled {
            if let Some(mut material) = materials.get_mut(handle) {
                let clock = Vec4::new(
                    playback.expect("enabled effect").elapsed as f32,
                    age.expect("active particle"),
                    0.0,
                    0.0,
                );
                if material.clock != clock {
                    material.clock = clock;
                    redraw.request();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emission() -> StageEmission {
        StageEmission {
            rate: 2.0,
            lifetime: 0.6,
            cycle: 2.0,
            burst: 1,
            prewarm: false,
            max_particles: 10,
        }
    }

    #[test]
    fn emission_tracks_births_and_expiration_analytically() {
        let source = emission();
        assert_eq!(source.ages(0.0), vec![0.0]);
        assert_eq!(source.ages(0.5), vec![0.0, 0.5]);
        assert_eq!(source.ages(0.75), vec![0.25]);
        assert_eq!(source.ages(2.0), vec![0.0, 0.5, 0.0]);
        assert!(source.ages(0.75).len() <= source.capacity());
    }

    #[test]
    fn prewarming_and_particle_capacity_are_source_driven() {
        let mut warm = emission();
        warm.prewarm = true;
        assert_eq!(warm.ages(0.0), emission().ages(2.0));
        warm.max_particles = 1;
        assert_eq!(warm.capacity(), 1);
        assert_eq!(warm.ages(0.0), vec![0.0]);
        warm.rate = f32::NAN;
        assert!(!warm.valid());
    }

    #[test]
    fn effect_descriptors_reject_invalid_shader_geometry_and_emission() {
        let text = r#".{shader: "alice.wesl", textures: ["bob.png"], quads: [.{
            pose: .{position: (0, 1, 2)}, size: (2, 3), pivot: (0.5, 0), color: [1, 0.5, 0.25, 1]
        }]}"#;
        let mut effect: StageEffect = hiraku_script::hson::from_str(text).expect("effect data");
        assert!(effect.validate());
        effect.quads[0].size.0 = 0.0;
        assert!(!effect.validate());
        effect.quads[0].size.0 = 2.0;
        effect.shader.clear();
        assert!(!effect.validate());
    }

    #[test]
    fn effect_geometry_is_owned_by_the_stage_and_shares_its_mesh() {
        #[derive(Resource)]
        struct Fixture(super::super::StageDefinition);
        fn create(
            mut commands: Commands,
            fixture: Res<Fixture>,
            mut meshes: ResMut<Assets<Mesh>>,
            mut materials: ResMut<Assets<StageEffectMaterial>>,
        ) {
            let root = commands
                .spawn((
                    super::super::StageRoot,
                    Transform::default(),
                    Visibility::Inherited,
                ))
                .id();
            spawn(&fixture.0, root, &mut commands, &mut meshes, &mut materials);
        }
        let mut definition: super::super::StageDefinition = hiraku_script::hson::from_str(r#".{
            defaultCamera: "wide", cameras: .{ wide: .{
                pose: .{ position: (0, 0, 10) }, projection: .{ kind: "perspective", fov: 60, near: 0.1, far: 100 }
            } }, effects: .{ alice: .{ shader: "alice.wesl", textures: [], quads: [.{
                pose: .{position: (0, 0, 0)}, size: (2, 3), pivot: (0.5, 0), color: [1, 1, 1, 1],
                emission: .{ rate: 2, lifetime: 0.6, cycle: 2, burst: 1, prewarm: false, maxParticles: 10 }
            }] } }
        }"#).expect("stage effect fixture");
        definition
            .effects
            .get_mut("alice")
            .expect("effect")
            .shader_handle = Some(Handle::default());
        let capacity = emission().capacity();
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StageEffectMaterial>>()
            .insert_resource(Fixture(definition))
            .add_systems(Startup, create);
        app.update();
        let world = app.world_mut();
        let roots = world
            .query_filtered::<Entity, With<super::super::StageRoot>>()
            .iter(world)
            .collect::<Vec<_>>();
        let rows = world
            .query::<(&EffectQuad, &Mesh3d, &Visibility, &ChildOf)>()
            .iter(world)
            .map(|(quad, mesh, visibility, parent)| {
                (quad.particle, mesh.id(), *visibility, parent.parent())
            })
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), capacity);
        assert!(
            rows.iter()
                .all(|(_, mesh, visibility, parent)| *mesh == rows[0].1
                    && *visibility == Visibility::Hidden
                    && *parent == roots[0])
        );
        world.entity_mut(roots[0]).despawn();
        assert_eq!(world.query::<&EffectQuad>().iter(world).count(), 0);
    }
}

pub(super) fn register(app: &mut App) {
    app.add_plugins(MaterialPlugin::<StageEffectMaterial>::default())
        .add_systems(
            PostUpdate,
            sync.after(super::runtime::prepare_surfaces)
                .run_if(crate::runtime_initialized),
        );
}
