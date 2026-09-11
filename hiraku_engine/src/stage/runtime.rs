use super::*;
use crate::script::AnimationSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StageCommand {
    Clip { id: u64, view: String, clip: super::ViewClip },
    Open {
        id: u64,
        path: String,
    },
    Close {
        id: u64,
    },
    Camera {
        id: u64,
        view: String,
        name: String,
        animation: AnimationSpec,
    },
    View {
        id: u64,
        view: String,
        visible: bool,
        animation: AnimationSpec,
    },
    Place {
        id: u64,
        actor: String,
        anchor: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageSnapshot {
    pub id: u64,
    pub path: String,
    pub views: BTreeMap<String, StageViewSnapshot>,
    pub actors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StageViewSnapshot {
    #[serde(default)]
    pub clip: super::ViewClip,
    pub camera: Option<StageCamera>,
    pub camera_name: Option<String>,
    pub request: Option<(String, AnimationSpec)>,
    pub tween: Option<StageCameraTween>,
    pub alpha: f32,
    pub order: u32,
    pub fade: Option<StageViewFade>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageViewFade {
    from: f32,
    to: f32,
    elapsed: f32,
    animation: AnimationSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageCameraTween {
    from: StageCamera,
    to: StageCamera,
    elapsed: f32,
    animation: AnimationSpec,
}

impl StageSnapshot {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let orders: std::collections::BTreeSet<_> = self.views.values().map(|v| v.order).collect();
        if self.id == 0
            || self.path.trim().is_empty()
            || self
                .views
                .iter()
                .any(|(name, view)| name.trim().is_empty() || !view.valid())
            || !self.views.contains_key("main")
            || orders.len() != self.views.len()
            || orders.iter().copied().ne(0..self.views.len() as u32)
            || self.views.get("main").is_some_and(|v| v.order != 0)
            || self
                .actors
                .iter()
                .any(|(actor, anchor)| actor.trim().is_empty() || anchor.trim().is_empty())
        {
            return Err("invalid spatial stage state".into());
        }
        Ok(())
    }
    pub fn apply(state: &mut Option<Self>, command: StageCommand) -> Result<(), String> {
        let id = match &command {
            StageCommand::Open { .. } => None,
            StageCommand::Close { id }
            | StageCommand::Clip { id, .. }
            | StageCommand::View { id, .. }
            | StageCommand::Camera { id, .. }
            | StageCommand::Place { id, .. } => Some(*id),
        };
        if let Some(id) = id {
            if state.as_ref().is_none_or(|stage| stage.id != id) {
                return Err("stage handle is no longer active".into());
            }
        }
        match command {
            StageCommand::Clip { view, clip, .. } => {
                if !clip.valid() { return Err("invalid stage view clip".into()); }
                state.as_mut().ok_or("stage is not open")?.views.get_mut(&view)
                    .ok_or("configure a stage view camera before clipping it")?.clip = clip;
            }
            StageCommand::Open { id, path } => {
                *state = Some(Self {
                    id,
                    path,
                    views: BTreeMap::from([(
                        "main".into(),
                        StageViewSnapshot {
                            alpha: 1.0,
                            ..default()
                        },
                    )]),
                    actors: BTreeMap::new(),
                });
            }
            StageCommand::Close { .. } => *state = None,
            StageCommand::Camera {
                view,
                name,
                animation,
                ..
            } => {
                if view.trim().is_empty() || name.trim().is_empty() || !valid_animation(animation) {
                    return Err("invalid stage camera animation".into());
                }
                let views = &mut state.as_mut().ok_or("stage is not open")?.views;
                let order = views.len() as u32;
                views
                    .entry(view)
                    .or_insert_with(|| StageViewSnapshot { order, ..default() })
                    .request = Some((name, animation));
            }
            StageCommand::View {
                view,
                visible,
                animation,
                ..
            } => {
                if !valid_animation(animation) {
                    return Err("invalid stage view animation".into());
                }
                let view = state
                    .as_mut()
                    .ok_or("stage is not open")?
                    .views
                    .get_mut(&view)
                    .ok_or("configure a stage view camera before showing it")?;
                let to = if visible { 1.0 } else { 0.0 };
                if animation.duration() == 0.0 {
                    view.alpha = to;
                    view.fade = None;
                } else {
                    view.fade = Some(StageViewFade {
                        from: view.alpha,
                        to,
                        elapsed: 0.0,
                        animation,
                    });
                }
            }
            StageCommand::Place { actor, anchor, .. } => {
                state
                    .as_mut()
                    .ok_or("stage is not open")?
                    .actors
                    .insert(actor, anchor);
            }
        }
        Ok(())
    }
}

impl StageViewSnapshot {
    fn valid(&self) -> bool {
        if !self.clip.valid() { return false; }
        !(self
            .camera
            .as_ref()
            .is_some_and(|camera| !camera.validate())
            || self.request.as_ref().is_some_and(|(name, animation)| {
                name.trim().is_empty() || !valid_animation(*animation)
            })
            || self.tween.as_ref().is_some_and(|tween| {
                !tween.from.validate()
                    || !tween.to.validate()
                    || !valid_animation(tween.animation)
                    || tween.animation.duration() <= 0.0
                    || !tween.elapsed.is_finite()
                    || tween.elapsed < 0.0
                    || tween.elapsed > tween.animation.duration()
            })
            || !self.alpha.is_finite()
            || !(0.0..=1.0).contains(&self.alpha)
            || self.fade.as_ref().is_some_and(|f| {
                !f.from.is_finite()
                    || !f.to.is_finite()
                    || !(0.0..=1.0).contains(&f.from)
                    || !(0.0..=1.0).contains(&f.to)
                    || !valid_animation(f.animation)
                    || f.animation.duration() <= 0.0
                    || !f.elapsed.is_finite()
                    || f.elapsed < 0.0
                    || f.elapsed > f.animation.duration()
            }))
    }
}

fn valid_animation(animation: AnimationSpec) -> bool {
    let (seconds, repeat) = match animation {
        AnimationSpec::Linear(seconds, repeat)
        | AnimationSpec::EaseIn(seconds, repeat)
        | AnimationSpec::EaseOut(seconds, repeat)
        | AnimationSpec::EaseOutSine(seconds, repeat)
        | AnimationSpec::EaseInOutSine(seconds, repeat)
        | AnimationSpec::EaseInOut(seconds, repeat) => (seconds, repeat),
    };
    seconds.is_finite() && seconds >= 0.0 && seconds <= f32::MAX as f64 && !repeat
}

#[derive(Resource, Default)]
pub(crate) struct StageRuntime {
    path: Option<String>,
    id: Option<u64>,
    handle: Option<Handle<StageDefinition>>,
    root: Option<Entity>,
    instantiated: bool,
    pub error: Option<String>,
    pub definition: Option<StageDefinition>,
    materials: std::collections::HashMap<AssetId<StandardMaterial>, Handle<StandardMaterial>>,
}
impl StageRuntime {
    pub fn ready(&self, state: &StageSnapshot) -> bool {
        self.id == Some(state.id) && self.path.as_deref() == Some(&state.path) && self.instantiated
    }
}

pub(crate) fn sync(
    mut commands: Commands,
    server: Res<AssetServer>,
    definitions: Res<Assets<StageDefinition>>,
    mut shared: ResMut<crate::state::SceneSharedState>,
    mut runtime: ResMut<StageRuntime>,
    time: crate::scene::playback::StoryTime,
    canvas: Option<Res<crate::HirakuCanvas>>,
    mut redraw: crate::redraw::Redraw,
    mut camera: Query<
        (&mut Transform, &mut Projection),
        With<crate::render::camera::WorldCamera3d>,
    >,
    instances: Query<(&bevy::world_serialization::WorldInstance, &ChildOf)>,
    spawner: Option<Res<WorldInstanceSpawner>>,
) {
    let path = shared
        .0
        .spatial_stage
        .as_ref()
        .map(|stage| stage.path.clone());
    let id = shared.0.spatial_stage.as_ref().map(|stage| stage.id);
    if path != runtime.path || id != runtime.id {
        if let Some(root) = runtime.root.take() {
            commands.entity(root).try_despawn();
        }
        runtime.definition = None;
        runtime.instantiated = false;
        runtime.materials.clear();
        runtime.error = None;
        runtime.handle = path.as_ref().map(|path| server.load(path.clone()));
        runtime.path = path;
        runtime.id = id;
    }
    let Some(stage) = shared.0.spatial_stage.as_mut() else {
        return;
    };
    let Some(handle) = &runtime.handle else {
        return;
    };
    if runtime.error.is_some() {
        return;
    }
    if let bevy::asset::LoadState::Failed(error) = server.load_state(handle.id()) {
        runtime.error = Some(format!("failed to load stage `{}`: {error}", stage.path));
        return;
    }
    if let bevy::asset::RecursiveDependencyLoadState::Failed(error) =
        server.recursive_dependency_load_state(handle.id())
    {
        runtime.error = Some(format!(
            "failed to load stage dependencies `{}`: {error}",
            stage.path
        ));
        return;
    }
    if runtime.root.is_none() {
        redraw.request();
        if !server.is_loaded_with_dependencies(handle.id()) {
            return;
        }
        let Some(definition) = definitions.get(handle) else {
            return;
        };
        match definition.spawn(&mut commands) {
            Ok(root) => {
                runtime.root = Some(root);
                runtime.definition = Some(definition.clone());
            }
            Err(error) => {
                runtime.error = Some(error.to_string());
                return;
            }
        }
    }
    if !runtime.instantiated {
        redraw.request();
        let has_model = runtime
            .definition
            .as_ref()
            .expect("loaded stage definition")
            .scene
            .is_some();
        runtime.instantiated = !has_model
            || instances.iter().any(|(instance, parent)| {
                Some(parent.parent()) == runtime.root
                    && spawner
                        .as_ref()
                        .is_some_and(|spawner| spawner.instance_is_ready(**instance))
            });
        if has_model && spawner.is_none() {
            runtime.error =
                Some("stage model rendering requires Bevy's WorldSerializationPlugin".into());
            return;
        }
    }
    let definition = runtime
        .definition
        .as_ref()
        .expect("loaded stage definition");
    for anchor in stage.actors.values() {
        if let Err(error) = definition.anchor(anchor) {
            runtime.error = Some(error.to_string());
            return;
        }
    }
    for view in stage.views.values_mut() {
        if view.tween.is_some() || view.fade.is_some() || view.request.is_some() {
            redraw.request();
        }
        if let Err(error) = tick_view(view, definition, time.delta_secs()) {
            runtime.error = Some(error);
            return;
        }
        if view.tween.is_some() || view.fade.is_some() {
            redraw.request();
        }
    }
    // Stage cameras render only spatial surfaces. The presentation camera stays
    // in canvas coordinates so curtains, screen pictures and UI keep their size.
    if let (Some(canvas), Ok((mut transform, mut projection))) = (canvas, camera.single_mut()) {
        transform.set_if_neq(Transform::from_xyz(0.0, 0.0, 1000.0));
        let mut lens = OrthographicProjection::default_3d();
        lens.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
            viewport_height: canvas.size.y as f32,
        };
        lens.near = -2000.0;
        lens.far = 2000.0;
        let next = Projection::Orthographic(lens);
        if projection_kind_values(&projection) != projection_kind_values(&next) {
            *projection = next;
        }
    }
}

fn tick_view(
    stage: &mut StageViewSnapshot,
    definition: &StageDefinition,
    delta: f32,
) -> Result<(), String> {
    if stage.camera.is_none() {
        stage.camera = definition.cameras.get(&definition.default_camera).cloned();
        stage.camera_name = Some(definition.default_camera.clone());
    }
    if let Some((name, animation)) = stage.request.take() {
        let target = match definition.camera(&name) {
            Ok(target) => target.clone(),
            Err(error) => return Err(error.to_string()),
        };
        stage.camera_name = Some(name);
        if animation.duration() > 0.0 {
            stage.tween = Some(StageCameraTween {
                from: stage.camera.clone().expect("initial camera"),
                to: target,
                animation,
                elapsed: 0.0,
            });
        } else {
            stage.camera = Some(target);
            stage.tween = None;
        }
    }
    if let Some(tween) = &mut stage.tween {
        let seconds = tween.animation.duration();
        tween.elapsed = (tween.elapsed + delta).min(seconds);
        stage.camera = Some(interpolate_camera(
            &tween.from,
            &tween.to,
            tween.animation.sample(tween.elapsed / seconds),
        ));
        if tween.elapsed >= seconds {
            stage.tween = None;
        }
    }
    if let Some(fade) = &mut stage.fade {
        fade.elapsed = (fade.elapsed + delta).min(fade.animation.duration());
        stage.alpha = fade.from
            + (fade.to - fade.from)
                * fade
                    .animation
                    .sample(fade.elapsed / fade.animation.duration());
        if fade.elapsed >= fade.animation.duration() {
            stage.fade = None;
        }
    }
    Ok(())
}

#[derive(Component)]
pub(crate) struct StageSurface;

/// World assets instantiate asynchronously. Decorate newly spawned descendants
/// without modifying the source asset or materials shared with another scene.
pub(crate) fn prepare_surfaces(
    mut commands: Commands,
    mut runtime: ResMut<StageRuntime>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    surfaces: Query<(Entity, &MeshMaterial3d<StandardMaterial>), Without<StageSurface>>,
    lights: Query<
        Entity,
        (
            Or<(With<PointLight>, With<SpotLight>, With<DirectionalLight>)>,
            Without<StageSurface>,
        ),
    >,
    parents: Query<&ChildOf>,
) {
    let Some(root) = runtime.root else { return };
    let unlit = runtime.definition.as_ref().is_some_and(|d| d.unlit);
    // RenderLayers are not inherited through ChildOf. Imported lights must
    // illuminate the stage's layer, not Bevy's default layer zero.
    for entity in &lights {
        if parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            commands
                .entity(entity)
                .insert((StageSurface, super::views::spatial_layer()));
        }
    }
    for (entity, material) in &surfaces {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            continue;
        }
        if unlit {
            let handle = if let Some(handle) = runtime.materials.get(&material.id()) {
                handle.clone()
            } else {
                let Some(source) = materials.get(material) else {
                    continue;
                };
                let mut source = source.clone();
                source.unlit = true;
                let handle = materials.add(source);
                runtime.materials.insert(material.id(), handle.clone());
                handle
            };
            commands.entity(entity).insert(MeshMaterial3d(handle));
        }
        commands
            .entity(entity)
            .insert((StageSurface, super::views::spatial_layer()));
    }
}

pub(super) fn projection_kind_values(p: &Projection) -> (u8, f32, f32, f32) {
    match p {
        Projection::Perspective(p) => (0, p.fov, p.near, p.far),
        Projection::Orthographic(p) => (
            1,
            match p.scaling_mode {
                bevy::camera::ScalingMode::FixedVertical { viewport_height } => viewport_height,
                _ => 0.0,
            },
            p.near,
            p.far,
        ),
        _ => (2, 0.0, 0.0, 0.0),
    }
}

fn interpolate_angle(from: f32, to: f32, t: f32) -> f32 {
    if t >= 1.0 { return to; }
    from + ((to - from + 180.0).rem_euclid(360.0) - 180.0) * t
}

fn interpolate_camera(from: &StageCamera, to: &StageCamera, t: f32) -> StageCamera {
    let a = from.pose.transform();
    let b = to.pose.transform();
    let rotation = a.rotation.slerp(b.rotation, t).to_euler(EulerRot::XYZ);
    let pose = StagePose {
        position: a.translation.lerp(b.translation, t).into(),
        rotation: (
            rotation.0.to_degrees(),
            rotation.1.to_degrees(),
            rotation.2.to_degrees(),
        ),
        scale: (1.0, 1.0, 1.0),
    };
    let pose = match (&from.pose, &to.pose) {
        (StageCameraPose::Orbit(a), StageCameraPose::Orbit(b)) if a.outward == b.outward => {
            StageCameraPose::Orbit(StageOrbit {
                pivot: Vec3::from(a.pivot).lerp(Vec3::from(b.pivot), t).into(),
                rotation: (
                    interpolate_angle(a.rotation.0, b.rotation.0, t),
                    interpolate_angle(a.rotation.1, b.rotation.1, t),
                    interpolate_angle(a.rotation.2, b.rotation.2, t),
                ),
                distance: a.distance + (b.distance - a.distance) * t,
                outward: b.outward,
            })
        }
        _ => StageCameraPose::Fixed(pose),
    };
    let projection = match (&from.projection, &to.projection) {
        (
            StageProjection::Perspective { fov: a, .. },
            StageProjection::Perspective { fov: b, near, far },
        ) => StageProjection::Perspective {
            fov: a + (b - a) * t,
            near: *near,
            far: *far,
        },
        (
            StageProjection::Orthographic { height: a, .. },
            StageProjection::Orthographic {
                height: b,
                near,
                far,
            },
        ) => StageProjection::Orthographic {
            height: a + (b - a) * t,
            near: *near,
            far: *far,
        },
        _ => to.projection.clone(),
    };
    StageCamera { pose, projection }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_tracks_advance_independently_and_survive_restore_mid_crossfade() {
        let definition: StageDefinition = hiraku_script::hson::from_str(r#".{
            defaultCamera: "wide", cameras: .{
                wide: .{ pose: .{ position: (0, 0, 10) }, projection: .{ kind: "perspective", fov: 60, near: 0.1, far: 100 } },
                near: .{ pose: .{ position: (0, 0, 2) }, projection: .{ kind: "perspective", fov: 60, near: 0.1, far: 100 } }
            }
        }"#).expect("fixture stage");
        let mut state = None;
        StageSnapshot::apply(
            &mut state,
            StageCommand::Open {
                id: 1,
                path: "fixture.stage.hson".into(),
            },
        )
        .expect("open");
        StageSnapshot::apply(
            &mut state,
            StageCommand::Camera {
                id: 1,
                view: "overlay".into(),
                name: "near".into(),
                animation: AnimationSpec::Linear(2.0, false),
            },
        )
        .expect("hidden motion");
        let stage = state.as_mut().expect("stage");
        for view in stage.views.values_mut() {
            tick_view(view, &definition, 1.0).expect("tick");
        }
        assert_eq!(
            stage.views["main"]
                .camera
                .as_ref()
                .expect("camera")
                .pose
                .transform()
                .translation
                .z,
            10.0
        );
        assert_eq!(
            stage.views["overlay"]
                .camera
                .as_ref()
                .expect("camera")
                .pose
                .transform()
                .translation
                .z,
            6.0
        );
        assert_eq!(stage.views["overlay"].alpha, 0.0);
        StageSnapshot::apply(
            &mut state,
            StageCommand::View {
                id: 1,
                view: "overlay".into(),
                visible: true,
                animation: AnimationSpec::Linear(1.0, false),
            },
        )
        .expect("show");
        let stage = state.as_mut().expect("stage");
        for view in stage.views.values_mut() {
            tick_view(view, &definition, 0.25).expect("tick");
        }
        let encoded = hiraku_script::hson::to_string(stage).expect("snapshot");
        let mut restored: StageSnapshot = hiraku_script::hson::from_str(&encoded).expect("restore");
        restored.validate().expect("valid snapshot");
        assert_eq!(restored.views["overlay"].alpha, 0.25);
        for view in restored.views.values_mut() {
            tick_view(view, &definition, 0.75).expect("resume");
        }
        assert_eq!(restored.views["overlay"].alpha, 1.0);
        assert!(restored.views["overlay"].tween.is_none());
        assert!(restored.views["overlay"].fade.is_none());
        assert_eq!(
            restored.views["overlay"]
                .camera
                .as_ref()
                .expect("camera")
                .pose
                .transform()
                .translation
                .z,
            2.0
        );
    }

    #[test]
    fn view_targets_are_owned_by_stage_and_removed_on_close() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<crate::state::SceneSharedState>()
            .init_resource::<StageRuntime>()
            .init_resource::<super::super::views::StageViews>()
            .insert_resource(crate::HirakuCanvas {
                image: Handle::default(),
                size: UVec2::new(320, 180),
            })
            .add_systems(Update, super::super::views::sync);
        // Use the actual primary-camera setup to avoid depending on its private fields.
        let mut images = app
            .world_mut()
            .remove_resource::<Assets<Image>>()
            .expect("images");
        {
            let mut commands = app.world_mut().commands();
            crate::render::camera::setup_stage_cameras(
                &mut commands,
                &mut images,
                &crate::RuntimeLaunchConfig::default(),
            );
        }
        app.world_mut().flush();
        app.insert_resource(images);
        let mut state = None;
        StageSnapshot::apply(
            &mut state,
            StageCommand::Open {
                id: 1,
                path: "fixture.stage.hson".into(),
            },
        )
        .expect("open");
        state
            .as_mut()
            .expect("stage")
            .views
            .get_mut("main")
            .expect("main")
            .camera = Some(view(0.0));
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .spatial_stage = state;
        {
            let mut runtime = app.world_mut().resource_mut::<StageRuntime>();
            runtime.path = Some("fixture.stage.hson".into());
            runtime.id = Some(1);
            runtime.instantiated = true;
        }
        app.update();
        let camera_count = app.world_mut().query::<&Camera>().iter(app.world()).count();
        assert_eq!(camera_count, 2);
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .spatial_stage = None;
        app.update();
        assert_eq!(
            app.world_mut().query::<&Camera>().iter(app.world()).count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::render::world_sprite::WorldSprite>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn imported_lights_use_the_stage_layer_without_touching_external_lights() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<StageRuntime>()
            .add_systems(Update, prepare_surfaces);
        let root = app.world_mut().spawn(StageRoot).id();
        let child = app.world_mut().spawn(ChildOf(root)).id();
        let light = app
            .world_mut()
            .spawn((SpotLight::default(), ChildOf(child)))
            .id();
        let external = app.world_mut().spawn(SpotLight::default()).id();
        app.world_mut().resource_mut::<StageRuntime>().root = Some(root);
        app.update();
        assert_eq!(
            app.world()
                .get::<bevy::camera::visibility::RenderLayers>(light),
            Some(&super::super::views::spatial_layer())
        );
        assert!(app.world().get::<StageSurface>(light).is_some());
        assert!(app.world().get::<StageSurface>(external).is_none());
        app.update();
        assert!(app.world().get_entity(light).is_ok());
    }

    #[test]
    fn headless_stage_systems_release_the_owned_hierarchy_on_close() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<StageDefinition>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<crate::state::SceneSharedState>()
            .init_resource::<StageRuntime>()
            .add_systems(Update, (sync, prepare_surfaces).chain());
        let root = app
            .world_mut()
            .spawn((StageRoot, Transform::default()))
            .id();
        let child = app
            .world_mut()
            .spawn((StageAnchor("alice".into()), ChildOf(root)))
            .id();
        {
            let mut runtime = app.world_mut().resource_mut::<StageRuntime>();
            runtime.path = Some("room.stage.hson".into());
            runtime.id = Some(1);
            runtime.root = Some(root);
        }
        // No stage snapshot means the presentation was closed or reset.
        app.update();
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get_entity(child).is_err());
        let runtime = app.world().resource::<StageRuntime>();
        assert!(runtime.root.is_none() && runtime.handle.is_none());
        assert!(runtime.materials.is_empty());
    }

    fn view(yaw: f32) -> StageCamera {
        StageCamera {
            pose: StageCameraPose::Orbit(StageOrbit {
                pivot: (0.0, 2.0, 0.0),
                rotation: (0.0, yaw, 0.0),
                distance: 4.0,
                outward: true,
            }),
            projection: StageProjection::Perspective {
                fov: 60.0,
                near: 0.1,
                far: 100.0,
            },
        }
    }

    #[test]
    fn orbit_keeps_radius_and_outward_view_during_rotation() {
        let camera = interpolate_camera(&view(0.0), &view(120.0), 0.5)
            .pose
            .transform();
        let direction = (camera.translation - Vec3::Y * 2.0).normalize();
        assert!((camera.translation.distance(Vec3::Y * 2.0) - 4.0).abs() < 0.0001);
        assert!(camera.forward().as_vec3().abs_diff_eq(direction, 0.0001));
    }

    #[test]
    fn orbit_crosses_zero_by_the_short_arc_in_both_directions() {
        for (a, b, mid) in [(350.0, 10.0, 360.0), (10.0, 350.0, 0.0), (0.0, -346.0, 7.0)] {
            assert!((interpolate_angle(a, b, 0.5) - mid).abs() < 0.001);
            assert_eq!(interpolate_angle(a, b, 1.0), b);
            let pose = interpolate_camera(&view(a), &view(b), 0.5).pose.transform();
            assert!(pose.translation.abs_diff_eq(view(mid).pose.transform().translation, 0.001));
        }
    }

    #[test]
    fn snapshot_preserves_camera_progress_and_actor_anchors() {
        let mut snapshot = None;
        StageSnapshot::apply(
            &mut snapshot,
            StageCommand::Open {
                id: 1,
                path: "stages/room.stage.hson".into(),
            },
        )
        .expect("open");
        StageSnapshot::apply(
            &mut snapshot,
            StageCommand::Place {
                id: 1,
                actor: "alice".into(),
                anchor: "desk".into(),
            },
        )
        .expect("place");
        let stage = snapshot.as_mut().expect("stage");
        stage.views.get_mut("main").expect("main view").tween = Some(StageCameraTween {
            from: view(0.0),
            to: view(120.0),
            elapsed: 0.7,
            animation: AnimationSpec::EaseOut(2.0, false),
        });
        let encoded = hiraku_script::hson::to_string(stage).expect("save");
        let restored: StageSnapshot = hiraku_script::hson::from_str(&encoded).expect("restore");
        assert_eq!(
            restored.actors.get("alice").map(String::as_str),
            Some("desk")
        );
        let scene = crate::state::SceneSnapshot {
            spatial_stage: Some(restored.clone()),
            ..default()
        };
        let proto = crate::proto::SceneSnapshot::from(&scene);
        let restored_scene =
            crate::state::SceneSnapshot::try_from(proto).expect("scene storage round trip");
        assert_eq!(
            hiraku_script::hson::to_string(&restored_scene.spatial_stage.expect("stage retained"))
                .expect("serialized stage"),
            encoded
        );
        let tween = restored
            .views
            .get("main")
            .expect("main view")
            .tween
            .as_ref()
            .expect("saved tween");
        assert_eq!(tween.elapsed, 0.7);
        assert_eq!(tween.animation, AnimationSpec::EaseOut(2.0, false));
        StageSnapshot::apply(
            &mut snapshot,
            StageCommand::Open {
                id: 2,
                path: "stages/room.stage.hson".into(),
            },
        )
        .expect("replace");
        assert!(StageSnapshot::apply(&mut snapshot, StageCommand::Close { id: 1 }).is_err());
        StageSnapshot::apply(&mut snapshot, StageCommand::Close { id: 2 }).expect("close current");
        assert!(snapshot.is_none());
    }
}
