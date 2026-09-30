//! Mount-owned one-shot UI timers. Covered/pending modals pause, not expire.
use crate::ui::{OverlayUiState, ScreenUiState, UiCallback};
use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct UiTimers(Vec<(Timer, UiCallback)>);

#[derive(Component)]
pub(crate) struct UiTimersPaused(pub bool);

#[derive(Component)]
pub(crate) struct UiUpdate(pub UiCallback);

#[derive(Component)]
pub(crate) struct UiPressCallback(pub UiCallback);

/// Picking owns pointer capture/cancellation. Observe its aggregate press state,
/// not clicks, so dragging out, cancelling a touch, or covering a modal releases
/// a held control. Keep transient state outside the recomposed node specification.
pub(crate) fn pressed_changes(
    screens: Res<ScreenUiState>,
    overlays: Res<OverlayUiState>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<crate::ui::ScreenUiRoot>>,
    buttons: Query<(
        Entity,
        &bevy::picking::hover::PickingInteraction,
        &UiPressCallback,
        Option<&crate::ui::ScreenUiButton>,
        Option<&crate::ui::ScreenUiImageButton>,
    )>,
    mut previous: Local<std::collections::HashMap<Entity, bool>>,
    mut output: MessageWriter<super::widgets::UiCallbackRequest>,
) {
    previous.retain(|entity, _| buttons.contains(*entity));
    for (entity, interaction, callback, button, image_button) in &buttons {
        let mut root = entity;
        while !roots.contains(root) {
            let Ok(parent) = parents.get(root) else { break };
            root = parent.parent();
        }
        if !screens.accepts_input(root, &overlays) {
            continue;
        }
        let pressed = button.is_none_or(|button| button.enabled)
            && image_button.is_none_or(|button| button.enabled)
            && *interaction == bevy::picking::hover::PickingInteraction::Pressed;
        let last = previous.entry(entity).or_insert(false);
        if *last != pressed {
            *last = pressed;
            output.write(super::widgets::UiCallbackRequest {
                entity,
                root,
                callback: callback.0.clone(),
                arguments: vec![hiraku_script::Value::Bool(pressed)],
            });
        }
    }
}

pub(crate) fn update(
    clock: UiClock,
    callbacks: Query<(Entity, &UiUpdate)>,
    mut output: MessageWriter<super::widgets::UiCallbackRequest>,
    mut redraw: crate::redraw::Redraw,
) {
    for (root, callback) in &callbacks {
        if clock.active_root(root) {
            redraw.request();
        }
        let delta = clock.delta(root).as_secs_f64();
        if delta > 0.0 {
            output.write(super::widgets::UiCallbackRequest {
                entity: root,
                root,
                callback: callback.0.clone(),
                arguments: vec![hiraku_script::Value::Number(delta)],
            });
        }
    }
}

/// UI animations and deadlines share the owning screen's pause policy. A modal
/// keeps its own clock running while screens beneath it remain frozen.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UiClock<'w, 's> {
    time: Res<'w, Time>,
    screens: Option<Res<'w, ScreenUiState>>,
    overlays: Option<Res<'w, OverlayUiState>>,
    parents: Query<'w, 's, &'static ChildOf>,
    roots: Query<'w, 's, (), With<crate::ui::ScreenUiRoot>>,
    paused: Query<'w, 's, &'static UiTimersPaused>,
    dependencies: Option<Res<'w, crate::dependencies::ScriptDependencies>>,
    shaders: Option<Res<'w, Assets<Shader>>>,
    images: Option<Res<'w, Assets<Image>>>,
    image_dependencies: Query<'w, 's, &'static crate::render::ui_quad::UiImageAssets>,
    shader_dependencies: Query<'w, 's, &'static crate::render::ui_quad::UiShaderAssets>,
}

impl UiClock<'_, '_> {
    fn active_root(&self, root: Entity) -> bool {
        !self.paused.get(root).is_ok_and(|paused| paused.0)
            && match (&self.screens, &self.overlays) {
                (Some(screens), Some(overlays)) => screens.accepts_input(root, overlays),
                _ => true,
            }
    }

