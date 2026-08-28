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
                complete_missing_animation(&mut animations, effect.animation_id.take());
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
                complete_missing_animation(&mut animations, effect.animation_id.take());
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
                complete_missing_animation(&mut animations, effect.animation_id.take());
                commands.entity(entity).try_remove::<CharacterShakeEffect>();
            }
        }

        transform.translation = translation;
    }
}

pub fn poll_pending_character_shows(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut alpha_mask_materials: ResMut<Assets<AlphaMaskMaterial>>,
    mut multiply_materials: ResMut<Assets<MultiplyMaterial>>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut pending: ResMut<PendingCharacterShows>,
    mut visual_queries: ParamSet<(
        Query<
            (
                Option<&WorldSprite>,
                Option<&Mesh3d>,
                Option<&MeshMaterial3d<WorldSpriteMaterial>>,
            ),
            (With<CharacterPartVisual>, With<Visibility>),
        >,
        Query<(
            &CharacterPartVisual,
            Option<&mut WorldSprite>,
            Option<&MeshMaterial3d<AlphaMaskMaterial>>,
            Option<&MeshMaterial3d<MultiplyMaterial>>,
            &mut Visibility,
        )>,
    )>,
) {
    let mut completed = Vec::new();
    {
        let visual_entities = visual_queries.p0();
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
                complete_missing_animation(&mut animations, item.animation_id.take());
                return false;
            }

            if !item
                .handles
                .iter()
                .all(|handle| asset_server.is_loaded_with_dependencies(handle.id()))
            {
                return true;
            }

            if !item.entities.iter().all(|entity| {
                visual_entities
                    .get(*entity)
                    .is_ok_and(|(sprite, mesh, material)| {
                        sprite.is_none() || (mesh.is_some() && material.is_some())
                    })
            }) {
                return true;
            }

            completed.push((
                item.entities.clone(),
                std::mem::take(&mut item.outgoing),
                item.fade,
                item.animation_id.take(),
            ));
            false
        });
    }

    let mut visuals = visual_queries.p1();
    for (entities, outgoing, fade, animation_id) in completed {
        let mut pending_animation = animation_id;
        for (index, entity) in entities.into_iter().enumerate() {
            if let Ok((visual, sprite, alpha_mask, multiply, mut visibility)) =
                visuals.get_mut(entity)
            {
                *visibility = Visibility::Visible;
                if let Some(fade) = fade {
                    set_character_part_alpha(
                        visual,
                        sprite,
                        alpha_mask,
                        multiply,
                        &mut alpha_mask_materials,
                        &mut multiply_materials,
                        0.0,
                    );
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(fade, TimerMode::Once),
                        animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                        despawn_on_finish: false,
                    });
                }
            }
        }

        for (id, entity) in outgoing {
            if stage.sprites.get(&id) == Some(&entity) {
                stage.sprites.remove(&id);
            }
            if let Some(fade) = fade {
                commands.entity(entity).try_insert(VisualTween {
                    from_alpha: Some(1.0),
                    to_alpha: Some(0.0),
                    from_translation: None,
                    to_translation: None,
                    from_scale: None,
                    to_scale: None,
                    timer: Timer::new(fade, TimerMode::Once),
                    animation_id: None,
                    despawn_on_finish: true,
                });
            } else {
                commands.entity(entity).try_despawn();
            }
        }

        if fade.is_none() {
            complete_missing_animation(&mut animations, pending_animation);
        }
    }
}

fn set_character_part_alpha(
    visual: &CharacterPartVisual,
    sprite: Option<Mut<WorldSprite>>,
    alpha_mask: Option<&MeshMaterial3d<AlphaMaskMaterial>>,
    multiply: Option<&MeshMaterial3d<MultiplyMaterial>>,
    alpha_mask_materials: &mut Assets<AlphaMaskMaterial>,
    multiply_materials: &mut Assets<MultiplyMaterial>,
    alpha: f32,
) {
    if let Some(mut sprite) = sprite {
        sprite.color.set_alpha(visual.base_alpha * alpha);
    }
    if let Some(material) = alpha_mask
        && let Some(mut material) = alpha_mask_materials.get_mut(&material.0)
    {
        material.opacity = alpha;
    }
    if let Some(material) = multiply
        && let Some(mut material) = multiply_materials.get_mut(&material.0)
    {
        material.opacity = alpha;
    }
}

