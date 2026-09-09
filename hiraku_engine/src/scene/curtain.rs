//! Data-driven scene curtain transitions. Mask loading never consumes fade time.
use super::*;
use crate::render::world_sprite::DissolveMask;
use std::time::Duration;

#[derive(Component)]
pub struct PendingCurtain {
    pub color: [u8; 3],
    pub opacity: f32,
    pub duration: Option<Duration>,
    pub mask: Option<DissolveMask>,
}

#[derive(Component)]
pub struct CurtainWait(pub ScriptRequestId);

#[derive(Component)]
pub struct CurtainFailed(pub String);

pub fn complete_curtain_waits(
    mut commands: Commands,
    mut runtime: ResMut<ScriptRuntimeState>,
    stage: Res<StageState>,
    waits: Query<(Entity, &CurtainWait)>,
    curtains: Query<
        (
            Option<&PendingCurtain>,
            Option<&VisualTween>,
            Option<&CurtainFailed>,
        ),
        With<OverlayMarker>,
    >,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    for (entity, wait) in &waits {
        if runtime.wait_request != Some(wait.0) && !runtime.task_requests.contains_key(&wait.0) {
            commands.entity(entity).try_despawn();
            continue;
        }
        let state = stage.overlay.and_then(|entity| curtains.get(entity).ok());
        let failure = match state {
            None => Some("scene curtain entity is unavailable"),
            Some((_, _, Some(failure))) => Some(failure.0.as_str()),
            _ => None,
        };
        if let Some(error) = failure {
            crate::script::emit_script_diagnostic("curtain transition failed", error);
            runtime.story = None;
            runtime.wait_request = None;
        } else if let Some((pending, tween, _)) = state {
            if pending.is_some() || tween.is_some_and(|tween| !tween.timer.is_finished()) {
                continue;
            }
            responses.write(ScriptResponseMessage {
                request: wait.0,
                response: ScriptResponse::Continue,
            });
        }
        commands.entity(entity).try_despawn();
    }
}

