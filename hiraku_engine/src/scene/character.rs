use super::*;
use std::time::Duration;

/// Hide logical actors while keeping their part entities available for reuse.
/// Cancel pending image readiness so it cannot resurrect a hidden actor later.
pub(super) fn hide_character_entities(
    commands: &mut Commands,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    actor: Option<&str>,
    fade_ms: u64,
) {
    let selected = |name: &str| actor.is_none_or(|actor| actor == name);
    pending.items.retain_mut(|item| {
        if !selected(&item.actor_id) {
            return true;
        }
        complete_missing_animation(animations, item.animation_id.take());
        false
    });
    let names = stage
        .character_roots
        .keys()
        .filter(|name| selected(name))
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        if let Some(root) = stage.character_roots.get(&name).copied() {
            super::character_composite::fade_group(
                commands,
                root,
                0.0,
                Duration::from_millis(fade_ms),
                None,
            );
        }
        let prefix = format!("character::{name}::");
        for (id, entity) in &stage.sprites {
            if id.starts_with(&prefix) {
                let entity = *entity;
                commands.queue(move |world: &mut World| {
                    let animation = world
                        .get::<VisualTween>(entity)
                        .and_then(|tween| tween.animation_id.clone());
                    if let Some(animation) = animation {
                        world
                            .resource_mut::<AnimationState>()
                            .completed
                            .insert(animation);
                    }
                    let Ok(mut entity) = world.get_entity_mut(entity) else {
                        return;
                    };
                    entity.remove::<VisualTween>();
                    entity.insert(HideAfterTween);
                    if fade_ms == 0 {
                        entity.insert(Visibility::Hidden);
                    }
                });
            }
        }
        stage.character_active_parts.remove(&name);
        stage.character_positions.remove(&name);
        stage.character_rotations.remove(&name);
    }
    stage.pending_character_restore.retain(|part| {
        actor.is_some_and(|name| !part.id.starts_with(&format!("character::{name}::")))
    });
}

pub fn reconcile_restored_characters(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
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
        let catalog_name = stage
            .character_catalog_names
            .get(&actor_id)
            .unwrap_or(&actor_id);
        let Some(character) = characters.characters.get(catalog_name) else {
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
            None,
        );
    }
}

fn restored_character_actor_id(id: &str) -> Option<&str> {
    id.strip_prefix("character::")?
        .split_once("::")
        .map(|(actor, _)| actor)
}

