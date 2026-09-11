//! Mount-owned one-shot UI timers. Covered/pending modals pause, not expire.
use crate::ui::{OverlayUiState, ScreenUiState, UiCallback};
use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct UiTimers(Vec<(Timer, UiCallback)>);

#[derive(Component)]
pub(crate) struct UiTimersPaused(pub bool);

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
    pub fn delta(&self, mut entity: Entity) -> std::time::Duration {
        let (Some(screens), Some(overlays)) = (&self.screens, &self.overlays) else {
            return self.time.delta();
        };
        if self.dependencies.as_ref().is_some_and(|d| d.loading) {
            return std::time::Duration::ZERO;
        }
        loop {
            if let Ok(deps) = self.image_dependencies.get(entity) {
                if !self.images.as_ref().is_some_and(|assets| deps.0.iter().all(|h| assets.contains(h))) {
                    return std::time::Duration::ZERO;
                }
            }
            if let Ok(deps) = self.shader_dependencies.get(entity) {
                if !self.shaders.as_ref().is_some_and(|assets| deps.0.iter().all(|h| assets.contains(h))) {
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