pub fn update_curtains(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    images: Res<Assets<Image>>,
    server: Res<AssetServer>,
    canvas: Res<crate::HirakuCanvas>,
    mut curtains: Query<(Entity, &mut WorldSprite, &PendingCurtain)>,
) {
    if !curtains.is_empty() { redraw.request(); }
    for (entity, mut sprite, pending) in &mut curtains {
        if let Some(mask) = &pending.mask {
            if !images.contains(mask.image.id()) {
                if matches!(
                    server.load_state(mask.image.id()),
                    bevy::asset::LoadState::Failed(_)
                ) {
                    error!("failed to load curtain dissolve mask `{}`", mask.path);
                    commands
                        .entity(entity)
                        .remove::<PendingCurtain>()
                        .insert(CurtainFailed(format!(
                            "failed to load dissolve mask `{}`",
                            mask.path
                        )));
                }
                continue;
            }
        }
        let from = sprite.color.alpha();
        sprite.color = Color::srgba_u8(pending.color[0], pending.color[1], pending.color[2], 255)
            .with_alpha(from);
        sprite.dissolve = pending.mask.clone();
        if let Some(mask) = &mut sprite.dissolve {
            mask.canvas_size = canvas.size.as_vec2();
            mask.reversed = pending.opacity < from;
        }
        if let Some(duration) = pending.duration {
            commands.entity(entity).insert(VisualTween {
                from_alpha: Some(from),
                to_alpha: Some(pending.opacity),
                from_translation: None,
                to_translation: None,
                from_scale: None,
                to_scale: None,
                timer: Timer::new(duration, TimerMode::Once),
                animation_id: None,
                despawn_on_finish: false,
            });
        } else {
            sprite.color = sprite.color.with_alpha(pending.opacity);
            commands.entity(entity).remove::<VisualTween>();
        }
        commands.entity(entity).remove::<PendingCurtain>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiter_requires_loaded_and_finished_transition_and_ignores_stale_requests() {
        let mut app = App::new();
        app.init_resource::<ScriptRuntimeState>()
            .init_resource::<StageState>()
            .add_message::<ScriptResponseMessage>()
            .add_systems(Update, complete_curtain_waits);
        let request = ScriptRequestId(7);
        app.world_mut()
            .resource_mut::<ScriptRuntimeState>()
            .wait_request = Some(request);
        let overlay = app
            .world_mut()
            .spawn((
                OverlayMarker,
                PendingCurtain {
                    color: [0; 3],
                    opacity: 1.0,
                    duration: None,
                    mask: None,
                },
            ))
            .id();
        app.world_mut().resource_mut::<StageState>().overlay = Some(overlay);
        app.world_mut().spawn(CurtainWait(request));
        let stale = app.world_mut().spawn(CurtainWait(ScriptRequestId(6))).id();
        app.update();
        assert!(app.world().get_entity(stale).is_err());
        assert!(
            app.world()
                .resource::<bevy::ecs::message::Messages<ScriptResponseMessage>>()
                .is_empty()
        );
        app.world_mut()
            .entity_mut(overlay)
            .remove::<PendingCurtain>()
            .insert(VisualTween {
                from_alpha: Some(0.0),
                to_alpha: Some(1.0),
                from_translation: None,
                to_translation: None,
                from_scale: None,
                to_scale: None,
                timer: Timer::new(Duration::from_secs(1), TimerMode::Once),
                animation_id: None,
                despawn_on_finish: false,
            });
        app.update();
        assert!(
            app.world()
                .resource::<bevy::ecs::message::Messages<ScriptResponseMessage>>()
                .is_empty()
        );
        app.world_mut().entity_mut(overlay).remove::<VisualTween>();
        app.update();
        let replies: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<ScriptResponseMessage>>()
            .drain()
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].request, request);
        app.update();
        assert!(
            app.world()
                .resource::<bevy::ecs::message::Messages<ScriptResponseMessage>>()
                .is_empty()
        );
    }

    #[test]
    fn mask_load_does_not_consume_transition_time() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>()
        .insert_resource(crate::HirakuCanvas {
            image: Handle::default(),
            size: UVec2::new(1280, 720),
        })
        .add_systems(Update, update_curtains);
        let image = app.world().resource::<Assets<Image>>().reserve_handle();
        let entity = app
            .world_mut()
            .spawn((
                WorldSprite::from_color(Color::BLACK.with_alpha(0.0), Vec2::splat(6000.0)),
                PendingCurtain {
                    color: [255, 32, 0],
                    opacity: 1.0,
                    duration: Some(Duration::from_millis(900)),
                    mask: Some(DissolveMask {
                        image: image.clone(),
                        path: "memory://mask.png".into(),
                        softness: 0.0,
                        canvas_size: Vec2::ONE,
                        reversed: false,
                    }),
                },
            ))
            .id();
        app.update();
        assert!(app.world().get::<PendingCurtain>(entity).is_some());
        assert!(app.world().get::<VisualTween>(entity).is_none());
        assert_eq!(
            app.world()
                .get::<WorldSprite>(entity)
                .expect("curtain")
                .color
                .to_srgba()
                .red,
            0.0
        );
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(image.id(), Image::default())
            .expect("reserved mask inserted");
        app.update();
        assert!(app.world().get::<PendingCurtain>(entity).is_none());
        let color = app
            .world()
            .get::<WorldSprite>(entity)
            .expect("curtain")
            .color
            .to_srgba();
        assert_eq!(color.to_u8_array(), [255, 32, 0, 0]);
        let tween = app
            .world()
            .get::<VisualTween>(entity)
            .expect("transition starts once loaded");
        assert_eq!(tween.timer.remaining(), Duration::from_millis(900));
        assert_eq!(
            app.world()
                .get::<WorldSprite>(entity)
                .expect("curtain")
                .dissolve
                .as_ref()
                .expect("mask")
                .canvas_size,
            Vec2::new(1280.0, 720.0)
        );
    }

    #[test]
    fn scene_save_preserves_dissolve_progress_and_remaining_time() {
        let original = crate::state::SceneSnapshot {
            overlay_alpha: 0.4,
            curtain: Some(crate::state::CurtainSnapshot {
                color: [255, 0, 0],
                target_color: [0, 0, 255],
                mask: Some("textures/mask.png".into()),
                softness: 0.05,
                target: 1.0,
                remaining_ms: 540,
            }),
            ..default()
        };
        let encoded = crate::proto::SceneSnapshot::from(&original);
        let restored =
            crate::state::SceneSnapshot::try_from(encoded).expect("curtain snapshot round trips");
        assert_eq!(restored.overlay_alpha, 0.4);
        let curtain = restored.curtain.expect("saved curtain");
        assert_eq!(curtain.color, [255, 0, 0]);
        assert_eq!(curtain.target_color, [0, 0, 255]);
        assert_eq!(curtain.mask.as_deref(), Some("textures/mask.png"));
        assert_eq!(curtain.target, 1.0);
        assert_eq!(curtain.remaining_ms, 540);
    }
}
