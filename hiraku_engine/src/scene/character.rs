use super::*;

pub fn animate_character_motion_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut stage: ResMut<StageState>,
    mut movers: Query<
        (
            Entity,
            &'static mut Transform,
            Option<&'static mut CharacterJumpEffect>,
            Option<&'static mut CharacterShakeEffect>,
            Option<&'static mut CharacterTimelineEffect>,
        ),
        Without<WorldCamera>,
    >,
) {
    for (entity, mut transform, jump, shake, timeline) in &mut movers {
        let base_origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin))
            .unwrap_or(transform.translation);

        let mut translation = base_origin;

        if let Some(mut effect) = timeline {
            effect.elapsed = (effect.elapsed + time.delta_secs()).min(effect.duration);
            let actor_position =
                character_timeline_position(effect.actor_origin, &effect.keyframes, effect.elapsed);
            translation += (actor_position - effect.actor_origin).extend(0.0);
            stage
                .character_positions
                .insert(effect.actor_id.clone(), actor_position);

            if effect.elapsed >= effect.duration {
                complete_missing_animation(
                    &mut animations,
                    effect.animation_id.take(),
                    effect.done.take(),
                );
                commands
                    .entity(entity)
                    .try_remove::<CharacterTimelineEffect>();
            }
        }

        if let Some(mut effect) = jump {
            effect.timer.tick(time.delta());
            let progress = tween_fraction(&effect.timer);
            translation.y += (std::f32::consts::PI * progress).sin().max(0.0) * effect.height;
            if effect.timer.is_finished() {
                complete_missing_animation(
                    &mut animations,
                    effect.animation_id.take(),
                    effect.done.take(),
                );
                commands.entity(entity).try_remove::<CharacterJumpEffect>();
            }
        }

        if let Some(mut effect) = shake {
            effect.timer.tick(time.delta());
            let decay = 1.0 - tween_fraction(&effect.timer);
            let elapsed = effect.timer.elapsed_secs();
            translation += Vec3::new(
                (elapsed * 52.0).sin() * effect.amplitude * decay,
                (elapsed * 39.0).cos() * effect.amplitude * 0.35 * decay,
                0.0,
            );
            if effect.timer.is_finished() {
                complete_missing_animation(
                    &mut animations,
                    effect.animation_id.take(),
                    effect.done.take(),
                );
                commands.entity(entity).try_remove::<CharacterShakeEffect>();
            }
        }

        transform.translation = translation;
    }
}

pub fn poll_pending_character_shows(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut pending: ResMut<PendingCharacterShows>,
    sprite_entities: Query<(), (With<Sprite>, With<Visibility>)>,
    mut sprites: Query<(&mut Sprite, &mut Visibility)>,
) {
    let mut completed = Vec::new();
    pending.items.retain_mut(|item| {
        let has_failed = item.handles.iter().any(|handle| {
            matches!(
                asset_server.load_state(handle.id()),
                bevy::asset::LoadState::Failed(_)
            )
        });
        if has_failed {
            warn!(
                "failed to load one or more parts for character `{}`",
                item.actor_id
            );
            for entity in item.entities.drain(..) {
                commands.entity(entity).try_despawn();
            }
            for id in item.entity_ids.drain(..) {
                stage.sprites.remove(&id);
            }
            complete_missing_animation(&mut animations, item.animation_id.take(), item.done.take());
            return false;
        }

        if !item
            .handles
            .iter()
            .all(|handle| asset_server.is_loaded_with_dependencies(handle.id()))
        {
            return true;
        }

        if !item
            .entities
            .iter()
            .all(|entity| sprite_entities.get(*entity).is_ok())
        {
            return true;
        }

        completed.push((
            item.entities.clone(),
            item.fade,
            item.animation_id.take(),
            item.done.take(),
        ));
        false
    });

    for (entities, fade, animation_id, done) in completed {
        let mut pending_done = done;
        let mut pending_animation = animation_id;
        for (index, entity) in entities.into_iter().enumerate() {
            if let Ok((mut sprite, mut visibility)) = sprites.get_mut(entity) {
                *visibility = Visibility::Visible;
                if let Some(fade) = fade {
                    sprite.color.set_alpha(0.0);
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(fade, TimerMode::Once),
                        animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                        done: (index == 0).then(|| pending_done.take()).flatten(),
                        despawn_on_finish: false,
                    });
                }
            }
        }

        if fade.is_none() {
            complete_missing_animation(&mut animations, pending_animation, pending_done);
        }
    }
}

