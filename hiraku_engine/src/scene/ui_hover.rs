//! State-driven UI movement, with normal Bevy picking on the transformed subtree.
use super::*;
use bevy::picking::hover::HoverMap;

#[derive(Component)]
pub struct HoverMotion {
    offset: Vec2,
    current: Vec2,
    from: Vec2,
    target: Vec2,
    elapsed: f32,
    animation: crate::script::AnimationSpec,
    selected: bool,
    binding: Option<crate::ui::PropertyComputation>,
    revision: u64,
}

impl HoverMotion {
    pub(super) fn transform(&self) -> UiTransform {
        UiTransform {
            translation: Val2::px(self.current.x, self.current.y),
            ..UiTransform::IDENTITY
        }
    }
    pub fn new(layout: &ScreenLayout) -> Self {
        let offset = Vec2::from_array(layout.hover_offset.unwrap_or_default());
        let initial = if layout.hover_active {
            offset
        } else {
            Vec2::ZERO
        };
        Self {
            offset,
            current: initial,
            from: initial,
            target: initial,
            elapsed: 0.0,
            animation: layout
                .animation
                .unwrap_or(crate::script::AnimationSpec::Linear(0.2, false)),
            selected: layout.hover_active,
            binding: layout.reactive_hover_active.clone(),
            revision: u64::MAX,
        }
    }

    fn advance(&mut self, active: bool, delta: f32) -> Vec2 {
        let target = if active { self.offset } else { Vec2::ZERO };
        if target != self.target {
            self.from = self.current;
            self.target = target;
            self.elapsed = 0.0;
        }
        let duration = self.animation.duration();
        self.elapsed = (self.elapsed + delta).min(duration.max(0.0));
        let progress = if duration > 0.0 {
            (self.elapsed / duration).min(1.0)
        } else {
            1.0
        };
        self.current = self.from.lerp(self.target, self.animation.sample(progress));
        self.current
    }
}