pub(super) fn queue_character_show(
    commands: &mut Commands,
    asset_server: &AssetServer,
    _texture_atlases: &TextureAtlasCatalog,
    meshes: &mut Assets<Mesh>,
    alpha_mask_materials: &mut Assets<AlphaMaskMaterial>,
    multiply_materials: &mut Assets<MultiplyMaterial>,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    actor_id: String,
    parts: Vec<CharacterPartDefinition>,
    position: Vec2,
    scale: f32,
    focused: bool,
    fade: Option<std::time::Duration>,
    animation_id: Option<String>,
) {
    const DEFAULT_CHARACTER_FADE: std::time::Duration = std::time::Duration::from_millis(120);

    let fade = fade.or(Some(DEFAULT_CHARACTER_FADE));
    let prefix = character_part_prefix(&actor_id);
    let desired_ids = parts
        .iter()
        .map(|part| character_part_id(&actor_id, part))
        .collect::<HashSet<_>>();

    // A previous statement can still be waiting for its images. Retain only
    // parts that are also present in the newly committed actor state.
    pending.items.retain_mut(|item| {
        if item.actor_id != actor_id {
            return true;
        }
        for index in (0..item.entity_ids.len()).rev() {
            if desired_ids.contains(&item.entity_ids[index]) {
                continue;
            }
            let id = item.entity_ids.remove(index);
            let entity = item.entities.remove(index);
            item.handles.remove(index);
            if stage.sprites.get(&id) == Some(&entity) {
                stage.sprites.remove(&id);
            }
            commands.entity(entity).try_despawn();
        }
        if item.entities.is_empty() {
            complete_missing_animation(animations, item.animation_id.take());
            false
        } else {
            true
        }
    });

    let existing_ids = stage
        .sprites
        .keys()
        .filter(|id| id.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    let new_part_count = parts
        .iter()
        .filter(|part| {
            !stage
                .sprites
                .contains_key(&character_part_id(&actor_id, part))
        })
        .count();
    let mut pending_animation = animation_id;
    let mut outgoing = Vec::new();

    // Slots absent from the committed state fade out. Stable part IDs remain
    // alive, retaining texture/visibility/tween state across expression changes.
    for id in existing_ids {
        if desired_ids.contains(&id) {
            continue;
        }
        let Some(entity) = stage.sprites.get(&id).copied() else {
            continue;
        };
        if new_part_count > 0 {
            outgoing.push((id, entity));
            continue;
        }
        stage.sprites.remove(&id);
        commands.entity(entity).try_insert(VisualTween {
            from_alpha: Some(1.0),
            to_alpha: Some(0.0),
            from_translation: None,
            to_translation: None,
            from_scale: None,
            to_scale: None,
            timer: Timer::new(fade.expect("character fade is always set"), TimerMode::Once),
            animation_id: (new_part_count == 0)
                .then(|| pending_animation.take())
                .flatten(),
            despawn_on_finish: true,
        });
    }

    let mut entities = Vec::new();
    let mut entity_ids = Vec::new();
    let mut handles = Vec::new();

    for part in &parts {
        let sprite_id = character_part_id(&actor_id, part);
        if let Some(entity) = stage.sprites.get(&sprite_id).copied() {
            let mut entity_commands = commands.entity(entity);
            entity_commands.try_insert(Transform {
                translation: Vec3::new(
                    position.x + part.offset.x * scale,
                    position.y + part.offset.y * scale,
                    STAGE_Z_SPRITE + part.layer,
                ),
                scale: Vec3::splat(scale),
                ..default()
            });
            if focused {
                entity_commands.try_insert((FocusedActorPart, focus_layer()));
            } else {
                entity_commands.try_remove::<FocusedActorPart>();
                entity_commands.try_insert(scene_layer());
            }
            continue;
        }
        let transform = Transform {
            translation: Vec3::new(
                position.x + part.offset.x * scale,
                position.y + part.offset.y * scale,
                STAGE_Z_SPRITE + part.layer,
            ),
            scale: Vec3::splat(scale),
            ..default()
        };
        let color = crate::render::character_part::rgba8_color(part.color);
        let base_alpha = color.alpha();
        let visual = CharacterPartVisual {
            base_alpha,
            rect: part.rect,
        };

        let reads_mask = part
            .mask
            .is_some_and(|mask| mask.kind == CharacterMaskKind::Read);
        let requires_material = reads_mask || part.blend != CharacterBlendMode::Normal;
        let handle = asset_server.load(part.path.clone());
        let entity = if requires_material {
            match part.rect {
                None => {
                    warn!(
                        "character part `{}` requires an atlas rect for mask/blend rendering; using a normal sprite",
                        part.id
                    );
                    let mut sprite = character_part_sprite(handle.clone(), part);
                    sprite.color = color;
                    commands
                        .spawn((
                            SpriteActor {
                                id: sprite_id.clone(),
                                path: part.path.clone(),
                            },
                            sprite,
                            visual,
                            Visibility::Hidden,
                            transform,
                        ))
                        .id()
                }
                Some(rect) => {
                    let width = rect[2] - rect[0];
                    let height = rect[3] - rect[1];
                    let mesh = meshes.add(Rectangle::new(width, height));
                    if reads_mask {
                        let writer = mask_writer_for_part(&parts, part);
                        if writer.is_none() {
                            let reference = part
                                .mask
                                .expect("a mask reader must carry mask metadata")
                                .reference;
                            warn!(
                                "character part `{}` reads mask ref `{reference}` but no selected writer exists",
                                part.id,
                            );
                        }
                        let writer = writer.unwrap_or(part);
                        let mask_rect = writer.rect.unwrap_or(rect);
                        let material = alpha_mask_materials.add(AlphaMaskMaterial {
                            texture: handle.clone(),
                            mask_texture: asset_server.load(writer.path.clone()),
                            tint: crate::render::character_part::rgba8_linear(part.color),
                            main_rect: Vec4::new(rect[0], rect[1], width, height),
                            mask_rect: Vec4::new(
                                mask_rect[0],
                                mask_rect[1],
                                mask_rect[2] - mask_rect[0],
                                mask_rect[3] - mask_rect[1],
                            ),
                            offsets: Vec4::new(
                                part.offset.x,
                                part.offset.y,
                                writer.offset.x,
                                writer.offset.y,
                            ),
                            opacity: 1.0,
                            mask_enabled: (writer.id != part.id) as u8 as f32,
                        });
                        commands
                            .spawn((
                                SpriteActor {
                                    id: sprite_id.clone(),
                                    path: part.path.clone(),
                                },
                                Mesh3d(mesh),
                                MeshMaterial3d(material),
                                visual,
                                Visibility::Hidden,
                                transform,
                            ))
                            .id()
                    } else {
                        let material = multiply_materials.add(MultiplyMaterial {
                            texture: handle.clone(),
                            tint: crate::render::character_part::rgba8_linear(part.color),
                            rect: Vec4::new(rect[0], rect[1], width, height),
                            opacity: 1.0,
                        });
                        commands
                            .spawn((
                                SpriteActor {
                                    id: sprite_id.clone(),
                                    path: part.path.clone(),
                                },
                                Mesh3d(mesh),
                                MeshMaterial3d(material),
                                visual,
                                Visibility::Hidden,
                                transform,
                            ))
                            .id()
                    }
                }
            }
        } else {
            let mut sprite = character_part_sprite(handle.clone(), part);
            sprite.color = color;
            commands
                .spawn((
                    SpriteActor {
                        id: sprite_id.clone(),
                        path: part.path.clone(),
                    },
                    sprite,
                    visual,
                    Visibility::Hidden,
                    transform,
                ))
                .id()
        };

        stage.sprites.insert(sprite_id.clone(), entity);
        if focused {
            commands
                .entity(entity)
                .try_insert((FocusedActorPart, focus_layer()));
        } else {
            commands.entity(entity).try_insert(scene_layer());
        }
        entities.push(entity);
        entity_ids.push(sprite_id);
        handles.push(handle);
    }

    if entities.is_empty() {
        complete_missing_animation(animations, pending_animation);
        return;
    }

    pending.items.push(PendingCharacterShow {
        actor_id,
        entity_ids,
        entities,
        handles,
        outgoing,
        fade,
        animation_id: pending_animation,
    });
}

fn mask_writer_for_part<'a>(
    parts: &'a [CharacterPartDefinition],
    reader: &CharacterPartDefinition,
) -> Option<&'a CharacterPartDefinition> {
    let reader_mask = reader
        .mask
        .filter(|mask| mask.kind == CharacterMaskKind::Read)?;
    parts
        .iter()
        .filter(|part| {
            part.mask.is_some_and(|mask| {
                mask.kind == CharacterMaskKind::Write
                    && mask.reference == reader_mask.reference
                    && part.layer <= reader.layer
            })
        })
        .max_by(|left, right| left.layer.total_cmp(&right.layer))
        .or_else(|| {
            parts.iter().find(|part| {
                part.mask.is_some_and(|mask| {
                    mask.kind == CharacterMaskKind::Write && mask.reference == reader_mask.reference
                })
            })
        })
}

