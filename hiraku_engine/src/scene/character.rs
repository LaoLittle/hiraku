use super::*;

pub fn reconcile_restored_characters(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    texture_atlases: Res<TextureAtlasCatalog>,
    characters: Res<CharacterCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut alpha_mask_materials: ResMut<Assets<AlphaMaskMaterial>>,
    mut multiply_materials: ResMut<Assets<MultiplyMaterial>>,
    mut stage: ResMut<StageState>,
    mut pending: ResMut<PendingCharacterShows>,
    mut animations: ResMut<AnimationState>,
) {
    if stage.pending_character_restore.is_empty() {
        return;
    }

    let snapshots = std::mem::take(&mut stage.pending_character_restore);
    let mut actors = BTreeMap::<String, Vec<SpriteSnapshot>>::new();
    for snapshot in snapshots {
        let Some(actor_id) = restored_character_actor_id(&snapshot.id) else {
            continue;
        };
        actors
            .entry(actor_id.to_string())
            .or_default()
            .push(snapshot);
    }

    for (actor_id, snapshots) in actors {
        let Some(character) = characters.characters.get(&actor_id) else {
            warn!("restored character `{actor_id}` is not present in the character catalog");
            continue;
        };
        let parts = character
            .parts
            .iter()
            .filter(|part| {
                let id = character_part_id(&actor_id, part);
                snapshots.iter().any(|snapshot| snapshot.id == id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if parts.is_empty() {
            warn!("restored character `{actor_id}` contains no recognized parts");
            continue;
        }

        let scale = snapshots.first().map_or(1.0, |snapshot| snapshot.scale);
        let position = stage
            .character_positions
            .get(&actor_id)
            .copied()
            .unwrap_or_else(|| {
                let snapshot = &snapshots[0];
                let part = parts
                    .iter()
                    .find(|part| character_part_id(&actor_id, part) == snapshot.id)
                    .expect("a restored snapshot must have a matching character part");
                Vec2::new(
                    snapshot.x - part.offset.x * scale,
                    snapshot.y - part.offset.y * scale,
                )
            });
        let focused = snapshots.iter().any(|snapshot| snapshot.focused);

        queue_character_show(
            &mut commands,
            &asset_server,
            &texture_atlases,
            &mut meshes,
            &mut alpha_mask_materials,
            &mut multiply_materials,
            &mut stage,
            &mut pending,
            &mut animations,
            actor_id,
            parts,
            position,
            scale,
            focused,
            Some(std::time::Duration::ZERO),
            None,
        );
    }
}

fn restored_character_actor_id(id: &str) -> Option<&str> {
    id.strip_prefix("character::")?
        .split_once("::")
        .map(|(actor, _)| actor)
}

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
                for ((id, entity), newly_spawned) in item
                    .entity_ids
                    .drain(..)
                    .zip(item.entities.drain(..))
                    .zip(item.newly_spawned.drain(..))
                {
                    if newly_spawned {
                        if stage.sprites.get(&id) == Some(&entity) {
                            stage.sprites.remove(&id);
                        }
                        commands.entity(entity).try_despawn();
                    } else {
                        commands.entity(entity).try_insert(Visibility::Hidden);
                    }
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

        for (_id, entity) in outgoing {
            if let Some(fade) = fade {
                commands.entity(entity).try_insert((
                    HideAfterTween,
                    VisualTween {
                        from_alpha: Some(1.0),
                        to_alpha: Some(0.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(fade, TimerMode::Once),
                        animation_id: None,
                        despawn_on_finish: false,
                    },
                ));
            } else {
                commands
                    .entity(entity)
                    .try_insert(Visibility::Hidden)
                    .try_remove::<HideAfterTween>();
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
    let root = stage
        .character_roots
        .get(&actor_id)
        .copied()
        .unwrap_or_else(|| {
            let root = commands
                .spawn((
                    CharacterRoot {
                        actor_id: actor_id.clone(),
                    },
                    Transform::default(),
                    Visibility::Inherited,
                ))
                .id();
            stage.character_roots.insert(actor_id.clone(), root);
            root
        });
    let desired_ids = parts
        .iter()
        .map(|part| character_part_id(&actor_id, part))
        .collect::<HashSet<_>>();
    let active_ids = stage
        .character_active_parts
        .get(&actor_id)
        .cloned()
        .unwrap_or_default();

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
            let newly_spawned = item.newly_spawned.remove(index);
            if newly_spawned {
                if stage.sprites.get(&id) == Some(&entity) {
                    stage.sprites.remove(&id);
                }
                commands.entity(entity).try_despawn();
            } else {
                commands.entity(entity).try_insert(Visibility::Hidden);
            }
        }
        if item.entities.is_empty() {
            complete_missing_animation(animations, item.animation_id.take());
            false
        } else {
            true
        }
    });

    let existing_ids = active_ids.iter().cloned().collect::<Vec<_>>();
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
        commands.entity(entity).try_insert((
            HideAfterTween,
            VisualTween {
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
                despawn_on_finish: false,
            },
        ));
    }

    let mut entities = Vec::new();
    let mut entity_ids = Vec::new();
    let mut handles = Vec::new();
    let mut newly_spawned = Vec::new();

    for part in &parts {
        let sprite_id = character_part_id(&actor_id, part);
        if let Some(entity) = stage.sprites.get(&sprite_id).copied() {
            commands.entity(root).add_child(entity);
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
            if active_ids.contains(&sprite_id) {
                continue;
            }
            entity_commands.try_remove::<HideAfterTween>();
            entities.push(entity);
            entity_ids.push(sprite_id);
            handles.push(asset_server.load(part.path.clone()));
            newly_spawned.push(false);
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
        commands.entity(root).add_child(entity);
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
        newly_spawned.push(true);
    }

    if entities.is_empty() {
        stage.character_active_parts.insert(actor_id, desired_ids);
        complete_missing_animation(animations, pending_animation);
        return;
    }

    stage
        .character_active_parts
        .insert(actor_id.clone(), desired_ids);
    pending.items.push(PendingCharacterShow {
        actor_id,
        entity_ids,
        entities,
        handles,
        newly_spawned,
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
        stage.sprites.remove(&id);
    }
    if let Some(root) = stage.character_roots.remove(actor_id) {
        commands.entity(root).try_despawn();
    }
    stage.character_active_parts.remove(actor_id);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_character_ids_recover_the_logical_actor() {
        assert_eq!(
            restored_character_actor_id("character::alice::slot-001-eyes_open"),
            Some("alice")
        );
        assert_eq!(restored_character_actor_id("sprite::alice"), None);
    }

    #[test]
    fn faded_character_parts_are_hidden_and_retained_for_reuse() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<AnimationState>()
            .init_resource::<Assets<AlphaMaskMaterial>>()
            .init_resource::<Assets<MultiplyMaterial>>()
            .add_systems(Update, animate_visual_tweens);
        let entity = app
            .world_mut()
            .spawn((
                Transform::default(),
                Visibility::Visible,
                HideAfterTween,
                VisualTween {
                    from_alpha: None,
                    to_alpha: None,
                    from_translation: None,
                    to_translation: None,
                    from_scale: None,
                    to_scale: None,
                    timer: Timer::from_seconds(0.0, TimerMode::Once),
                    animation_id: None,
                    despawn_on_finish: false,
                },
            ))
            .id();

        app.update();

        assert!(app.world().get_entity(entity).is_ok());
        assert_eq!(
            app.world().get::<Visibility>(entity),
            Some(&Visibility::Hidden)
        );
        assert!(app.world().get::<VisualTween>(entity).is_none());
        assert!(app.world().get::<HideAfterTween>(entity).is_none());
    }
}