pub fn animate_hover(
    mut redraw: crate::redraw::Redraw,
    time: super::ui_timers::UiClock,
    models: Res<UiModels>,
    screen: Res<ScreenUiState>,
    overlays: Res<OverlayUiState>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<ScreenUiRoot>>,
    local_states: Query<&super::widgets::UiLocalState>,
    containers: Query<(), With<HoverMotion>>,
    mut motions: Query<(Entity, &mut HoverMotion, &mut UiTransform)>,
    hover_map: Option<Res<HoverMap>>,
) {
    // Read Bevy's current picking state, including virtual pointers. Derive
    // ancestor hover afresh so removed children cannot leave a stuck pose.
    let mut hovered = HashSet::new();
    if let Some(hover_map) = hover_map {
        for hits in hover_map.0.values() {
            for hit in hits.keys() {
                let mut entity = *hit;
                loop {
                    if containers.contains(entity) {
                        hovered.insert(entity);
                    }
                    let Ok(parent) = parents.get(entity) else {
                        break;
                    };
                    entity = parent.parent();
                }
            }
        }
    }
    for (entity, mut motion, mut transform) in &mut motions {
        let delta = time.delta(entity);
        if delta.is_zero() { continue; }
        let root = find_component_ancestor(entity, &roots, &parents);
        let interactive = root.is_some_and(|root| screen.accepts_input(root, &overlays));
        let revision = models.revision();
        let old_revision = motion.revision;
        if let Some(binding) = &mut motion.binding {
            let changed =
                super::screen_ui::refresh_local_binding(entity, binding, &parents, &local_states);
            if changed || revision != old_revision {
                match crate::script::evaluate_ui_reactive_binding(binding, &models) {
                    Ok(hiraku_script::Value::Bool(value)) => motion.selected = value,
                    Ok(_) => warn!("hover active expression must return Bool"),
                    Err(error) => crate::script::emit_script_diagnostic(
                        "hover expression failed",
                        &error.to_string(),
                    ),
                }
            }
        }
        motion.revision = revision;
        let active = motion.selected || (interactive && hovered.contains(&entity));
        let old_position = motion.current;
        let position = motion.advance(active, delta.as_secs_f32());
        if position != old_position || motion.current != motion.target { redraw.request(); }
        let translation = Val2::px(position.x, position.y);
        if transform.translation != translation {
            transform.translation = translation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_mount_starts_at_its_selected_position() {
        let motion = HoverMotion::new(&ScreenLayout {
            hover_offset: Some([-84.0, 0.0]),
            hover_active: true,
            ..default()
        });
        assert_eq!(motion.transform().translation, Val2::px(-84.0, 0.0));
    }

    #[test]
    fn virtual_pointer_moves_subtree_and_respects_modal_input() {
        use bevy::picking::{backend::HitData, pointer::PointerId};
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<bevy::window::RequestRedraw>()
            .init_resource::<UiModels>()
            .init_resource::<ScreenUiState>()
            .init_resource::<OverlayUiState>()
            .init_resource::<HoverMap>()
            .add_systems(Update, animate_hover);
        let mut redraws = bevy::ecs::message::MessageCursor::<bevy::window::RequestRedraw>::default();
        let root = app.world_mut().spawn(ScreenUiRoot).id();
        let group = app
            .world_mut()
            .spawn((
                ChildOf(root),
                UiTransform::IDENTITY,
                HoverMotion::new(&ScreenLayout {
                    hover_offset: Some([-84.0, 0.0]),
                    ..default()
                }),
            ))
            .id();
        let button = app
            .world_mut()
            .spawn((ChildOf(group), UiTransform::IDENTITY))
            .id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(200));
        app.world_mut()
            .resource_mut::<HoverMap>()
            .0
            .entry(PointerId::Touch(7))
            .or_default()
            .insert(button, HitData::new(root, 0.0, None, None));
        app.update();
        assert_eq!(redraws.read(app.world().resource::<Messages<bevy::window::RequestRedraw>>()).count(), 1);
        app.update();
        assert_eq!(redraws.read(app.world().resource::<Messages<bevy::window::RequestRedraw>>()).count(), 0);
        assert_eq!(
            app.world()
                .get::<UiTransform>(group)
                .expect("group transform")
                .translation,
            Val2::px(-84.0, 0.0)
        );
        assert_eq!(
            app.world()
                .get::<UiTransform>(button)
                .expect("button transform")
                .translation,
            Val2::ZERO
        );
        let modal = app.world_mut().spawn(ScreenUiRoot).id();
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(modal);
        app.update();
        assert_eq!(
            app.world()
                .get::<UiTransform>(group)
                .expect("blocked transform")
                .translation,
            Val2::px(-84.0, 0.0) // Covered screen's animation clock is frozen.
        );
        app.world_mut().resource_mut::<ScreenUiState>().active_root = Some(root);
        app.world_mut().resource_mut::<HoverMap>().0.clear();
        app.update();
        assert_eq!(
            app.world()
                .get::<UiTransform>(group)
                .expect("released transform")
                .translation,
            Val2::ZERO
        );
    }
    #[test]
    fn hover_reverses_from_current_pose_and_can_be_held() {
        let mut motion = HoverMotion::new(&ScreenLayout {
            hover_offset: Some([-84.0, 0.0]),
            ..default()
        });
        assert_eq!(motion.advance(true, 0.1), Vec2::new(-42.0, 0.0));
        assert_eq!(motion.advance(false, 0.0), Vec2::new(-42.0, 0.0));
        assert_eq!(motion.advance(false, 0.2), Vec2::ZERO);
        assert_eq!(motion.advance(true, 0.2), Vec2::new(-84.0, 0.0));
        assert_eq!(motion.advance(true, 1.0), Vec2::new(-84.0, 0.0));
    }
}
