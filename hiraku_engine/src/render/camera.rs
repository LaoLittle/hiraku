use crate::{
    HirakuCanvas, RuntimeLaunchConfig,
    effect::post_process::PostProcessSettings,
    scene::{AnimationState, apply_character_ease, complete_missing_animation, tween_fraction},
    script::{CameraEffectScope, CameraProjectionMode},
};
use crate::{
    effect::{custom::CustomScreenEffectPlayer, transition::RuleTransitionPlayer},
    scene::{
        BackgroundLayer, ChoiceUi, DialogueRoot, FocusedActorPart, OverlayMarker, PauseMenuRoot,
        SpriteActor,
    },
    script::CharacterEase,
    ui::ScreenUiRoot,
};
use bevy::{
    camera::{RenderTarget, ScalingMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CAMERA_SCOPES: [CameraEffectScope; 4] = [
    CameraEffectScope::Background,
    CameraEffectScope::World,
    CameraEffectScope::Ui,
    CameraEffectScope::Canvas,
];

#[cfg(test)]
#[path = "camera_tests.rs"]
mod virtual_camera_tests;

/// Static background artwork and background-only effects such as rain or fog.
pub const BACKGROUND_LAYER: usize = 0;
/// Normal scene actors and props.
pub const SCENE_LAYER: usize = 1;
/// Isolated actors/props that must remain separable for focus effects.
pub const FOCUS_LAYER: usize = 2;
/// Engine UI. This layer is intentionally unaffected by world post-processing.
pub const UI_LAYER: usize = 3;

pub fn background_layer() -> RenderLayers {
    RenderLayers::layer(BACKGROUND_LAYER)
}

pub fn scene_layer() -> RenderLayers {
    RenderLayers::layer(SCENE_LAYER)
}

pub fn focus_layer() -> RenderLayers {
    RenderLayers::layer(FOCUS_LAYER)
}

pub fn ui_layer() -> RenderLayers {
    RenderLayers::layer(UI_LAYER)
}

/// Cameras transformed together by story-level zoom, pan and shake.
#[derive(Component)]
pub struct WorldCamera;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World3dLayer {
    Background,
    Scene,
    Focus,
}

/// Marks a 3D mesh for one of Hiraku's semantic render phases.
#[derive(Component, Clone, Copy, Debug)]
pub struct World3dObject {
    pub layer: World3dLayer,
}

/// The single primary 3D camera owned by Hiraku.
#[derive(Component)]
pub struct WorldCamera3d {
    orthographic_height: f32,
}

#[derive(Resource, Default)]
pub struct CameraShakeState {
    pub active: Option<CameraShake>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraView {
    pub blur_intensity: f32,
    pub zoom: f32,
    pub offset: Vec3,
    pub rotation: Vec3,
    pub projection: CameraProjectionMode,
}

impl Default for CameraView {
    fn default() -> Self {
        Self {
            blur_intensity: 0.0,
            zoom: 1.0,
            offset: Vec3::ZERO,
            rotation: Vec3::ZERO,
            projection: CameraProjectionMode::Orthographic,
        }
    }
}

impl CameraView {
    pub(crate) fn transform_picture(&self, mut transform: Transform) -> Transform {
        let depth = transform.translation.z;
        let rotation = Quat::from_rotation_z(-self.rotation.z.to_radians());
        transform.translation = rotation * (transform.translation - self.offset) * self.zoom;
        transform.translation.z = depth;
        transform.rotation = rotation * transform.rotation;
        transform.scale.x *= self.zoom;
        transform.scale.y *= self.zoom;
        transform
    }
    /// Inverse presentation transform, shared with virtual-pointer picking.
    fn source_uv(&self, uv: Vec2, size: Vec2) -> Vec2 {
        let p = (uv - Vec2::splat(0.5)) * size / self.zoom.max(0.01);
        let (s, c) = self.rotation.z.to_radians().sin_cos();
        (Vec2::new(c * p.x + s * p.y, -s * p.x + c * p.y)
            + Vec2::new(self.offset.x, -self.offset.y))
            / size
            + Vec2::splat(0.5)
    }
}

/// Independent virtual views sharing one physical presentation camera.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct CameraState {
    #[serde(with = "view_entries")]
    views: BTreeMap<CameraEffectScope, CameraView>,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            views: CAMERA_SCOPES
                .into_iter()
                .map(|scope| (scope, CameraView::default()))
                .collect(),
        }
    }
}

