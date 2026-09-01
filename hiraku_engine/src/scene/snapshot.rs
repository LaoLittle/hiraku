use super::*;

pub fn sync_scene_snapshot(
    mut shared_state: ResMut<SceneSharedState>,
    stage: Res<StageState>,
    pending_characters: Res<PendingCharacterShows>,
    alpha_mask_materials: Res<Assets<AlphaMaskMaterial>>,
    multiply_materials: Res<Assets<MultiplyMaterial>>,
    dialogue_state: Res<DialogueState>,
    background_layers: Query<&BackgroundLayer>,
    bgms: Query<&BgmChannel>,
    overlay: Query<&WorldSprite, With<OverlayMarker>>,
    sprites: Query<(
        &SpriteActor,
        Option<&WorldSprite>,
        Option<&MeshMaterial3d<AlphaMaskMaterial>>,
        Option<&MeshMaterial3d<MultiplyMaterial>>,
        Option<&CharacterPartVisual>,
        Option<&FocusedActorPart>,
        &Transform,
        &Visibility,
    )>,
) {
    let snapshot = &mut shared_state.0;

    snapshot.background = stage
        .background
        .and_then(|entity| background_layers.get(entity).ok())
        .map(|layer| ImageLayerSnapshot {
            path: layer.path.clone(),
        });

    if stage.pending_character_restore.is_empty() && pending_characters.items.is_empty() {
        let mut sprite_snapshots = sprites
            .iter()
            .filter(|(_, _, _, _, _, _, _, visibility)| **visibility != Visibility::Hidden)
            .map(
                |(actor, sprite, alpha_mask, multiply, visual, focused, transform, _)| {
                    SpriteSnapshot {
                        id: actor.id.clone(),
                        path: actor.path.clone(),
                        x: transform.translation.x,
                        y: transform.translation.y,
                        layer: transform.translation.z - STAGE_Z_SPRITE,
                        scale: transform.scale.x,
                        alpha: sprite
                            .map(|sprite| sprite.color.alpha())
                            .unwrap_or_else(|| {
                                alpha_mask
                                    .and_then(|material| alpha_mask_materials.get(&material.0))
                                    .map(|material| material.tint.w * material.opacity)
                                    .or_else(|| {
                                        multiply
                                            .and_then(|material| {
                                                multiply_materials.get(&material.0)
                                            })
                                            .map(|material| material.tint.w * material.opacity)
                                    })
                                    .unwrap_or(1.0)
                            }),
                        rect: sprite
                            .and_then(|sprite| sprite.rect.map(source_rect_to_corners))
                            .or_else(|| visual.and_then(|visual| visual.rect)),
                        focused: focused.is_some(),
                    }
                },
            )
            .collect::<Vec<_>>();
        sprite_snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshot.sprites = sprite_snapshots;
        snapshot.character_positions = stage
            .character_positions
            .iter()
            .map(|(actor_id, position)| (actor_id.clone(), [position.x, position.y]))
            .collect();
    }

    // Restoring an intro + loop track is a two-phase operation: the snapshot is
    // accepted first and the audio entity is spawned by `reconcile_restored_bgm`
    // on the next runtime tick. Keep the saved track intact during that gap.
    if stage.pending_bgm_restore.is_none() {
        snapshot.bgm = stage.bgm.and_then(|entity| {
            bgms.get(entity).ok().map(|bgm| AudioSnapshot {
                path: bgm.path.clone(),
                volume: bgm.volume,
            })
        });
    }

    if let Ok(overlay_sprite) = overlay.single() {
        snapshot.overlay_alpha = overlay_sprite.color.alpha();
    }

    snapshot.text_effect = text_effect_snapshot(&dialogue_state.effect);
}

pub(super) fn restore_scene_snapshot(
    commands: &mut Commands,
    asset_server: &AssetServer,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    dialogue_root: &mut Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    _user_settings: &UserSettings,
    snapshot: SceneSnapshot,
) {
    let text_effect = snapshot.text_effect.clone();

    if let Some(background) = stage.background.take() {
        commands.entity(background).try_despawn();
    }
    if let Some(effect) = stage.screen_effect.take() {
        commands.entity(effect).try_despawn();
    }
    if let Some(transition) = stage.transition.take() {
        commands.entity(transition).try_despawn();
    }
    for (_, root) in stage.character_roots.drain() {
        commands.entity(root).try_despawn();
    }
    for (_, entity) in stage.sprites.drain() {
        commands.entity(entity).try_despawn();
    }
    stage.character_positions.clear();
    stage.character_active_parts.clear();
    stage.pending_character_restore = snapshot
        .sprites
        .iter()
        .filter(|sprite| sprite.id.starts_with("character::"))
        .cloned()
        .collect();
    if let Some(bgm) = stage.bgm.take() {
        commands.entity(bgm).try_despawn();
    }
    stage.pending_bgm_restore = snapshot.bgm.clone();

    if let Some(background) = snapshot.background.as_ref() {
        stage.background = Some(
            commands
                .spawn((
                    BackgroundLayer {
                        path: background.path.clone(),
                    },
                    WorldSprite::from_image(asset_server.load(background.path.clone())),
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id(),
        );
    }

    for sprite in &snapshot.sprites {
        // Character parts are logical scene state, not plain sprites. A
        // dedicated reconciliation system rebuilds their child hierarchy and
        // mask/blend materials from the catalog on the next update.
        if sprite.id.starts_with("character::") {
            continue;
        }
        let mut entity_sprite = WorldSprite::from_image(asset_server.load(sprite.path.clone()));
        entity_sprite.color.set_alpha(sprite.alpha);
        entity_sprite.rect = sprite.rect.map(source_rect_from_corners);
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite.id.clone(),
                    path: sprite.path.clone(),
                },
                entity_sprite,
                Transform {
                    translation: Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer),
                    scale: Vec3::splat(sprite.scale),
                    ..default()
                },
            ))
            .id();
        if sprite.focused {
            commands
                .entity(entity)
                .try_insert((FocusedActorPart, focus_layer()));
        } else {
            commands.entity(entity).try_insert(scene_layer());
        }
        stage.sprites.insert(sprite.id.clone(), entity);
    }

    stage.character_positions = snapshot
        .character_positions
        .iter()
        .map(|(actor_id, position)| (actor_id.clone(), Vec2::new(position[0], position[1])))
        .collect();

    if let Some(overlay) = stage.overlay {
        commands.entity(overlay).insert(WorldSprite::from_color(
            Color::BLACK.with_alpha(snapshot.overlay_alpha),
            Vec2::new(6000.0, 6000.0),
        ));
    }

    match snapshot.dialogue {
        Some(dialogue) => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Visible;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = dialogue.speaker;
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = dialogue.text;
            }
        }
        None => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Hidden;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = String::new();
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = String::new();
            }
        }
    }

    dialogue_state.effect = dialogue_text_effect_from_snapshot(&text_effect);
    dialogue_state.waiting = None;
    choice_state.waiting = None;
    choice_state.options.clear();
}