pub(super) fn apply_character_motion(
    commands: &mut Commands,
    stage: &mut StageState,
    shared_state: &SceneSharedState,
    actor_id: &str,
    kind: CharacterMotionKind,
    duration: std::time::Duration,
    animation_id: Option<String>,
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
        complete_missing_animation(animations, animation_id);
        return;
    }

    let mut pending_animation = animation_id;
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
                });
            }
            CharacterMotionKind::Shake { amplitude } => {
                commands.entity(entity).insert(CharacterShakeEffect {
                    origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
                    timer: Timer::new(duration, TimerMode::Once),
                    amplitude,
                    animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
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
    animations: &mut AnimationState,
) {
    let Some(actor_origin) = stage.character_positions.get(actor_id).copied() else {
        complete_missing_animation(animations, animation_id);
        return;
    };

    let duration = keyframes.last().map(|frame| frame.time).unwrap_or(0.0);
    if duration <= f32::EPSILON {
        if let Some(final_position) = keyframes.last().map(|frame| frame.position) {
            stage
                .character_positions
                .insert(actor_id.to_string(), final_position);
        }
        complete_missing_animation(animations, animation_id);
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
        complete_missing_animation(animations, animation_id);
        return;
    }

    let mut pending_animation = animation_id;
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

fn character_part_sprite(image: Handle<Image>, part: &CharacterPartDefinition) -> WorldSprite {
    WorldSprite::from_image(image).with_rect(part.rect.map(source_rect_from_corners))
}

pub(super) fn source_rect_from_corners(rect: [f32; 4]) -> [f32; 4] {
    [rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]]
}

pub(super) fn source_rect_to_corners(rect: [f32; 4]) -> [f32; 4] {
    [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]]
}