impl CameraState {
    pub(crate) fn ui_source_uv(&self, uv: Vec2, size: Vec2) -> Vec2 {
        let scene_uv = self.view(CameraEffectScope::Canvas).source_uv(uv, size);
        if scene_uv.min_element() < 0.0 || scene_uv.max_element() > 1.0 {
            return Vec2::splat(-1.0);
        }
        self.view(CameraEffectScope::Ui).source_uv(scene_uv, size)
    }
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if CAMERA_SCOPES
            .iter()
            .any(|scope| !self.views.contains_key(scope))
        {
            return Err("virtual camera snapshot is missing a view");
        }
        if self.views.values().any(|view| {
            !view.zoom.is_finite()
                || view.zoom <= 0.0
                || !view.blur_intensity.is_finite()
                || view.blur_intensity < 0.0
                || !view.offset.is_finite()
                || !view.rotation.is_finite()
        }) {
            return Err("invalid virtual camera pose");
        }
        Ok(())
    }
    pub fn view(&self, scope: CameraEffectScope) -> &CameraView {
        self.views
            .get(&scope)
            .expect("every camera scope has a view")
    }

    fn view_mut(&mut self, scope: CameraEffectScope) -> &mut CameraView {
        self.views
            .get_mut(&scope)
            .expect("every camera scope has a view")
    }
}

mod camera_timer {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        timer: &Timer,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        (timer.duration(), timer.elapsed()).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Timer, D::Error> {
        let (duration, elapsed) =
            <(std::time::Duration, std::time::Duration)>::deserialize(deserializer)?;
        if elapsed > duration {
            return Err(serde::de::Error::custom(
                "camera elapsed time exceeds its duration",
            ));
        }
        let mut timer = Timer::new(duration, TimerMode::Once);
        timer.tick(elapsed);
        Ok(timer)
    }
}

#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct CameraTweenState {
    #[serde(with = "view_entries")]
    pub views: BTreeMap<CameraEffectScope, CameraTimeline>,
}

impl CameraTweenState {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        for timeline in self.views.values() {
            let Some(tween) = &timeline.active else {
                continue;
            };
            for scalar in [tween.blur.as_ref(), tween.zoom.as_ref()]
                .into_iter()
                .flatten()
            {
                if !scalar.from.is_finite() || !scalar.to.is_finite() || !valid_ease(scalar.ease) {
                    return Err("invalid virtual camera scalar tween");
                }
            }
            if tween
                .zoom
                .as_ref()
                .is_some_and(|v| v.from <= 0.0 || v.to <= 0.0)
                || tween
                    .blur
                    .as_ref()
                    .is_some_and(|v| v.from < 0.0 || v.to < 0.0)
            {
                return Err("invalid virtual camera zoom or blur range");
            }
            for vector in [tween.offset.as_ref(), tween.rotation.as_ref()]
                .into_iter()
                .flatten()
            {
                if !vector.from.is_finite() || !vector.to.is_finite() || !valid_ease(vector.ease) {
                    return Err("invalid virtual camera vector tween");
                }
            }
        }
        Ok(())
    }
}

fn valid_ease(ease: CharacterEase) -> bool {
    match ease {
        CharacterEase::CubicBezier(x1, y1, x2, y2) => {
            [x1, y1, x2, y2].into_iter().all(f64::is_finite)
                && (0.0..=1.0).contains(&x1)
                && (0.0..=1.0).contains(&x2)
        }
        _ => true,
    }
}

