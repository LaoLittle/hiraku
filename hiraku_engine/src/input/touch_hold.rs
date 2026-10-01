use super::*;
use crate::script::animation::Easing;
use std::collections::HashSet;

const DELAY: f64 = 0.15;
const FILL: f64 = 0.3;
const MOTION_SLOP: f32 = 0.008;
const IMAGE_SIZE: usize = 96;
const CLOSE_CURVE: Easing = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
const BREATH_CURVE: Easing = Easing::CubicBezier(0.42, 0.0, 0.58, 1.0);
const GLOW_CURVE: Easing = Easing::CubicBezier(0.22, 0.0, 0.36, 1.0);
const GLOW_FADE: f64 = 0.25;

struct Hold {
    pointer: HirakuPointerId,
    uv: Vec2,
    started: f64,
    active: bool,
}

impl Hold {
    fn progress(&self, now: f64) -> Option<f32> {
        let elapsed = now - self.started;
        (elapsed >= DELAY).then(|| CLOSE_CURVE.sample(((elapsed - DELAY) / FILL) as f32))
    }
}

#[derive(Resource, Default)]
pub(crate) struct TouchHold {
    hold: Option<Hold>,
    pressed: HashSet<HirakuPointerId>,
    swallowed: HashSet<HirakuPointerId>,
    visual: Option<(Vec2, f32, bool)>,
}

impl TouchHold {
    pub(super) fn consumed(&self, pointer: HirakuPointerId) -> bool {
        self.swallowed.contains(&pointer)
    }

    fn cancel(&mut self, actions: &mut MessageWriter<HirakuActionInput>) {
        if self.hold.take().is_some_and(|hold| hold.active) {
            actions.write(HirakuActionInput(HirakuAction::FastForwardHeld(false)));
        }
        self.visual = None;
    }
}

pub(crate) fn update(
    mut state: ResMut<TouchHold>,
    time: Res<Time<Real>>,
    canvas: Option<Res<HirakuCanvas>>,
    target: Option<Res<HirakuInputTarget>>,
    mut input: MessageReader<HirakuPointerInput>,
    mut actions: MessageWriter<HirakuActionInput>,
    mut picking: MessageWriter<PointerInput>,
    hover: Option<Res<bevy::picking::hover::HoverMap>>,
    parents: Query<&ChildOf>,
    controls: Query<(
        &Node,
        Has<bevy::ui_widgets::Button>,
        Has<crate::scene::widgets::InputControl>,
    )>,
    screens: Res<crate::ui::ScreenUiState>,
    choice: Res<crate::scene::ChoiceState>,
    movies: Res<crate::scene::PendingMovieWaits>,
    dependencies: Res<crate::dependencies::ScriptDependencies>,
    focus: Res<HirakuTextFocus>,
    mut redraw: crate::redraw::Redraw,
) {
    let now = time.elapsed_secs_f64();
    let blocked = screens.active_root.is_some()
        || screens.pending_root.is_some()
        || choice.waiting.is_some()
        || movies.is_waiting()
        || dependencies.loading
        || focus.0.is_some();
    let pressed = state.pressed.clone();
    state.swallowed.retain(|pointer| pressed.contains(pointer));
    for sample in input.read() {
        if !matches!(sample.pointer, HirakuPointerId::Touch(_)) {
            continue;
        }
        match sample.phase {
            HirakuPointerPhase::Press => {
                state.pressed.insert(sample.pointer);
                state.cancel(&mut actions);
                if state.pressed.len() == 1
                    && !blocked
                    && sample.uv.is_finite()
                    && sample.uv.cmpge(Vec2::ZERO).all()
                    && sample.uv.cmple(Vec2::ONE).all()
                {
                    state.hold = Some(Hold {
                        pointer: sample.pointer,
                        uv: sample.uv,
                        started: now,
                        active: false,
                    });
                }
            }
            HirakuPointerPhase::Move => {
                if state.hold.as_ref().is_some_and(|hold| {
                    hold.pointer == sample.pointer
                        && (!sample.uv.is_finite() || sample.uv.distance(hold.uv) > MOTION_SLOP)
                }) {
                    state.cancel(&mut actions);
                }
            }
            HirakuPointerPhase::Release | HirakuPointerPhase::Cancel => {
                state.pressed.remove(&sample.pointer);
                if state
                    .hold
                    .as_ref()
                    .is_some_and(|hold| hold.pointer == sample.pointer)
                {
                    state.cancel(&mut actions);
                }
            }
        }
    }
    if blocked {
        state.cancel(&mut actions);
    }
    let Some(hold) = state.hold.as_ref() else {
        return;
    };
    let blocks_hold = |entity| {
        controls.get(entity).is_ok_and(|(node, button, input)| {
            button
                || input
                || node.overflow.x == OverflowAxis::Scroll
                || node.overflow.y == OverflowAxis::Scroll
        })
    };
    let control_hit = now > hold.started
        && hover
            .as_ref()
            .and_then(|hover| hover.0.get(&hold.pointer.picking_id()))
            .is_some_and(|hits| {
                hits.keys().any(|entity| {
                    blocks_hold(*entity) || parents.iter_ancestors(*entity).any(blocks_hold)
                })
            });
    if control_hit {
        state.cancel(&mut actions);
        redraw.request();
        return;
    }
    let (Some(canvas), Some(target)) = (canvas, target) else {
        state.cancel(&mut actions);
        return;
    };
    let progress = hold.progress(now);
    if now - hold.started >= DELAY + FILL && !hold.active {
        let pointer = hold.pointer;
        let uv = hold.uv;
        let Some(target) = RenderTarget::Image(target.0.clone().into()).normalize(None) else {
            return;
        };
        picking.write(PointerInput::new(
            pointer.picking_id(),
            Location {
                target,
                position: uv * canvas.size.as_vec2(),
            },
            PointerAction::Cancel,
        ));
        state.swallowed.insert(pointer);
        state.hold.as_mut().expect("hold exists").active = true;
        actions.write(HirakuActionInput(HirakuAction::FastForwardHeld(true)));
    }
    let hold = state.hold.as_ref().expect("hold exists");
    state.visual = progress.map(|progress| (hold.uv, progress, hold.active));
    redraw.request();
}