pub(super) fn queue_character_show(
    commands: &mut Commands,
    asset_server: &AssetServer,
    texture_atlases: &TextureAtlasCatalog,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    actor_id: String,
    parts: Vec<CharacterPartDefinition>,
    position: Vec2,
    scale: f32,
    fade: Option<std::time::Duration>,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
) {
    let mut entities = Vec::new();
    let mut entity_ids = Vec::new();
    let mut handles = Vec::new();

    for part in &parts {
        let sprite_id = character_part_id(&actor_id, part);
        let atlas = texture_atlases.resolve(&part.path, part.atlas_rect);
        let handle = atlas
            .map(|texture| texture.image.clone())
            .unwrap_or_else(|| asset_server.load(part.path.clone()));
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite_id.clone(),
                    path: part.path.clone(),
                },
                character_part_sprite(handle.clone(), part, atlas),
                Visibility::Hidden,
                Transform {
                    translation: Vec3::new(
                        position.x + part.offset.x * scale,
                        position.y + part.offset.y * scale,
                        STAGE_Z_SPRITE + part.layer,
                    ),
                    scale: Vec3::splat(scale),
                    ..default()
                },
            ))
            .id();

        stage.sprites.insert(sprite_id.clone(), entity);
        entities.push(entity);
        entity_ids.push(sprite_id);
        handles.push(handle);
    }

    if entities.is_empty() {
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    pending.items.push(PendingCharacterShow {
        actor_id,
        entity_ids,
        entities,
        handles,
        fade,
        animation_id,
        done,
    });
}