mod view_entries {
    use super::*;
    pub fn serialize<T: Serialize, S: serde::Serializer>(
        values: &BTreeMap<CameraEffectScope, T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        values.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<CameraEffectScope, T>, D::Error> {
        let entries = Vec::<(CameraEffectScope, T)>::deserialize(deserializer)?;
        let mut result = BTreeMap::new();
        for (scope, value) in entries {
            if result.insert(scope, value).is_some() {
                return Err(serde::de::Error::custom("duplicate virtual camera scope"));
            }
        }
        Ok(result)
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct CameraTimeline {
    pub active: Option<CameraTween>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraTween {
    pub zoom_view_space: bool,
    pub blur: Option<CameraScalarTween>,
    pub zoom: Option<CameraScalarTween>,
    pub offset: Option<CameraVectorTween>,
    pub rotation: Option<CameraVectorTween>,
    pub completions: Vec<CameraTweenCompletion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraTweenCompletion {
    pub blur: bool,
    pub zoom: bool,
    pub offset: bool,
    pub rotation: bool,
    pub animation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraScalarTween {
    pub from: f32,
    pub to: f32,
    #[serde(with = "camera_timer")]
    pub timer: Timer,
    pub ease: CharacterEase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraVectorTween {
    pub from: Vec3,
    pub to: Vec3,
    #[serde(with = "camera_timer")]
    pub timer: Timer,
    pub ease: CharacterEase,
}

pub struct CameraShake {
    pub timer: Timer,
    pub amplitude: Vec2,
    pub interval: f32,
    pub seed: u64,
    pub animation_id: Option<String>,
}

impl CameraShake {
    fn displacement(&self) -> Vec2 {
        let elapsed = self.timer.elapsed_secs();
        if self.timer.is_finished() || elapsed < self.interval {
            return Vec2::ZERO;
        }
        let step = (elapsed / self.interval).floor() as u64;
        // Counter-based noise: independent of frame rate and never consumes the
        // story's random stream. Sampling is stable throughout one interval.
        let sample = |axis: u64| {
            let mut bits = self
                .seed
                .wrapping_add(step.wrapping_mul(0x9e3779b97f4a7c15))
                .wrapping_add(axis);
            bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d049bb133111eb);
            bits ^= bits >> 31;
            (bits >> 40) as f32 / 16777216.0 * 2.0 - 1.0
        };
        Vec2::new(sample(0), sample(1)) * self.amplitude
    }
}

pub fn setup_stage_cameras(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    config: &RuntimeLaunchConfig,
) {
    let canvas_size = config.canvas_size.max(UVec2::ONE);
    let canvas_image = images.add(Image::new_target_texture(
        canvas_size.x,
        canvas_size.y,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));
    commands.insert_resource(HirakuCanvas {
        image: canvas_image.clone(),
        size: canvas_size,
    });
    commands.insert_resource(crate::HirakuInputTarget(canvas_image.clone()));

    let mut projection = OrthographicProjection::default_3d();
    projection.scaling_mode = ScalingMode::FixedVertical {
        viewport_height: canvas_size.y as f32,
    };
    projection.near = -2000.0;
    projection.far = 2000.0;
    commands.spawn((
        Camera3d::default(),
        IsDefaultUiCamera,
        Projection::Orthographic(projection),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        PostProcessSettings::default(),
        Camera {
            order: config.camera_order,
            clear_color: config.camera_clear_color.clone(),
            ..default()
        },
        RenderTarget::Image(canvas_image.into()),
        RenderLayers::from_layers(&[BACKGROUND_LAYER, SCENE_LAYER, FOCUS_LAYER]),
        WorldCamera,
        WorldCamera3d {
            orthographic_height: canvas_size.y as f32,
        },
    ));
}

/// Assigns engine visual entities to their semantic render layer.
///
/// Centralizing this rule keeps command handlers and snapshot restoration from
/// having to remember camera implementation details on every spawn path.
pub fn assign_render_layers(
    mut commands: Commands,
    backgrounds: Query<
        Entity,
        Or<(
            Added<BackgroundLayer>,
            Added<RuleTransitionPlayer>,
            Added<CustomScreenEffectPlayer>,
        )>,
    >,
    actors: Query<(Entity, Option<&FocusedActorPart>), Added<SpriteActor>>,
    world_3d_objects: Query<(Entity, &World3dObject), Added<World3dObject>>,
    overlays: Query<Entity, Added<OverlayMarker>>,
    ui_roots: Query<
        Entity,
        Or<(
            Added<DialogueRoot>,
            Added<ChoiceUi>,
            Added<PauseMenuRoot>,
            Added<ScreenUiRoot>,
        )>,
    >,
) {
    for entity in &backgrounds {
        commands.entity(entity).try_insert(background_layer());
    }
    for (entity, focused) in &actors {
        commands.entity(entity).try_insert(if focused.is_some() {
            focus_layer()
        } else {
            scene_layer()
        });
    }
    for (entity, object) in &world_3d_objects {
        let layer = match object.layer {
            World3dLayer::Background => background_layer(),
            World3dLayer::Scene => scene_layer(),
            World3dLayer::Focus => focus_layer(),
        };
        commands.entity(entity).try_insert(layer);
    }
    for entity in &overlays {
        commands.entity(entity).try_insert(focus_layer());
    }
    for entity in &ui_roots {
        commands.entity(entity).try_insert(ui_layer());
    }
}

pub(crate) fn animate_camera_shake(
    mut redraw: crate::redraw::Redraw,
    time: crate::scene::playback::StoryTime,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    camera_state: Res<CameraState>,
    mut cameras: Query<&mut Transform, With<WorldCamera>>,
) {
    let camera_state = camera_state.view(CameraEffectScope::World);
    let Some(shake) = shake_state.active.as_mut() else {
        for mut camera in &mut cameras {
            camera.translation.x = camera_state.offset.x;
            camera.translation.y = camera_state.offset.y;
        }
        return;
    };

    redraw.request();
    shake.timer.tick(time.delta());
    let displacement = shake.displacement();
    for mut camera in &mut cameras {
        camera.translation.x = camera_state.offset.x + displacement.x;
        camera.translation.y = camera_state.offset.y + displacement.y;
    }

    if shake.timer.is_finished() {
        for mut camera in &mut cameras {
            camera.translation.x = camera_state.offset.x;
            camera.translation.y = camera_state.offset.y;
        }
        if let Some(animation_id) = shake.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        shake_state.active = None;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_camera_tween(
    camera: &mut CameraState,
    tweens: &mut CameraTweenState,
    blur_intensity: Option<f32>,
    zoom: Option<f32>,
    zoom_view_space: bool,
    offset: Option<Vec3>,
    rotation: Option<Vec3>,
    projection: Option<CameraProjectionMode>,
    scope: CameraEffectScope,
    duration: std::time::Duration,
    ease: CharacterEase,
    animation_id: Option<String>,
    animations: &mut AnimationState,
) {
    let camera = camera.view_mut(scope);
    let tweens = tweens.views.entry(scope).or_default();
    if let Some(projection) = projection {
        camera.projection = projection;
    }
    if duration.is_zero() {
        if let Some(tween) = tweens.active.as_mut() {
            cancel_camera_completions(
                tween,
                blur_intensity.is_some(),
                zoom.is_some(),
                offset.is_some(),
                rotation.is_some(),
                animations,
            );
            if blur_intensity.is_some() {
                tween.blur = None;
            }
            if zoom.is_some() {
                tween.zoom = None;
            }
            if offset.is_some() {
                tween.offset = None;
            }
            if rotation.is_some() {
                tween.rotation = None;
            }
        }
        if let Some(blur_intensity) = blur_intensity {
            camera.blur_intensity = blur_intensity;
        }
        if let Some(zoom) = zoom {
            camera.zoom = zoom;
        }
        if let Some(offset) = offset {
            camera.offset = offset;
        }
        if let Some(rotation) = rotation {
            camera.rotation = rotation;
        }
        complete_missing_animation(animations, animation_id);
        return;
    }

    let tween = tweens.active.get_or_insert_with(|| CameraTween {
        zoom_view_space: false,
        blur: None,
        zoom: None,
        offset: None,
        rotation: None,
        completions: Vec::new(),
    });
    cancel_camera_completions(
        tween,
        blur_intensity.is_some(),
        zoom.is_some(),
        offset.is_some(),
        rotation.is_some(),
        animations,
    );
    if let Some(to) = blur_intensity {
        tween.blur = Some(CameraScalarTween {
            from: camera.blur_intensity,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = zoom {
        tween.zoom_view_space = zoom_view_space;
        tween.zoom = Some(CameraScalarTween {
            from: camera.zoom,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = offset {
        tween.offset = Some(CameraVectorTween {
            from: camera.offset,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    if let Some(to) = rotation {
        tween.rotation = Some(CameraVectorTween {
            from: camera.rotation,
            to,
            timer: Timer::new(duration, TimerMode::Once),
            ease,
        });
    }
    tween.completions.push(CameraTweenCompletion {
        blur: blur_intensity.is_some(),
        zoom: zoom.is_some(),
        offset: offset.is_some(),
        rotation: rotation.is_some(),
        animation_id,
    });
}

fn cancel_camera_completions(
    tween: &mut CameraTween,
    blur: bool,
    zoom: bool,
    offset: bool,
    rotation: bool,
    animations: &mut AnimationState,
) {
    let mut retained = Vec::new();
    for completion in tween.completions.drain(..) {
        if (blur && completion.blur)
            || (zoom && completion.zoom)
            || (offset && completion.offset)
            || (rotation && completion.rotation)
        {
            complete_missing_animation(animations, completion.animation_id);
        } else {
            retained.push(completion);
        }
    }
    tween.completions = retained;
}

fn interpolate_zoom(from: f32, to: f32, progress: f32, view_space: bool) -> f32 {
    if view_space {
        from.recip().lerp(to.recip(), progress).recip()
    } else {
        from.lerp(to, progress)
    }
}

pub(crate) fn animate_camera_transition(
    mut shared: Option<ResMut<crate::state::SceneSharedState>>,
    canvas: Option<Res<HirakuCanvas>>,
    mut redraw: crate::redraw::Redraw,
    time: crate::scene::playback::StoryTime,
    mut animations: ResMut<AnimationState>,
    mut camera_state: ResMut<CameraState>,
    mut tweens: ResMut<CameraTweenState>,
    mut applied_state: Local<Option<CameraView>>,
    mut world_cameras: Query<
        (
            &WorldCamera3d,
            &mut Projection,
            &mut Transform,
            &mut PostProcessSettings,
        ),
        With<WorldCamera>,
    >,
) {
    let mut changed = camera_state.is_changed() || tweens.is_changed();
    for scope in CAMERA_SCOPES {
        if !tweens
            .views
            .get(&scope)
            .is_some_and(|timeline| timeline.active.is_some())
        {
            continue;
        }
        changed = true;
        let timeline = tweens
            .views
            .get_mut(&scope)
            .expect("active timeline exists");
        redraw.request();
        tick_camera_view(
            camera_state.view_mut(scope),
            timeline,
            time.delta(),
            &mut animations,
        );
    }

    if changed && let Some(shared) = shared.as_mut() {
        shared.0.camera = crate::state::CameraSnapshot {
            views: camera_state.clone(),
            timelines: tweens.clone(),
        };
    }

    // Each view contributes only to its own composition boundary. Changing a
    // UI/canvas view must never switch off an ongoing scene lens animation.
    for (_, _, _, mut effects) in &mut world_cameras {
        let mut settings = shared
            .as_ref()
            .map_or_else(PostProcessSettings::default, |s| s.0.post_process);
        for scope in CAMERA_SCOPES {
            let view = camera_state.view(scope);
            let layer = settings.layer_mut(scope);
            layer.blur_radius += 2.0 * view.blur_intensity.max(0.0);
            if matches!(scope, CameraEffectScope::Ui | CameraEffectScope::Canvas) {
                layer.zoom *= view.zoom.max(0.01);
                let size = canvas
                    .as_ref()
                    .map_or(Vec2::new(1920.0, 1080.0), |c| c.size.as_vec2())
                    .max(Vec2::ONE);
                if view.offset != Vec3::ZERO || view.rotation != Vec3::ZERO || view.zoom != 1.0 {
                    layer.view_transform = Vec4::new(
                        view.offset.x / size.x,
                        -view.offset.y / size.y,
                        view.rotation.z.to_radians(),
                        size.x / size.y,
                    );
                }
            }
        }
        effects.set_if_neq(settings);
    }
    let camera_state = camera_state.view(CameraEffectScope::World);
    apply_scene_camera(
        camera_state,
        shared.as_deref(),
        &mut applied_state,
        &mut world_cameras,
    );
}

fn tick_camera_view(
    camera_state: &mut CameraView,
    tweens: &mut CameraTimeline,
    delta: std::time::Duration,
    animations: &mut AnimationState,
) {
    let mut completed = Vec::new();
    if let Some(tween) = tweens.active.as_mut() {
        if let Some(blur_tween) = tween.blur.as_mut() {
            blur_tween.timer.tick(delta);
            camera_state.blur_intensity = blur_tween.from.lerp(
                blur_tween.to,
                apply_character_ease(blur_tween.ease, tween_fraction(&blur_tween.timer)),
            );
        }
        if let Some(zoom_tween) = tween.zoom.as_mut() {
            zoom_tween.timer.tick(delta);
            camera_state.zoom = interpolate_zoom(
                zoom_tween.from,
                zoom_tween.to,
                apply_character_ease(zoom_tween.ease, tween_fraction(&zoom_tween.timer)),
                tween.zoom_view_space,
            );
        }
        if let Some(offset_tween) = tween.offset.as_mut() {
            offset_tween.timer.tick(delta);
            camera_state.offset = offset_tween.from.lerp(
                offset_tween.to,
                apply_character_ease(offset_tween.ease, tween_fraction(&offset_tween.timer)),
            );
        }
        if let Some(rotation_tween) = tween.rotation.as_mut() {
            rotation_tween.timer.tick(delta);
            camera_state.rotation = rotation_tween.from.lerp(
                rotation_tween.to,
                apply_character_ease(rotation_tween.ease, tween_fraction(&rotation_tween.timer)),
            );
        }

        let blur_finished = tween
            .blur
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let zoom_finished = tween
            .zoom
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let offset_finished = tween
            .offset
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let rotation_finished = tween
            .rotation
            .as_ref()
            .is_none_or(|tween| tween.timer.is_finished());
        let mut pending = Vec::new();
        for completion in tween.completions.drain(..) {
            if (!completion.blur || blur_finished)
                && (!completion.zoom || zoom_finished)
                && (!completion.offset || offset_finished)
                && (!completion.rotation || rotation_finished)
            {
                completed.push(completion);
            } else {
                pending.push(completion);
            }
        }
        tween.completions = pending;
    }
    for completion in completed {
        complete_missing_animation(animations, completion.animation_id);
    }
    if tweens
        .active
        .as_ref()
        .is_some_and(|tween| tween.completions.is_empty())
    {
        tweens.active = None;
    }
}

fn apply_scene_camera(
    camera_state: &CameraView,
    shared: Option<&crate::state::SceneSharedState>,
    applied_state: &mut Option<CameraView>,
    world_cameras: &mut Query<
        (
            &WorldCamera3d,
            &mut Projection,
            &mut Transform,
            &mut PostProcessSettings,
        ),
        With<WorldCamera>,
    >,
) {
    if shared
        .as_ref()
        .is_some_and(|shared| shared.0.spatial_stage.is_some())
    {
        *applied_state = None;
        return;
    }
    if applied_state.as_ref() == Some(&*camera_state) {
        return;
    }
    *applied_state = Some(camera_state.clone());

    for (camera, mut projection, mut transform, _) in world_cameras.iter_mut() {
        // Only the scene view drives the physical lens. UI/canvas views are
        // composed later and never mutate its transform or projection.
        let zoom = camera_state.zoom.max(0.01);
        match camera_state.projection {
            CameraProjectionMode::Orthographic => {
                if !matches!(*projection, Projection::Orthographic(_)) {
                    let mut orthographic = OrthographicProjection::default_3d();
                    orthographic.scaling_mode = ScalingMode::FixedVertical {
                        viewport_height: camera.orthographic_height,
                    };
                    orthographic.near = -2000.0;
                    orthographic.far = 2000.0;
                    *projection = Projection::Orthographic(orthographic);
                }
                if let Projection::Orthographic(orthographic) = projection.as_mut() {
                    orthographic.scale = 1.0 / zoom;
                }
            }
            CameraProjectionMode::Perspective => {
                if !matches!(*projection, Projection::Perspective(_)) {
                    *projection = Projection::Perspective(PerspectiveProjection::default());
                }
                if let Projection::Perspective(perspective) = projection.as_mut() {
                    perspective.fov = (60.0_f32.to_radians() / zoom)
                        .clamp(5.0_f32.to_radians(), 170.0_f32.to_radians());
                }
            }
        }
        transform.translation = Vec3::new(
            camera_state.offset.x,
            camera_state.offset.y,
            1000.0 + camera_state.offset.z,
        );
        let rotation = camera_state.rotation;
        transform.rotation = Quat::from_euler(
            EulerRot::XYZ,
            rotation.x.to_radians(),
            rotation.y.to_radians(),
            rotation.z.to_radians(),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stepped_shake_is_frame_rate_independent_and_returns_to_origin() {
        let mut shake = super::CameraShake {
            timer: Timer::from_seconds(0.4, TimerMode::Once),
            amplitude: Vec2::new(15.0, 15.0),
            interval: 0.05,
            seed: 42,
            animation_id: None,
        };
        assert_eq!(shake.displacement(), Vec2::ZERO);
        shake.timer.tick(std::time::Duration::from_millis(60));
        let a = shake.displacement();
        shake.timer.tick(std::time::Duration::from_millis(20));
        assert_eq!(a, shake.displacement());
        assert!(a.abs().cmple(shake.amplitude).all());
        shake.timer.tick(std::time::Duration::from_millis(30));
        assert_ne!(a, shake.displacement());
        shake.timer.tick(std::time::Duration::from_secs(1));
        assert_eq!(shake.displacement(), Vec2::ZERO);
    }
    #[test]
    fn view_scale_interpolates_visible_extent_not_magnification() {
        assert!((super::interpolate_zoom(1.0, 2.5, 0.5, true) - 1.0 / 0.7).abs() < 0.00001);
        assert_eq!(super::interpolate_zoom(1.0, 2.5, 0.5, false), 1.75);
        assert!((super::interpolate_zoom(1.0, 2.5, 1.0, true) - 2.5).abs() < 0.00001);
    }

    use super::*;

    #[test]
    fn render_layer_ids_follow_composition_order() {
        assert_eq!(
            [BACKGROUND_LAYER, SCENE_LAYER, FOCUS_LAYER, UI_LAYER],
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn semantic_entities_receive_their_render_layers() {
        let mut app = App::new();
        app.add_systems(Update, assign_render_layers);
        let background = app
            .world_mut()
            .spawn(BackgroundLayer {
                path: "background.webp".to_string(),
            })
            .id();
        let actor = app
            .world_mut()
            .spawn(SpriteActor {
                id: "alice".to_string(),
                path: "alice.webp".to_string(),
            })
            .id();
        let focused_actor = app
            .world_mut()
            .spawn((
                SpriteActor {
                    id: "bob".to_string(),
                    path: "bob.webp".to_string(),
                },
                FocusedActorPart,
            ))
            .id();
        let overlay = app.world_mut().spawn(OverlayMarker).id();

        app.update();

        let world = app.world();
        assert!(
            world
                .get::<RenderLayers>(background)
                .is_some_and(|layers| layers.intersects(&background_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(focused_actor)
                .is_some_and(|layers| layers.intersects(&focus_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(actor)
                .is_some_and(|layers| layers.intersects(&scene_layer()))
        );
        assert!(
            world
                .get::<RenderLayers>(overlay)
                .is_some_and(|layers| layers.intersects(&focus_layer()))
        );
    }

    #[test]
    fn render_layer_assignment_tolerates_same_frame_despawn() {
        fn despawn_added_actors(mut commands: Commands, actors: Query<Entity, Added<SpriteActor>>) {
            for entity in &actors {
                commands.entity(entity).try_despawn();
            }
        }

        let mut app = App::new();
        app.add_systems(
            Update,
            (despawn_added_actors, assign_render_layers).chain_ignore_deferred(),
        );
        let actor = app
            .world_mut()
            .spawn(SpriteActor {
                id: "alice".to_string(),
                path: "alice.webp".to_string(),
            })
            .id();

        app.update();

        assert!(app.world().get_entity(actor).is_err());
    }
}