/// A group fade-out may be interrupted by show. Its completion callback is then
/// replaced, so obsolete children must be retired before reversing group alpha.
fn reconcile_group_reveal(world: &mut World, root: Entity, desired: &HashSet<Entity>) {
    use super::character_composite::LogicalCharacterPart;
    let children = world
        .get::<Children>(root)
        .map(|children| children.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    for child in children {
        if world.get::<LogicalCharacterPart>(child).is_none() {
            continue;
        }
        let animation = world
            .get_mut::<VisualTween>(child)
            .and_then(|mut tween| tween.animation_id.take());
        if let Ok(mut part) = world.get_entity_mut(child) {
            part.remove::<(VisualTween, HideAfterTween)>();
            if !desired.contains(&child) {
                part.insert(Visibility::Hidden);
            }
        }
        complete_missing_animation(&mut world.resource_mut::<AnimationState>(), animation);
    }
}

/// Placement interpolation is independent from per-part opacity transitions.
#[derive(Component, Clone)]
pub(crate) struct CharacterPlacementTween {
    animation: Option<crate::script::AnimationSpec>,
    from: Transform,
    to: Transform,
    timer: Timer,
}

/// Authoritative placement survives replacement of every expression part.
#[derive(Component, Clone)]
pub(crate) struct ActorPlacement {
    pub(super) current: Transform,
    trajectory: Option<CharacterPlacementTween>,
}

impl ActorPlacement {
    pub(super) fn is_animating(&self) -> bool {
        self.trajectory.is_some()
    }
}

// Derive one actor-space trajectory, then project it onto every part, including
// newly appearing and outgoing expression layers. Part identity must not decide
// whether placement animates or teleports.
#[cfg(test)]
fn update_actor_placement(
    world: &mut World,
    root: Entity,
    reference: Option<Entity>,
    position: Vec2,
    scale: f32,
) {
    let rotation = world
        .get::<ActorPlacement>(root)
        .map(|p| {
            p.trajectory
                .as_ref()
                .map_or(p.current.rotation, |t| t.to.rotation)
        })
        .unwrap_or(Quat::IDENTITY);
    update_actor_placement_with_animation(world, root, reference, position, scale, rotation, None);
}

fn update_actor_placement_with_animation(
    world: &mut World,
    root: Entity,
    reference: Option<Entity>,
    position: Vec2,
    scale: f32,
    rotation: Quat,
    animation: Option<crate::script::AnimationSpec>,
) {
    use super::character_composite::LogicalCharacterPart;
    let anchor = |transform: Transform, offset: Vec2| Transform {
        translation: (transform.translation.truncate() - offset * transform.scale.x).extend(0.0),
        scale: transform.scale,
        ..default()
    };
    let target = Transform::from_translation(position.extend(0.0))
        .with_scale(Vec3::splat(scale))
        .with_rotation(rotation);
    let source = world
        .get::<ActorPlacement>(root)
        .map(|placement| (placement.current, placement.trajectory.clone()))
        .or_else(|| {
            reference.and_then(|entity| {
                let offset = world.get::<LogicalCharacterPart>(entity)?.0.offset;
                let current = anchor(*world.get::<Transform>(entity)?, offset);
                let tween = world.get::<CharacterPlacementTween>(entity).map(|tween| {
                    CharacterPlacementTween {
                        animation: tween.animation,
                        from: anchor(tween.from, offset),
                        to: anchor(tween.to, offset),
                        timer: tween.timer.clone(),
                    }
                });
                Some((current, tween))
            })
        });
    let (current, previous) = source.unwrap_or((target, None));
    let at_target = |value: Transform| {
        value.translation.abs_diff_eq(target.translation, 0.0001)
            && value.scale.abs_diff_eq(target.scale, 0.0001)
            && value.rotation.abs_diff_eq(target.rotation, 0.0001)
    };
    let trajectory = if let Some(tween) = previous.filter(|tween| at_target(tween.to)) {
        Some(tween)
    } else if !at_target(current) {
        Some(CharacterPlacementTween {
            animation,
            from: current,
            to: target,
            timer: Timer::new(
                animation
                    .map(|a| Duration::from_secs_f32(a.duration()))
                    .unwrap_or(Duration::from_millis(300)),
                TimerMode::Once,
            ),
        })
    } else {
        None
    };
    world.entity_mut(root).insert(ActorPlacement {
        current,
        trajectory: trajectory.clone(),
    });
    let children = world
        .get::<Children>(root)
        .map(|c| c.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    for child in children {
        let Some(part) = world.get::<LogicalCharacterPart>(child) else {
            continue;
        };
        let offset = part.0.offset;
        let depth = character_depth(part.0.layer);
        let project = |actor: Transform| Transform {
            translation: (actor.translation.truncate() + offset * actor.scale.x).extend(depth),
            scale: actor.scale,
            ..default()
        };
        let Ok(mut entity) = world.get_entity_mut(child) else {
            continue;
        };
        if let Some(tween) = &trajectory {
            entity.insert((
                project(current),
                CharacterPlacementTween {
                    animation: tween.animation,
                    from: project(tween.from),
                    to: project(tween.to),
                    timer: tween.timer.clone(),
                },
            ));
        } else {
            entity.remove::<CharacterPlacementTween>();
            entity.insert(project(target));
        }
    }
}

pub fn animate_character_motion_effects(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    time: crate::scene::playback::StoryTime,
    mut animations: ResMut<AnimationState>,
    mut stage: ResMut<StageState>,
    mut placements: Query<&mut ActorPlacement>,
    mut movers: Query<
        (
            Entity,
            &'static mut Transform,
            Option<&'static mut CharacterJumpEffect>,
            Option<&'static mut CharacterShakeEffect>,
            Option<&'static mut CharacterTimelineEffect>,
            Option<&'static mut CharacterPlacementTween>,
        ),
        (
            Without<WorldCamera>,
            Or<(
                With<CharacterJumpEffect>,
                With<CharacterShakeEffect>,
                With<CharacterTimelineEffect>,
                With<CharacterPlacementTween>,
            )>,
        ),
    >,
) {
    if !movers.is_empty() {
        redraw.request();
    }
    for mut placement in &mut placements {
        if let Some(tween) = placement.trajectory.as_mut() {
            redraw.request();
            tween.timer.tick(time.delta());
            let t = tween
                .animation
                .map(|a| a.sample(tween.timer.fraction()))
                .unwrap_or_else(|| 1.0 - (1.0 - tween.timer.fraction()).powi(3));
            let current = Transform {
                translation: tween.from.translation.lerp(tween.to.translation, t),
                scale: tween.from.scale.lerp(tween.to.scale, t),
                rotation: tween.from.rotation.slerp(tween.to.rotation, t),
                ..default()
            };
            let finished = tween.timer.is_finished();
            placement.current = current;
            if finished {
                placement.trajectory = None;
            }
        }
    }
    for (entity, mut transform, mut jump, mut shake, timeline, placement) in &mut movers {
        let mut placement_origin = None;
        if let Some(mut placement) = placement {
            placement.timer.tick(time.delta());
            let t = placement
                .animation
                .map(|a| a.sample(placement.timer.fraction()))
                .unwrap_or_else(|| 1.0 - (1.0 - placement.timer.fraction()).powi(3));
            let origin = placement.from.translation.lerp(placement.to.translation, t);
            transform.scale = placement.from.scale.lerp(placement.to.scale, t);
            transform.rotation = placement.from.rotation.slerp(placement.to.rotation, t);
            placement_origin = Some(origin);
            if let Some(effect) = jump.as_mut() {
                effect.origin = origin;
            }
            if let Some(effect) = shake.as_mut() {
                effect.origin = origin;
            }
            if placement.timer.is_finished() {
                commands
                    .entity(entity)
                    .try_remove::<CharacterPlacementTween>();
            }
        }
        let base_origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin))
            .or(placement_origin)
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
                Has<super::character_composite::LogicalCharacterPart>,
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
                    .is_ok_and(|(sprite, mesh, material, logical)| {
                        logical || sprite.is_none() || (mesh.is_some() && material.is_some())
                    })
            }) {
                return true;
            }

            completed.push((
                item.actor_id.clone(),
                item.whole_actor,
                item.entities.clone(),
                std::mem::take(&mut item.outgoing),
                item.fade,
                item.animation_id.take(),
            ));
            false
        });
    }

    let mut visuals = visual_queries.p1();
    for (actor_id, whole_actor, entities, outgoing, fade, animation_id) in completed {
        let mut pending_animation = animation_id;
        if whole_actor {
            if let Some(root) = stage.character_roots.get(&actor_id).copied() {
                let desired = stage
                    .character_active_parts
                    .get(&actor_id)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| stage.sprites.get(id).copied())
                    .collect();
                commands
                    .queue(move |world: &mut World| reconcile_group_reveal(world, root, &desired));
                super::character_composite::fade_group(
                    &mut commands,
                    root,
                    1.0,
                    fade.unwrap_or(Duration::ZERO),
                    pending_animation.take(),
                );
            }
        }
        for (index, entity) in entities.into_iter().enumerate() {
            if let Ok((visual, sprite, alpha_mask, multiply, mut visibility)) =
                visuals.get_mut(entity)
            {
                *visibility = Visibility::Visible;
                if whole_actor {
                    set_character_part_alpha(
                        visual,
                        sprite,
                        alpha_mask,
                        multiply,
                        &mut alpha_mask_materials,
                        &mut multiply_materials,
                        1.0,
                    );
                } else if let Some(fade) = fade {
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
            // A later commit may have selected this part again while loading.
            if stage
                .character_active_parts
                .get(&actor_id)
                .is_some_and(|ids| ids.contains(&id))
            {
                continue;
            }
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
    _meshes: &mut Assets<Mesh>,
    _alpha_mask_materials: &mut Assets<AlphaMaskMaterial>,
    _multiply_materials: &mut Assets<MultiplyMaterial>,
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
    placement_animation: Option<crate::script::AnimationSpec>,
) {
    const DEFAULT_CHARACTER_FADE: std::time::Duration = std::time::Duration::from_millis(120);

    // Re-entry resets placement below, but opacity still transitions normally.
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
                    super::character_composite::CharacterGroup::default(),
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
    if active_ids.is_empty() {
        stage.character_order.retain(|name| name != &actor_id);
        stage.character_order.push(actor_id.clone());
        let count = stage.character_order.len().max(1) as f32;
        for (index, id) in stage.character_order.iter().enumerate() {
            if let Some(entity) = stage.character_roots.get(id).copied() {
                let depth = index as f32 / count;
                commands.queue(move |world: &mut World| {
                    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                        transform.translation.z = depth;
                    }
                });
            }
        }
        // A re-entry has no visible source pose to interpolate from.
        commands.entity(root).remove::<ActorPlacement>();
        let retained = stage
            .sprites
            .iter()
            .filter(|(id, _)| id.starts_with(&format!("character::{actor_id}::")))
            .map(|(_, entity)| *entity)
            .collect::<Vec<_>>();
        for entity in retained {
            commands.entity(entity).remove::<CharacterPlacementTween>();
        }
    }
    // A newer statement may replace the initial show before its atlas loads.
    // Preserve the group fade-in even if that pending show loses every part.
    let whole_actor = active_ids.is_empty()
        || pending
            .items
            .iter()
            .any(|item| item.actor_id == actor_id && item.whole_actor);
    let reference = active_ids
        .iter()
        .filter_map(|id| stage.sprites.get(id).map(|entity| (id, *entity)))
        .min_by_key(|(id, _)| *id)
        .map(|(_, entity)| entity);

    // A previous statement can still be waiting for its images. Retain only
    // parts that are also present in the newly committed actor state.
    let mut superseded_outgoing = Vec::new();
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
            superseded_outgoing.append(&mut item.outgoing);
            complete_missing_animation(animations, item.animation_id.take());
            false
        } else {
            true
        }
    });

    let existing_ids = active_ids
        .iter()
        .cloned()
        .chain(superseded_outgoing.into_iter().map(|(id, _)| id))
        .collect::<HashSet<_>>();
    let new_part_count = parts
        .iter()
        .filter(|part| !active_ids.contains(&character_part_id(&actor_id, part)))
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
            // Cancel an interrupted fade-out before reusing the cached part.
            // Removing only HideAfterTween would leave a tween driving alpha to 0.
            commands.queue(move |world: &mut World| {
                let old = world
                    .get_mut::<VisualTween>(entity)
                    .and_then(|mut tween| tween.animation_id.take());
                if let Ok(mut part) = world.get_entity_mut(entity) {
                    part.remove::<VisualTween>();
                }
                complete_missing_animation(&mut world.resource_mut::<AnimationState>(), old);
            });
            entities.push(entity);
            entity_ids.push(sprite_id);
            handles.push(load_part_source(asset_server, part));
            newly_spawned.push(false);
            continue;
        }
        let transform = Transform {
            translation: Vec3::new(
                position.x + part.offset.x * scale,
                position.y + part.offset.y * scale,
                character_depth(part.layer),
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

        let handle = load_part_source(asset_server, part);
        let cpu = part.pack_source.then(|| {
            handle
                .clone()
                .typed::<super::character_composite::source::AtlasSource>()
        });
        let mut sprite = if part.pack_source {
            WorldSprite::from_color(Color::WHITE, Vec2::ZERO)
                .with_rect(part.rect.map(source_rect_from_corners))
        } else {
            character_part_sprite(handle.clone().typed::<Image>(), part)
        };
        sprite.color = color;
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite_id.clone(),
                    path: part.path.clone(),
                },
                sprite,
                visual,
                super::character_composite::LogicalCharacterPart(part.clone(), cpu),
                Visibility::Hidden,
                transform,
            ))
            .id();

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

    let rotation = Quat::from_rotation_z(
        stage
            .character_rotations
            .get(&actor_id)
            .copied()
            .unwrap_or(0.0)
            .to_radians(),
    );
    commands.queue(move |world: &mut World| {
        update_actor_placement_with_animation(
            world,
            root,
            reference,
            position,
            scale,
            rotation,
            placement_animation,
        )
    });
    if entities.is_empty() {
        stage.character_active_parts.insert(actor_id, desired_ids);
        complete_missing_animation(animations, pending_animation);
        return;
    }

    stage
        .character_active_parts
        .insert(actor_id.clone(), desired_ids);
    pending.items.push(PendingCharacterShow {
        whole_actor,
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

#[cfg(test)]
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
        CharacterEase::EaseOutSine => (t * std::f32::consts::FRAC_PI_2).sin(),
        CharacterEase::EaseInOutSine => (1.0 - (t * std::f32::consts::PI).cos()) * 0.5,
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

// Descriptor layers are local sorting units, not world-space distances.
// Keep ordinary part orders within the character band below the scene curtain.
fn character_depth(layer: f32) -> f32 {
    STAGE_Z_SPRITE + layer * 0.001
}

fn load_part_source(server: &AssetServer, part: &CharacterPartDefinition) -> UntypedHandle {
    if part.pack_source {
        server
            .load::<super::character_composite::source::AtlasSource>(part.path.clone())
            .untyped()
    } else {
        crate::texture::load_static_image(server, part.path.clone()).untyped()
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
    fn hiding_an_actor_keeps_cached_children_but_cancels_pending_reveal() {
        let mut app = App::new();
        app.init_resource::<StageState>()
            .init_resource::<PendingCharacterShows>()
            .init_resource::<AnimationState>();
        let alice = app.world_mut().spawn(Visibility::Visible).id();
        let bob = app.world_mut().spawn(Visibility::Visible).id();
        let alice_root = app.world_mut().spawn_empty().id();
        let bob_root = app.world_mut().spawn_empty().id();
        {
            let mut stage = app.world_mut().resource_mut::<StageState>();
            stage
                .character_roots
                .extend([("alice".into(), alice_root), ("bob".into(), bob_root)]);
            stage.sprites.extend([
                ("character::alice::body".into(), alice),
                ("character::bob::body".into(), bob),
            ]);
        }
        app.world_mut()
            .resource_mut::<PendingCharacterShows>()
            .items
            .push(PendingCharacterShow {
                whole_actor: false,
                actor_id: "alice".into(),
                entity_ids: vec!["character::alice::body".into()],
                entities: vec![alice],
                handles: vec![],
                newly_spawned: vec![false],
                outgoing: vec![],
                fade: None,
                animation_id: Some("reveal".into()),
            });
        app.add_systems(
            Update,
            |mut commands: Commands,
             mut stage: ResMut<StageState>,
             mut pending: ResMut<PendingCharacterShows>,
             mut animations: ResMut<AnimationState>| {
                hide_character_entities(
                    &mut commands,
                    &mut stage,
                    &mut pending,
                    &mut animations,
                    Some("alice"),
                    0,
                );
            },
        );
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(alice),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<Visibility>(bob),
            Some(&Visibility::Visible)
        );
        assert!(
            app.world()
                .resource::<PendingCharacterShows>()
                .items
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<AnimationState>()
                .completed
                .contains("reveal")
        );
        assert!(character_depth(131.0) < STAGE_Z_OVERLAY);
    }

    #[test]
    fn zero_duration_pose_does_not_disable_later_default_tween() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<AnimationState>()
            .init_resource::<StageState>()
            .add_systems(Update, animate_character_motion_effects);
        let root = app.world_mut().spawn_empty().id();
        let part = spawn_placement_part(app.world_mut(), root, "alice/body", Vec2::ZERO, 0.0);
        update_actor_placement_with_animation(
            app.world_mut(),
            root,
            Some(part),
            Vec2::new(100.0, 0.0),
            1.0,
            Quat::IDENTITY,
            Some(crate::script::AnimationSpec::Linear(0.0, false)),
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(16));
        app.update();
        assert_eq!(
            app.world()
                .get::<Transform>(part)
                .expect("pose")
                .translation
                .x,
            100.0
        );
        update_actor_placement_with_animation(
            app.world_mut(),
            root,
            Some(part),
            Vec2::new(200.0, 0.0),
            1.0,
            Quat::IDENTITY,
            None,
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(150));
        app.update();
        let x = app
            .world()
            .get::<Transform>(part)
            .expect("pose")
            .translation
            .x;
        assert!(
            (x - 187.5).abs() < 0.01,
            "default ease-out must remain active: {x}"
        );
    }

    #[test]
    fn explicit_placement_animation_interpolates_position_and_scale_together() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<AnimationState>()
            .init_resource::<StageState>()
            .add_systems(Update, animate_character_motion_effects);
        let root = app.world_mut().spawn_empty().id();
        let part = spawn_placement_part(app.world_mut(), root, "alice/body", Vec2::ZERO, 0.0);
        update_actor_placement_with_animation(
            app.world_mut(),
            root,
            Some(part),
            Vec2::new(100.0, 200.0),
            2.0,
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Some(crate::script::AnimationSpec::Linear(1.2, false)),
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(600));
        app.update();
        let pose = app.world().get::<ActorPlacement>(root).expect("placement");
        assert!(
            pose.current
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 0.001)
        );
        assert!((pose.current.translation.x - 50.0).abs() < 0.001);
        assert!((pose.current.translation.y - 100.0).abs() < 0.001);
        assert!((pose.current.scale.x - 1.5).abs() < 0.001);
        assert!(pose.is_animating());
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(601));
        app.update();
        assert!(
            !app.world()
                .get::<ActorPlacement>(root)
                .expect("placement")
                .is_animating()
        );
    }

    #[test]
    fn placement_uses_ease_out_and_repeated_commits_do_not_restart_it() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<AnimationState>()
            .init_resource::<StageState>()
            .add_systems(Update, animate_character_motion_effects);
        let root = app.world_mut().spawn_empty().id();
        let entity = spawn_placement_part(app.world_mut(), root, "alice/body", Vec2::ZERO, 0.0);
        let target = Transform::from_xyz(100.0, 0.0, 0.0);
        update_actor_placement(
            app.world_mut(),
            root,
            Some(entity),
            target.translation.truncate(),
            1.0,
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(150));
        app.update();
        let position = app
            .world()
            .get::<Transform>(entity)
            .expect("transform")
            .translation
            .x;
        assert!((position - 87.5).abs() < 0.01);
        // An incoming expression and its fading predecessor must both inherit
        // the same elapsed trajectory rather than appearing at the destination.
        let incoming = spawn_placement_part(
            app.world_mut(),
            root,
            "alice/eyes",
            Vec2::new(10.0, 20.0),
            100.0,
        );
        app.world_mut().entity_mut(entity).insert(HideAfterTween);
        update_actor_placement(
            app.world_mut(),
            root,
            Some(entity),
            target.translation.truncate(),
            1.0,
        );
        let incoming_position = app
            .world()
            .get::<Transform>(incoming)
            .expect("incoming part")
            .translation;
        assert!((incoming_position.x - 97.5).abs() < 0.01);
        assert_eq!(incoming_position.y, 20.0);
        assert_eq!(
            app.world()
                .get::<CharacterPlacementTween>(incoming)
                .expect("inherited tween")
                .timer
                .elapsed(),
            Duration::from_millis(150)
        );
        assert!(
            app.world().get::<VisualTween>(incoming).is_none(),
            "placement must not create alpha fades"
        );
        assert_eq!(
            app.world()
                .get::<CharacterPlacementTween>(entity)
                .expect("same tween")
                .timer
                .elapsed(),
            std::time::Duration::from_millis(150)
        );
        app.update();
        assert_eq!(
            app.world()
                .get::<Transform>(entity)
                .expect("target transform")
                .translation,
            Vec3::new(100.0, 0.0, character_depth(0.0))
        );
        assert_eq!(
            app.world()
                .get::<Transform>(incoming)
                .expect("incoming target")
                .translation,
            Vec3::new(110.0, 20.0, character_depth(0.0))
        );
        assert!(app.world().get::<CharacterPlacementTween>(entity).is_none());
    }

    fn spawn_placement_part(
        world: &mut World,
        root: Entity,
        id: &str,
        offset: Vec2,
        actor_x: f32,
    ) -> Entity {
        world
            .spawn((
                super::super::character_composite::LogicalCharacterPart(
                    CharacterPartDefinition {
                        id: id.into(),
                        slot: None,
                        path: "atlas.png".into(),
                        pack_source: false,
                        atlas_rect: None,
                        offset,
                        layer: 0.0,
                        rect: None,
                        mask: None,
                        blend: CharacterBlendMode::Normal,
                        color: [255; 4],
                    },
                    None,
                ),
                Transform::from_xyz(actor_x + offset.x, offset.y, character_depth(0.0)),
                ChildOf(root),
            ))
            .id()
    }

    #[test]
    fn relative_motion_projects_all_parts_without_accumulating_or_restarting() {
        use crate::script::actor_motion::{ActorMotion, ActorOffset};
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<StageState>()
            .init_resource::<AnimationState>()
            .init_resource::<SceneSharedState>()
            .add_systems(Update, super::super::actor_motion::animate);
        let root = app
            .world_mut()
            .spawn(ActorPlacement {
                current: Transform::from_xyz(100.0, 20.0, 0.0),
                trajectory: None,
            })
            .id();
        let outgoing = spawn_placement_part(app.world_mut(), root, "old", Vec2::ZERO, 100.0);
        let incoming =
            spawn_placement_part(app.world_mut(), root, "new", Vec2::new(5.0, 10.0), 100.0);
        {
            let mut stage = app.world_mut().resource_mut::<StageState>();
            stage.character_roots.insert("alice".into(), root);
            stage
                .character_active_parts
                .entry("alice".into())
                .or_default();
        }
        app.world_mut()
            .resource_mut::<SceneSharedState>()
            .0
            .actor_motions
            .insert(
                "alice".into(),
                ActorMotion::new(
                    1,
                    ActorOffset {
                        target: [0.0, 20.0],
                        animation: crate::script::AnimationSpec::Linear(1.0, false),
                    },
                    [0.0; 2],
                ),
            );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.25));
        app.update();
        assert_eq!(
            app.world()
                .get::<Transform>(outgoing)
                .expect("outgoing part")
                .translation
                .y,
            25.0
        );
        assert_eq!(
            app.world()
                .get::<Transform>(incoming)
                .expect("incoming part")
                .translation
                .y,
            35.0
        );
        // A base move and a newly mounted expression must sample the same clock.
        app.world_mut()
            .get_mut::<ActorPlacement>(root)
            .expect("placement")
            .current
            .translation
            .x = 140.0;
        let added = spawn_placement_part(app.world_mut(), root, "added", Vec2::ZERO, 140.0);
        app.update();
        for entity in [outgoing, added] {
            let transform = app
                .world()
                .get::<Transform>(entity)
                .expect("projected part");
            assert_eq!(transform.translation.truncate(), Vec2::new(140.0, 30.0));
        }
    }

    #[test]
    fn show_during_group_hide_retires_obsolete_expression_layers() {
        use bevy::ecs::world::CommandQueue;
        let mut app = App::new();
        app.init_resource::<AnimationState>();
        let root = app
            .world_mut()
            .spawn(super::super::character_composite::CharacterGroup::default())
            .id();
        let body = spawn_placement_part(app.world_mut(), root, "alice/body", Vec2::ZERO, 0.0);
        let old = spawn_placement_part(app.world_mut(), root, "alice/eyes_a", Vec2::ZERO, 0.0);
        let new = spawn_placement_part(app.world_mut(), root, "alice/eyes_b", Vec2::ZERO, 0.0);
        for entity in [body, old] {
            app.world_mut()
                .entity_mut(entity)
                .insert(Visibility::Visible);
        }
        app.world_mut().entity_mut(new).insert(Visibility::Hidden);
        let mut stage = StageState::default();
        stage.character_roots.insert("alice".into(), root);
        for (name, entity) in [("body", body), ("eyes_a", old), ("eyes_b", new)] {
            stage
                .sprites
                .insert(format!("character::alice::{name}"), entity);
        }
        stage.character_active_parts.insert(
            "alice".into(),
            HashSet::from([
                "character::alice::body".into(),
                "character::alice::eyes_a".into(),
            ]),
        );
        let mut queue = CommandQueue::default();
        hide_character_entities(
            &mut Commands::new(&mut queue, app.world()),
            &mut stage,
            &mut PendingCharacterShows::default(),
            &mut AnimationState::default(),
            None,
            300,
        );
        queue.apply(app.world_mut());
        assert_eq!(
            *app.world()
                .get::<Visibility>(old)
                .expect("outgoing visibility"),
            Visibility::Visible
        );
        assert!(app.world().get::<HideAfterTween>(old).is_some());
        assert!(!stage.character_active_parts.contains_key("alice"));

        // The new show is ready before the 300 ms group fade has completed.
        // Shared parts stay visible, but no old expression can be resurrected.
        reconcile_group_reveal(app.world_mut(), root, &HashSet::from([body, new]));
        assert_eq!(
            *app.world()
                .get::<Visibility>(old)
                .expect("retired visibility"),
            Visibility::Hidden
        );
        assert_eq!(
            *app.world()
                .get::<Visibility>(body)
                .expect("shared visibility"),
            Visibility::Visible
        );
        for entity in [body, old, new] {
            assert!(app.world().get::<HideAfterTween>(entity).is_none());
            assert!(app.world().get::<VisualTween>(entity).is_none());
        }
        assert!(
            app.world().get_entity(old).is_ok(),
            "retired parts remain cached"
        );
    }

    #[test]
    fn replacing_pending_expression_retains_retirement_and_actor_trajectory() {
        use super::super::character_composite::LogicalCharacterPart;
        use bevy::ecs::world::CommandQueue;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<AnimationState>();
        let root = app.world_mut().spawn_empty().id();
        let first = spawn_placement_part(app.world_mut(), root, "alice/eyes_a", Vec2::ZERO, 0.0);
        let first_part = app
            .world()
            .get::<LogicalCharacterPart>(first)
            .expect("part")
            .0
            .clone();
        let first_id = character_part_id("alice", &first_part);
        let mut stage = StageState::default();
        stage.character_roots.insert("alice".into(), root);
        stage.sprites.insert(first_id.clone(), first);
        stage
            .character_active_parts
            .insert("alice".into(), HashSet::from([first_id.clone()]));
        let server = app.world().resource::<AssetServer>().clone();
        let mut pending = PendingCharacterShows::default();
        let mut animations = AnimationState::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut masks = Assets::<AlphaMaskMaterial>::default();
        let mut blends = Assets::<MultiplyMaterial>::default();
        for name in ["alice/eyes_b", "alice/eyes_c"] {
            let mut part = first_part.clone();
            part.id = name.into();
            let mut queue = CommandQueue::default();
            let mut commands = Commands::new(&mut queue, app.world());
            queue_character_show(
                &mut commands,
                &server,
                &mut meshes,
                &mut masks,
                &mut blends,
                &mut stage,
                &mut pending,
                &mut animations,
                "alice".into(),
                vec![part],
                Vec2::new(100.0, 0.0),
                1.0,
                false,
                None,
                None,
                None,
            );
            queue.apply(app.world_mut());
        }
        assert_eq!(pending.items.len(), 1);
        assert_eq!(pending.items[0].outgoing, vec![(first_id, first)]);
        let placement = app
            .world()
            .get::<ActorPlacement>(root)
            .expect("actor owns motion");
        assert_eq!(placement.current.translation.x, 0.0);
        assert_eq!(
            placement
                .trajectory
                .as_ref()
                .expect("still moving")
                .to
                .translation
                .x,
            100.0
        );
        let incoming = pending.items[0].entities[0];
        assert_eq!(
            app.world()
                .get::<Transform>(incoming)
                .expect("incoming")
                .translation
                .x,
            0.0
        );
        assert!(
            app.world()
                .get::<CharacterPlacementTween>(incoming)
                .is_some()
        );
    }

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