#[derive(Resource, Default)]
pub(crate) struct HoldVisual {
    entity: Option<Entity>,
    image: Option<Handle<Image>>,
    painted: Option<(f32, bool, f32)>,
}

fn paint(progress: f32, active: bool, glow: f32) -> Vec<u8> {
    let mut pixels = vec![0; IMAGE_SIZE * IMAGE_SIZE * 4];
    for y in 0..IMAGE_SIZE {
        for x in 0..IMAGE_SIZE {
            let p =
                Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - Vec2::splat(IMAGE_SIZE as f32 / 2.0);
            let radius = p.length();
            let angle = p.x.atan2(-p.y).rem_euclid(std::f32::consts::TAU);
            let ring = (2.5 - (radius - 38.0).abs()).clamp(0.0, 1.0);
            let distance = (radius - 38.0).abs();
            let halo = if active {
                0.24 * glow
                    * (-0.5 * (distance / 3.6).powi(2)).exp()
                    * ((8.0 - distance) / 2.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let icon = active
                && [-14.0, 1.0].into_iter().any(|left| {
                    let local = p.x - left;
                    (0.0..=13.0).contains(&local) && p.y.abs() <= (13.0 - local) * 0.8
                });
            let alpha = if icon {
                1.0
            } else {
                ring * if angle <= progress * std::f32::consts::TAU {
                    1.0
                } else {
                    0.2
                }
            };
            let alpha = 1.0 - (1.0 - alpha) * (1.0 - halo);
            let offset = (y * IMAGE_SIZE + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, (alpha * 255.0) as u8]);
        }
    }
    pixels
}

fn pulse_scale(elapsed: f64) -> f32 {
    let phase = elapsed.rem_euclid(1.8) / 0.9;
    let progress = if phase <= 1.0 { phase } else { 2.0 - phase };
    1.0 - 0.12 * BREATH_CURVE.sample(progress as f32)
}

fn glow_strength(elapsed: f64) -> f32 {
    GLOW_CURVE.sample((elapsed / GLOW_FADE) as f32)
}

pub(crate) fn render(
    mut commands: Commands,
    state: Res<TouchHold>,
    time: Res<Time<Real>>,
    canvas: Option<Res<HirakuCanvas>>,
    cameras: Query<Entity, With<crate::render::camera::WorldCamera>>,
    mut visual: ResMut<HoldVisual>,
    mut images: ResMut<Assets<Image>>,
    mut nodes: Query<(&mut Node, &mut Visibility, &mut UiTransform)>,
    mut redraw: crate::redraw::Redraw,
) {
    let (Some(canvas), Ok(camera)) = (canvas, cameras.single()) else {
        return;
    };
    let Some((uv, progress, active)) = state.visual else {
        if let Some(entity) = visual.entity
            && let Ok((_, mut visibility, _)) = nodes.get_mut(entity)
        {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
                redraw.request();
            }
        }
        return;
    };
    let elapsed = state.hold.as_ref().map_or(0.0, |hold| {
        (time.elapsed_secs_f64() - hold.started - DELAY - FILL).max(0.0)
    });
    let scale = if active {
        redraw.request();
        pulse_scale(elapsed)
    } else {
        1.0
    };
    let size = (canvas.size.y as f32 * 0.085).clamp(48.0, 128.0);
    let transform = UiTransform {
        scale: Vec2::splat(scale),
        ..default()
    };
    let glow = if active { glow_strength(elapsed) } else { 0.0 };
    if visual.painted != Some((progress, active, glow)) {
        let pixels = paint(progress, active, glow);
        if let Some(mut image) = visual
            .image
            .as_ref()
            .and_then(|handle| images.get_mut(handle))
        {
            image.data = Some(pixels);
        } else {
            visual.image = Some(images.add(Image::new(
                bevy::render::render_resource::Extent3d {
                    width: IMAGE_SIZE as u32,
                    height: IMAGE_SIZE as u32,
                    depth_or_array_layers: 1,
                },
                bevy::render::render_resource::TextureDimension::D2,
                pixels,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::default(),
            )));
        }
        visual.painted = Some((progress, active, glow));
        redraw.request();
    }
    let node = Node {
        position_type: PositionType::Absolute,
        width: Val::Px(size),
        height: Val::Px(size),
        left: Val::Px(uv.x * canvas.size.x as f32 - size / 2.0),
        top: Val::Px(uv.y * canvas.size.y as f32 - size / 2.0),
        ..default()
    };
    if let Some(entity) = visual.entity
        && let Ok((mut current, mut visibility, mut current_transform)) = nodes.get_mut(entity)
    {
        current.set_if_neq(node);
        current_transform.set_if_neq(transform);
        visibility.set_if_neq(Visibility::Visible);
    } else {
        visual.entity = Some(
            commands
                .spawn((
                    node,
                    transform,
                    ImageNode::new(visual.image.clone().expect("indicator image is allocated")),
                    UiTargetCamera(camera),
                    GlobalZIndex(i32::MAX - 1),
                    Pickable::IGNORE,
                    Visibility::Visible,
                ))
                .id(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<TouchHold>()
            .init_resource::<crate::ui::ScreenUiState>()
            .init_resource::<crate::scene::ChoiceState>()
            .init_resource::<crate::scene::PendingMovieWaits>()
            .init_resource::<crate::dependencies::ScriptDependencies>()
            .init_resource::<HirakuTextFocus>()
            .insert_resource(HirakuCanvas {
                image: Handle::default(),
                size: UVec2::new(1920, 1080),
            })
            .insert_resource(HirakuInputTarget(Handle::default()))
            .add_message::<HirakuPointerInput>()
            .add_message::<HirakuActionInput>()
            .add_message::<PointerInput>()
            .add_systems(Update, update);
        app
    }

    #[test]
    fn touch_systems_wait_for_deferred_scene_initialization() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<TouchHold>()
            .init_resource::<HoldVisual>()
            .init_resource::<Assets<Image>>()
            .init_resource::<crate::scene::PendingMovieWaits>()
            .init_resource::<crate::dependencies::ScriptDependencies>()
            .init_resource::<HirakuTextFocus>()
            .add_message::<HirakuPointerInput>()
            .add_message::<HirakuActionInput>()
            .add_message::<PointerInput>()
            .add_systems(First, update.run_if(crate::runtime_initialized))
            .add_systems(Update, render.run_if(crate::runtime_initialized));

        for _ in 0..3 {
            app.update();
        }
        assert!(!app.world().contains_resource::<crate::ui::ScreenUiState>());
        assert!(!app.world().contains_resource::<crate::scene::ChoiceState>());

        app.add_systems(
            Update,
            (|mut commands: Commands| {
                commands.init_resource::<crate::ui::ScreenUiState>();
                commands.init_resource::<crate::scene::ChoiceState>();
                commands.init_resource::<crate::scene::FrontendState>();
            })
            .run_if(not(resource_exists::<crate::scene::FrontendState>)),
        );
        app.update();
        app.insert_resource(HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(1920, 1080),
        })
        .insert_resource(HirakuInputTarget(Handle::default()));
        sample(
            &mut app,
            HirakuPointerId::Touch(1),
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 451);
        assert!(
            app.world()
                .resource::<TouchHold>()
                .hold
                .as_ref()
                .is_some_and(|hold| hold.active)
        );
    }

    fn sample(app: &mut App, pointer: HirakuPointerId, phase: HirakuPointerPhase, uv: Vec2) {
        app.world_mut()
            .write_message(HirakuPointerInput { pointer, uv, phase });
        app.update();
    }

    fn advance(app: &mut App, millis: u64) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(millis));
        app.update();
    }

    #[test]
    fn indicator_allocates_its_own_transparent_texture_and_reuses_it() {
        let mut app = app();
        app.init_resource::<HoldVisual>()
            .init_resource::<Assets<Image>>()
            .add_message::<bevy::window::RequestRedraw>()
            .add_systems(Update, render.after(update));
        app.world_mut().spawn(crate::render::camera::WorldCamera);
        let white = Image::default();
        let white_data = white.data.clone();
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(AssetId::default(), white)
            .expect("default white image");
        sample(
            &mut app,
            HirakuPointerId::Touch(1),
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 451);
        let visual = app.world().resource::<HoldVisual>();
        let entity = visual.entity.expect("indicator node");
        let handle = visual.image.clone().expect("indicator texture");
        assert_ne!(handle.id(), AssetId::default());
        let images = app.world().resource::<Assets<Image>>();
        assert_eq!(
            images.get(AssetId::default()).expect("white image").data,
            white_data
        );
        let image = images.get(&handle).expect("ring texture");
        assert_eq!(image.width(), IMAGE_SIZE as u32);
        assert_eq!(image.height(), IMAGE_SIZE as u32);
        let pixels = image.data.as_ref().expect("rgba");
        assert_eq!(pixels[3], 0, "corners must be transparent");
        assert_eq!(
            pixels[(48 * IMAGE_SIZE + 48) * 4 + 3],
            0,
            "gap between triangles"
        );
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 255));
        let initial_width = app.world().get::<Node>(entity).expect("node").width;
        let initial_scale = app
            .world()
            .get::<UiTransform>(entity)
            .expect("transform")
            .scale
            .x;
        let asset_count = images.len();
        advance(&mut app, 900);
        let Val::Px(initial) = initial_width else {
            panic!("pixel width")
        };
        let node = app.world().get::<Node>(entity).expect("node");
        let smaller = app
            .world()
            .get::<UiTransform>(entity)
            .expect("transform")
            .scale
            .x;
        assert!(smaller < initial_scale);
        assert_eq!(node.width, initial_width);
        assert_eq!(node.left, Val::Px(960.0 - initial / 2.0));
        advance(&mut app, 900);
        let restored = app
            .world()
            .get::<UiTransform>(entity)
            .expect("transform")
            .scale
            .x;
        assert!((restored - initial_scale).abs() < 0.001);
        assert_eq!(app.world().resource::<Assets<Image>>().len(), asset_count);
        assert_eq!(
            app.world()
                .get::<ImageNode>(entity)
                .expect("image node")
                .image,
            handle
        );
        let pickable = app.world().get::<Pickable>(entity).expect("pickable");
        assert!(!pickable.is_hoverable && !pickable.should_block_lower);
    }

    #[test]
    fn touch_activation_cancels_click_and_release_stops_fast_forward() {
        let mut app = app();
        let mut actions = bevy::ecs::message::MessageCursor::<HirakuActionInput>::default();
        let pointer = HirakuPointerId::Touch(1);
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 149);
        assert!(app.world().resource::<TouchHold>().visual.is_none());
        advance(&mut app, 151);
        assert!(
            (app.world().resource::<TouchHold>().visual.expect("ring").1 - CLOSE_CURVE.sample(0.5))
                .abs()
                < 0.001
        );
        assert_eq!(
            actions
                .read(app.world().resource::<Messages<HirakuActionInput>>())
                .count(),
            0
        );
        advance(&mut app, 151);
        assert!(app.world().resource::<TouchHold>().consumed(pointer));
        assert_eq!(
            actions
                .read(app.world().resource::<Messages<HirakuActionInput>>())
                .map(|input| input.0)
                .collect::<Vec<_>>(),
            vec![HirakuAction::FastForwardHeld(true)]
        );
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Release,
            Vec2::splat(0.5),
        );
        assert!(app.world().resource::<TouchHold>().visual.is_none());
        assert!(app.world().resource::<TouchHold>().consumed(pointer));
        assert_eq!(
            actions
                .read(app.world().resource::<Messages<HirakuActionInput>>())
                .map(|input| input.0)
                .collect::<Vec<_>>(),
            vec![HirakuAction::FastForwardHeld(false)]
        );
    }

    #[test]
    fn virtual_pointer_bridge_never_releases_a_consumed_hold_as_a_click() {
        let mut app = app();
        app.init_resource::<bevy::picking::pointer::PointerMap>()
            .add_message::<HirakuScrollInput>()
            .add_message::<PointerScroll>()
            .add_systems(Update, super::super::bridge_virtual_pointers.after(update));
        let pointer = HirakuPointerId::Touch(1);
        let mut picking = bevy::ecs::message::MessageCursor::<PointerInput>::default();
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        picking
            .read(app.world().resource::<Messages<PointerInput>>())
            .for_each(drop);
        advance(&mut app, 451);
        assert!(
            picking
                .read(app.world().resource::<Messages<PointerInput>>())
                .any(|event| matches!(event.action, PointerAction::Cancel))
        );
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Release,
            Vec2::splat(0.5),
        );
        let events = picking
            .read(app.world().resource::<Messages<PointerInput>>())
            .collect::<Vec<_>>();
        assert!(!events.is_empty());
        assert!(
            events
                .iter()
                .all(|event| matches!(event.action, PointerAction::Cancel))
        );
    }

    #[test]
    fn mouse_movement_multitouch_and_modals_do_not_activate() {
        let mut app = app();
        sample(
            &mut app,
            HirakuPointerId::Pointer(0),
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 500);
        assert!(app.world().resource::<TouchHold>().hold.is_none());
        let pointer = HirakuPointerId::Touch(1);
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Move,
            Vec2::splat(0.6),
        );
        advance(&mut app, 500);
        assert!(app.world().resource::<TouchHold>().hold.is_none());
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Release,
            Vec2::splat(0.6),
        );
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        sample(
            &mut app,
            HirakuPointerId::Touch(2),
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 500);
        assert!(app.world().resource::<TouchHold>().hold.is_none());
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Release,
            Vec2::splat(0.5),
        );
        sample(
            &mut app,
            HirakuPointerId::Touch(2),
            HirakuPointerPhase::Release,
            Vec2::splat(0.5),
        );
        app.world_mut()
            .resource_mut::<crate::ui::ScreenUiState>()
            .active_root = Some(app.world_mut().spawn_empty().id());
        sample(
            &mut app,
            pointer,
            HirakuPointerPhase::Press,
            Vec2::splat(0.5),
        );
        advance(&mut app, 500);
        assert!(app.world().resource::<TouchHold>().hold.is_none());
    }
    #[test]
    fn silent_delay_and_ring_duration_are_independent_of_story_speed() {
        let hold = Hold {
            pointer: HirakuPointerId::Touch(1),
            uv: Vec2::splat(0.5),
            started: 1.0,
            active: false,
        };
        assert_eq!(hold.progress(1.149), None);
        assert!((hold.progress(1.3).expect("progress") - CLOSE_CURVE.sample(0.5)).abs() < 0.001);
        assert_eq!(hold.progress(1.451), Some(1.0));
    }
    #[test]
    fn bezier_animation_preserves_deadlines_and_smooth_breathing_turns() {
        let hold = Hold {
            pointer: HirakuPointerId::Touch(1),
            uv: Vec2::splat(0.5),
            started: 0.0,
            active: false,
        };
        assert_eq!(hold.progress(DELAY), Some(0.0));
        assert!(hold.progress(DELAY + FILL / 2.0).expect("arc") > 0.7);
        assert!(hold.progress(DELAY + FILL - 0.001).expect("arc") < 1.0);
        assert_eq!(hold.progress(DELAY + FILL), Some(1.0));
        assert_eq!(pulse_scale(0.0), 1.0);
        assert!((pulse_scale(0.9) - 0.88).abs() < 0.0001);
        assert_eq!(pulse_scale(1.8), 1.0);
        for turn in [0.9, 1.8] {
            assert!((pulse_scale(turn - 0.001) - pulse_scale(turn)).abs() < 0.00001);
            assert!((pulse_scale(turn + 0.001) - pulse_scale(turn)).abs() < 0.00001);
        }
        let mut previous = 0.0;
        for frame in 0..=30 {
            let glow = glow_strength(GLOW_FADE * frame as f64 / 30.0);
            assert!(glow >= previous && glow <= 1.0);
            previous = glow;
        }
        assert_eq!(glow_strength(0.0), 0.0);
        assert_eq!(glow_strength(GLOW_FADE), 1.0);
        assert_eq!(glow_strength(10.0), 1.0);
    }

    #[test]
    fn bloom_fades_in_without_a_rectangular_background() {
        let base = paint(1.0, true, 0.0);
        let partial = paint(1.0, true, glow_strength(GLOW_FADE / 2.0));
        let full = paint(1.0, true, 1.0);
        let halo = (48 * IMAGE_SIZE + 91) * 4 + 3;
        assert_eq!(base[halo], 0);
        assert!(partial[halo] > 0 && partial[halo] < full[halo]);
        for corner in [
            0,
            IMAGE_SIZE - 1,
            (IMAGE_SIZE - 1) * IMAGE_SIZE,
            IMAGE_SIZE * IMAGE_SIZE - 1,
        ] {
            assert_eq!(full[corner * 4 + 3], 0);
        }
        assert_eq!(full[(48 * IMAGE_SIZE + 48) * 4 + 3], 0);
        assert_eq!(paint(1.0, false, 1.0)[halo], 0);
    }

    #[test]
    fn fast_forward_indicator_only_appears_after_activation() {
        let center = ((IMAGE_SIZE / 2) * IMAGE_SIZE + IMAGE_SIZE / 2 - 8) * 4 + 3;
        assert_eq!(paint(0.5, false, 0.0)[center], 0);
        assert_eq!(paint(1.0, true, 0.0)[center], 255);
    }

    #[test]
    fn ordinary_canvas_nodes_allow_hold_but_buttons_and_scrollers_do_not() {
        use bevy::picking::{backend::HitData, hover::HoverMap};
        for kind in 0..3 {
            let mut app = app();
            app.init_resource::<HoverMap>();
            let entity = match kind {
                0 => app.world_mut().spawn(Node::default()).id(),
                1 => app
                    .world_mut()
                    .spawn((Node::default(), bevy::ui_widgets::Button))
                    .id(),
                _ => app
                    .world_mut()
                    .spawn(Node {
                        overflow: Overflow::scroll_y(),
                        ..default()
                    })
                    .id(),
            };
            let pointer = HirakuPointerId::Touch(1);
            sample(
                &mut app,
                pointer,
                HirakuPointerPhase::Press,
                Vec2::splat(0.5),
            );
            app.world_mut()
                .resource_mut::<HoverMap>()
                .0
                .entry(pointer.picking_id())
                .or_default()
                .insert(entity, HitData::new(entity, 0.0, None, None));
            advance(&mut app, 451);
            assert_eq!(
                app.world().resource::<TouchHold>().consumed(pointer),
                kind == 0
            );
        }
    }
}