pub(super) fn apply_character_motion(
    commands: &mut Commands,
    stage: &mut StageState,
    shared_state: &SceneSharedState,
    actor_id: &str,
    kind: CharacterMotionKind,
    duration: std::time::Duration,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
    animations: &mut AnimationState,
) {
    let prefix = character_part_prefix(actor_id);
    let snapshot = &shared_state.0;
    let mut part_ids = snapshot
        .sprites
        .iter()
        .filter(|sprite| sprite.id.starts_with(&prefix))
        .map(|sprite| (sprite.id.clone(), sprite.x, sprite.y, sprite.layer))
        .collect::<Vec<_>>();
    part_ids.sort_by(|left, right| left.0.cmp(&right.0));

    if part_ids.is_empty() {
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let mut pending_animation = animation_id;
    let mut pending_done = done;
    for (index, (id, x, y, layer)) in part_ids.into_iter().enumerate() {
        let Some(entity) = stage.sprites.get(&id).copied() else {
            continue;
        };
        match kind {
            CharacterMotionKind::Jump { height } => {
                commands.entity(entity).insert(CharacterJumpEffect {
                    origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
                    timer: Timer::new(duration, TimerMode::Once),
                    height,
                    animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                    done: (index == 0).then(|| pending_done.take()).flatten(),
                });
            }
            CharacterMotionKind::Shake { amplitude } => {
                commands.entity(entity).insert(CharacterShakeEffect {
                    origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
                    timer: Timer::new(duration, TimerMode::Once),
                    amplitude,
                    animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                    done: (index == 0).then(|| pending_done.take()).flatten(),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_character_timeline(
    commands: &mut Commands,
    stage: &mut StageState,
    shared_state: &SceneSharedState,
    actor_id: &str,
    keyframes: Vec<ResolvedCharacterKeyframe>,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
    animations: &mut AnimationState,
) {
    let Some(actor_origin) = stage.character_positions.get(actor_id).copied() else {
        complete_missing_animation(animations, animation_id, done);
        return;
    };

    let duration = keyframes.last().map(|frame| frame.time).unwrap_or(0.0);
    if duration <= f32::EPSILON {
        if let Some(final_position) = keyframes.last().map(|frame| frame.position) {
            stage
                .character_positions
                .insert(actor_id.to_string(), final_position);
        }
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let prefix = character_part_prefix(actor_id);
    let snapshot = &shared_state.0;
    let mut part_ids = snapshot
        .sprites
        .iter()
        .filter(|sprite| sprite.id.starts_with(&prefix))
        .map(|sprite| (sprite.id.clone(), sprite.x, sprite.y, sprite.layer))
        .collect::<Vec<_>>();
    part_ids.sort_by(|left, right| left.0.cmp(&right.0));

    if part_ids.is_empty() {
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let mut pending_animation = animation_id;
    let mut pending_done = done;
    for (index, (id, x, y, layer)) in part_ids.into_iter().enumerate() {
        let Some(entity) = stage.sprites.get(&id).copied() else {
            continue;
        };
        commands.entity(entity).insert(CharacterTimelineEffect {
            origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
            actor_id: actor_id.to_string(),
            actor_origin,
            keyframes: keyframes.clone(),
            elapsed: 0.0,
            duration,
            animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
            done: (index == 0).then(|| pending_done.take()).flatten(),
        });
    }
}

fn character_timeline_position(
    actor_origin: Vec2,
    keyframes: &[ResolvedCharacterKeyframe],
    elapsed: f32,
) -> Vec2 {
    let Some(first) = keyframes.first() else {
        return actor_origin;
    };

    if elapsed <= first.time {
        return interpolate_character_position(actor_origin, 0.0, first.clone(), elapsed);
    }

    let mut previous = ResolvedCharacterKeyframe {
        time: 0.0,
        position: actor_origin,
        ease: CharacterEase::Linear,
    };
    for keyframe in keyframes {
        if elapsed <= keyframe.time {
            return interpolate_character_position(
                previous.position,
                previous.time,
                keyframe.clone(),
                elapsed,
            );
        }
        previous = keyframe.clone();
    }

    keyframes
        .last()
        .map(|frame| frame.position)
        .unwrap_or(actor_origin)
}

fn interpolate_character_position(
    start: Vec2,
    start_time: f32,
    end: ResolvedCharacterKeyframe,
    elapsed: f32,
) -> Vec2 {
    let duration = (end.time - start_time).max(f32::EPSILON);
    let fraction = ((elapsed - start_time) / duration).clamp(0.0, 1.0);
    let fraction = apply_character_ease(end.ease, fraction);
    start.lerp(end.position, fraction)
}

pub(crate) fn apply_character_ease(ease: CharacterEase, t: f32) -> f32 {
    match ease {
        CharacterEase::Linear => t,
        CharacterEase::Ease | CharacterEase::EaseInOut => t * t * (3.0 - 2.0 * t),
        CharacterEase::EaseIn => t * t,
        CharacterEase::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        CharacterEase::Bounce => {
            let n1 = 7.5625;
            let d1 = 2.75;
            if t < 1.0 / d1 {
                n1 * t * t
            } else if t < 2.0 / d1 {
                let t = t - 1.5 / d1;
                n1 * t * t + 0.75
            } else if t < 2.5 / d1 {
                let t = t - 2.25 / d1;
                n1 * t * t + 0.9375
            } else {
                let t = t - 2.625 / d1;
                n1 * t * t + 0.984375
            }
        }
    }
}

pub(super) fn despawn_character_actor(
    commands: &mut Commands,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    actor_id: &str,
) {
    let prefix = character_part_prefix(actor_id);

    pending.items.retain_mut(|item| {
        if item.actor_id != actor_id {
            return true;
        }

        for entity in item.entities.drain(..) {
            commands.entity(entity).try_despawn();
        }
        false
    });

    let ids = stage
        .sprites
        .keys()
        .filter(|id| id.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    for id in ids {
        if let Some(entity) = stage.sprites.remove(&id) {
            commands.entity(entity).try_despawn();
        }
    }
}

pub(super) fn character_part_prefix(actor_id: &str) -> String {
    format!("character::{actor_id}::")
}

fn character_part_id(actor_id: &str, part: &CharacterPartDefinition) -> String {
    match part.slot {
        Some(slot) => format!(
            "{}slot-{slot:03}-{}",
            character_part_prefix(actor_id),
            part.id
        ),
        None => format!("{}{}", character_part_prefix(actor_id), part.id),
    }
}

fn character_part_sprite(
    image: Handle<Image>,
    part: &CharacterPartDefinition,
    atlas: Option<&crate::texture::AtlasTexture>,
) -> Sprite {
    let mut sprite = atlas
        .map(|atlas| Sprite::from_atlas_image(image.clone(), atlas.atlas.clone()))
        .unwrap_or_else(|| Sprite::from_image(image));
    if atlas.is_none() {
        sprite.rect = part.rect.map(array_to_rect);
    }
    sprite
}

pub(super) fn array_to_rect(rect: [f32; 4]) -> Rect {
    Rect::from_corners(Vec2::new(rect[0], rect[1]), Vec2::new(rect[2], rect[3]))
}

pub(super) fn rect_to_array(rect: Rect) -> [f32; 4] {
    [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
}