    pub fn delta(&self, mut entity: Entity) -> std::time::Duration {
        let (Some(screens), Some(overlays)) = (&self.screens, &self.overlays) else {
            return self.time.delta();
        };
        if self.dependencies.as_ref().is_some_and(|d| d.loading) {
            return std::time::Duration::ZERO;
        }
        loop {
            if let Ok(deps) = self.image_dependencies.get(entity) {
                if !self
                    .images
                    .as_ref()
                    .is_some_and(|assets| deps.0.iter().all(|h| assets.contains(h)))
                {
                    return std::time::Duration::ZERO;
                }
            }
            if let Ok(deps) = self.shader_dependencies.get(entity) {
                if !self
                    .shaders
                    .as_ref()
                    .is_some_and(|assets| deps.0.iter().all(|h| assets.contains(h)))
                {
                    return std::time::Duration::ZERO;
                }
            }
            if self.roots.contains(entity) || self.paused.contains(entity) {
                return if screens.accepts_input(entity, overlays)
                    && !self.paused.get(entity).is_ok_and(|p| p.0)
                {
                    self.time.delta()
                } else {
                    std::time::Duration::ZERO
                };
            }
            let Ok(parent) = self.parents.get(entity) else {
                return self.time.delta();
            };
            entity = parent.parent();
        }
    }
}

impl UiTimers {
    pub(super) fn new(timers: &[(f32, UiCallback)]) -> Self {
        Self(
            timers
                .iter()
                .map(|(seconds, callback)| {
                    (
                        Timer::from_seconds(*seconds, TimerMode::Once),
                        callback.clone(),
                    )
                })
                .collect(),
        )
    }
}

pub(crate) fn tick(
    time: UiClock,
    screen: Res<ScreenUiState>,
    overlays: Res<OverlayUiState>,
    dependencies: Option<Res<crate::dependencies::ScriptDependencies>>,
    mut timers: Query<(Entity, &mut UiTimers, Option<&UiTimersPaused>)>,
    mut output: MessageWriter<super::widgets::UiCallbackRequest>,
    mut redraw: crate::redraw::Redraw,
) {
    if dependencies.is_some_and(|d| d.loading) {
        return;
    }
    for (root, mut timers, paused) in &mut timers {
        if paused.is_some_and(|paused| paused.0) {
            continue;
        }
        if !screen.accepts_input(root, &overlays) {
            continue;
        }
        timers.0.retain_mut(|(timer, callback)| {
            redraw.request();
            timer.tick(time.delta(root));
            if timer.is_finished() {
                output.write(super::widgets::UiCallbackRequest {
                    entity: root,
                    root,
                    callback: callback.clone(),
                    arguments: Vec::new(),
                });
                false
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callback_fixture() -> (UiCallback, UiCallback) {
        let screen = crate::script::evaluate_ui_component_named_with_args(
            "memory://alice.ui.hks",
            r#"import ui.widgets.*
                canvas { button { text("Alice") }.onPressedChange { pressed: Bool -> } }
                    .onUpdate { delta: Float -> }
            "#,
            crate::script::ui_runtime::UiContext::default(),
            &crate::texture::TextureCatalog::default(),
            &crate::glossary::TermCatalog::default(),
            &[],
        )
        .expect("callback screen");
        let crate::ui::ScreenNode::Button(button) = &screen.children[0] else {
            panic!("button")
        };
        (
            *button.layout.on_pressed_change.clone().expect("press"),
            screen.on_update.expect("update"),
        )
    }

    #[test]
    fn picking_press_release_and_cancel_emit_transitions_not_clicks() {
        use bevy::picking::hover::PickingInteraction;
        let (callback, _) = callback_fixture();
        let mut app = App::new();
        app.init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .add_message::<super::super::widgets::UiCallbackRequest>()
            .add_systems(Update, pressed_changes);
        let root = app.world_mut().spawn(crate::ui::ScreenUiRoot).id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        let button = app
            .world_mut()
            .spawn((
                ChildOf(root),
                PickingInteraction::None,
                UiPressCallback(callback),
            ))
            .id();
        for (interaction, expected) in [
            (PickingInteraction::None, None),
            (PickingInteraction::Pressed, Some(true)),
            (PickingInteraction::Pressed, None),
            (PickingInteraction::None, Some(false)),
            (PickingInteraction::Pressed, Some(true)),
            (PickingInteraction::Hovered, Some(false)),
        ] {
            app.world_mut().entity_mut(button).insert(interaction);
            app.update();
            let messages: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<super::super::widgets::UiCallbackRequest>>()
                .drain()
                .collect();
            assert_eq!(messages.len(), usize::from(expected.is_some()));
            if let Some(pressed) = expected {
                assert_eq!(messages[0].arguments, [hiraku_script::Value::Bool(pressed)]);
                assert_eq!(messages[0].root, root);
            }
        }
    }

    #[test]
    fn frame_callbacks_use_mount_clock_and_request_redraw() {
        let (_, callback) = callback_fixture();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .add_message::<super::super::widgets::UiCallbackRequest>()
            .add_message::<bevy::window::RequestRedraw>()
            .add_systems(Update, update);
        let root = app
            .world_mut()
            .spawn((
                crate::ui::ScreenUiRoot,
                UiUpdate(callback),
                UiTimersPaused(false),
            ))
            .id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        for (paused, expected) in [(false, 1), (true, 0), (false, 1)] {
            app.world_mut()
                .entity_mut(root)
                .insert(UiTimersPaused(paused));
            app.update();
            let messages: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<super::super::widgets::UiCallbackRequest>>()
                .drain()
                .collect();
            assert_eq!(messages.len(), expected);
            if expected != 0 {
                assert_eq!(messages[0].arguments, [hiraku_script::Value::Number(0.1)]);
            }
            assert_eq!(
                app.world_mut()
                    .resource_mut::<Messages<bevy::window::RequestRedraw>>()
                    .drain()
                    .count(),
                expected
            );
        }
        app.world_mut().resource_mut::<ScreenUiState>().active_root = None;
        app.update();
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<super::super::widgets::UiCallbackRequest>>()
                .drain()
                .count(),
            0
        );
    }

    #[test]
    fn child_animation_uses_its_own_screen_clock() {
        #[derive(Resource)]
        struct Samples(Vec<(Entity, std::time::Duration)>);
        fn sample(clock: UiClock, mut samples: ResMut<Samples>) {
            for (entity, elapsed) in &mut samples.0 {
                *elapsed += clock.delta(*entity);
            }
        }
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .add_systems(Update, sample);
        let background = app.world_mut().spawn(crate::ui::ScreenUiRoot).id();
        let modal = app.world_mut().spawn(crate::ui::ScreenUiRoot).id();
        let a = app.world_mut().spawn(ChildOf(background)).id();
        let b = app.world_mut().spawn(ChildOf(modal)).id();
        app.insert_resource(Samples(vec![
            (a, std::time::Duration::ZERO),
            (b, std::time::Duration::ZERO),
        ]));
        {
            let mut screens = app.world_mut().resource_mut::<ScreenUiState>();
            screens.active_root = Some(modal);
            screens.stack.push((background, None));
        }
        let delta = std::time::Duration::from_millis(16);
        app.world_mut().resource_mut::<Time>().advance_by(delta);
        for _ in 0..100 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Samples>().0[0].1,
            std::time::Duration::ZERO
        );
        assert_eq!(app.world().resource::<Samples>().0[1].1, delta * 100);
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(background);
        app.world_mut()
            .resource_mut::<ScreenUiState>()
            .stack
            .clear();
        app.update();
        assert_eq!(app.world().resource::<Samples>().0[0].1, delta);
    }

    #[test]
    fn timers_pause_under_modals_fire_once_and_die_with_the_mount() {
        let spec = crate::script::evaluate_ui_component_named_with_args(
            "memory://timer.ui.hks",
            r#"import ui.widgets.*
                canvas { column { text("Alice") }.centered().rotation(15) }
                    .after(1) { ui.close("bob") }
            "#,
            crate::script::ui_runtime::UiContext::default(),
            &crate::texture::TextureCatalog::default(),
            &crate::glossary::TermCatalog::default(),
            &[],
        )
        .expect("timer closure compiles without executing");
        assert_eq!(spec.timers.len(), 1);
        let crate::ui::ScreenNode::Column(column) = &spec.children[0] else {
            panic!("column node")
        };
        assert_eq!(column.layout.rotation, 15.0);
        assert_eq!(column.justify.as_deref(), Some("center"));
        assert_eq!(column.align_items.as_deref(), Some("center"));
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .add_message::<super::super::widgets::UiCallbackRequest>()
            .add_systems(Update, tick);
        let root = app.world_mut().spawn(UiTimers::new(&spec.timers)).id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(600));
        app.update(); // Not the active screen: no elapsed time.
        assert_eq!(
            app.world().get::<UiTimers>(root).expect("timer").0[0]
                .0
                .elapsed_secs(),
            0.0
        );
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        app.update();
        assert!(
            !app.world()
                .get::<UiTimers>(root)
                .expect("timer")
                .0
                .is_empty()
        );
        app.world_mut()
            .entity_mut(root)
            .insert(UiTimersPaused(true));
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world().get::<UiTimers>(root).expect("paused timer").0[0]
                .0
                .elapsed_secs(),
            0.6
        );
        app.world_mut()
            .entity_mut(root)
            .insert(UiTimersPaused(false));
        app.update();
        let mut cursor = bevy::ecs::message::MessageCursor::<
            super::super::widgets::UiCallbackRequest,
        >::default();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<Messages<super::super::widgets::UiCallbackRequest>>()
                )
                .count(),
            1
        );
        app.update();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<Messages<super::super::widgets::UiCallbackRequest>>()
                )
                .count(),
            0
        );
        app.world_mut().despawn(root);
        app.update();
    }
}
