use super::*;

#[derive(Resource, Default)]
pub struct PendingWaits {
    pub items: Vec<PendingWait>,
}

#[derive(Resource, Default)]
pub struct AnimationState {
    pub completed: HashSet<String>,
    pub waits: Vec<PendingAnimationWait>,
}

#[derive(Resource, Default)]
pub struct PendingAnimationCancels {
    pub ids: Vec<String>,
}

pub struct PendingWait {
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: ScriptRequestId,
}

pub struct PendingAnimationWait {
    pub ids: Vec<String>,
    pub done: ScriptRequestId,
}

#[derive(Component)]
pub struct VisualTween {
    pub from_alpha: Option<f32>,
    pub to_alpha: Option<f32>,
    pub from_translation: Option<Vec3>,
    pub to_translation: Option<Vec3>,
    pub from_scale: Option<Vec3>,
    pub to_scale: Option<Vec3>,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub despawn_on_finish: bool,
}

pub fn tick_pending_waits(
    time: Res<Time>,
    mut waits: ResMut<PendingWaits>,
    mut animations: ResMut<AnimationState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    for wait in waits.items.iter_mut() {
        wait.timer.tick(time.delta());
    }
    let mut completed = Vec::new();
    waits.items.retain(|wait| {
        if wait.timer.is_finished() {
            completed.push((wait.animation_id.clone(), wait.done.clone()));
            false
        } else {
            true
        }
    });
    for (animation_id, done) in completed {
        complete_missing_animation(&mut animations, animation_id);
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Continue,
        });
    }
}

pub fn animate_visual_tweens(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut alpha_mask_materials: ResMut<Assets<AlphaMaskMaterial>>,
    mut multiply_materials: ResMut<Assets<MultiplyMaterial>>,
    mut visuals: Query<(
        Entity,
        Option<&mut WorldSprite>,
        Option<&MeshMaterial3d<AlphaMaskMaterial>>,
        Option<&MeshMaterial3d<MultiplyMaterial>>,
        Option<&CharacterPartVisual>,
        &mut Transform,
        &mut VisualTween,
        Option<&HideAfterTween>,
        Option<&mut Visibility>,
    )>,
) {
    for (
        entity,
        mut sprite,
        alpha_mask,
        multiply,
        part_visual,
        mut transform,
        mut tween,
        hide_after,
        visibility,
    ) in &mut visuals
    {
        tween.timer.tick(time.delta());
        let fraction = tween_fraction(&tween.timer);
        if let (Some(from), Some(to)) = (tween.from_alpha, tween.to_alpha) {
            set_visual_alpha(
                sprite.as_deref_mut(),
                alpha_mask,
                multiply,
                part_visual,
                &mut alpha_mask_materials,
                &mut multiply_materials,
                from + (to - from) * fraction,
            );
        }
        if let (Some(from), Some(to)) = (tween.from_translation, tween.to_translation) {
            transform.translation = from.lerp(to, fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_scale, tween.to_scale) {
            transform.scale = from.lerp(to, fraction);
        }
        if tween.timer.is_finished() {
            if let Some(to) = tween.to_alpha {
                set_visual_alpha(
                    sprite.as_deref_mut(),
                    alpha_mask,
                    multiply,
                    part_visual,
                    &mut alpha_mask_materials,
                    &mut multiply_materials,
                    to,
                );
            }
            if let Some(to) = tween.to_translation {
                transform.translation = to;
            }
            if let Some(to) = tween.to_scale {
                transform.scale = to;
            }
            complete_missing_animation(&mut animations, tween.animation_id.take());
            if tween.despawn_on_finish {
                commands.entity(entity).try_despawn();
            } else {
                if hide_after.is_some() {
                    if let Some(mut visibility) = visibility {
                        *visibility = Visibility::Hidden;
                    }
                    commands.entity(entity).try_remove::<HideAfterTween>();
                }
                commands.entity(entity).try_remove::<VisualTween>();
            }
        }
    }
}

fn set_visual_alpha(
    sprite: Option<&mut WorldSprite>,
    alpha_mask: Option<&MeshMaterial3d<AlphaMaskMaterial>>,
    multiply: Option<&MeshMaterial3d<MultiplyMaterial>>,
    part_visual: Option<&CharacterPartVisual>,
    alpha_mask_materials: &mut Assets<AlphaMaskMaterial>,
    multiply_materials: &mut Assets<MultiplyMaterial>,
    alpha: f32,
) {
    if let Some(sprite) = sprite {
        sprite.color.set_alpha(
            part_visual
                .map(|visual| visual.base_alpha * alpha)
                .unwrap_or(alpha),
        );
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

pub fn animate_rule_transitions(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut rule_materials: ResMut<Assets<RuleTransitionMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut world_sprite_materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
) {
    for (entity, mut transition) in &mut transitions {
        transition.timer.tick(time.delta());
        if let Some(mut material) = rule_materials.get_mut(&transition.material) {
            material.progress = tween_fraction(&transition.timer);
        }
        if transition.timer.is_finished() {
            let sprite = WorldSprite::from_image(transition.target_image.clone());
            let render =
                world_sprite_render_components(&sprite, &mut meshes, &mut world_sprite_materials);
            let new_background = commands
                .spawn((
                    BackgroundLayer {
                        path: transition.target_path.clone(),
                    },
                    sprite,
                    render,
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id();
            stage.background = Some(new_background);
            if stage.transition == Some(entity) {
                stage.transition = None;
            }
            commands
                .entity(transition.previous_background)
                .try_despawn();
            commands.entity(entity).try_despawn();
            complete_missing_animation(&mut animations, transition.animation_id.take());
        }
    }
}

pub fn animate_custom_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut materials: ResMut<Assets<CustomScreenEffectMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut world_sprite_materials: ResMut<Assets<WorldSpriteMaterial>>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    for (entity, mut effect) in &mut effects {
        effect.timer.tick(time.delta());
        if let Some(mut material) = materials.get_mut(&effect.material) {
            material.progress = tween_fraction(&effect.timer);
            material.time = effect.timer.elapsed_secs();
        }
        if effect.timer.is_finished() {
            if let Some(target_path) = effect.target_path.take()
                && let Some(target_image) = effect.target_image.take()
            {
                let sprite = WorldSprite::from_image(target_image);
                let render = world_sprite_render_components(
                    &sprite,
                    &mut meshes,
                    &mut world_sprite_materials,
                );
                let new_background = commands
                    .spawn((
                        BackgroundLayer { path: target_path },
                        sprite,
                        render,
                        Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                    ))
                    .id();
                stage.background = Some(new_background);
            }
            if let Some(previous_background) = effect.previous_background.take()
                && stage.background != Some(previous_background)
            {
                commands.entity(previous_background).try_despawn();
            }
            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            commands.entity(entity).try_despawn();
            complete_missing_animation(&mut animations, effect.animation_id.take());
        }
    }
}

pub fn tick_animation_waits(
    mut animations: ResMut<AnimationState>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    let completed = animations.completed.clone();
    let mut resolved = Vec::new();
    animations.waits.retain(|wait| {
        if wait.ids.iter().all(|id| completed.contains(id)) {
            resolved.push(wait.done.clone());
            false
        } else {
            true
        }
    });
    for done in resolved {
        responses.write(ScriptResponseMessage {
            request: done,
            response: ScriptResponse::Continue,
        });
    }
}

pub(crate) fn complete_missing_animation(
    animations: &mut AnimationState,
    animation_id: Option<String>,
) {
    if let Some(animation_id) = animation_id {
        animations.completed.insert(animation_id);
    }
}

pub(crate) fn tween_fraction(timer: &Timer) -> f32 {
    let duration = timer.duration().as_secs_f32();
    if duration <= f32::EPSILON {
        1.0
    } else {
        (timer.elapsed_secs() / duration).clamp(0.0, 1.0)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_animation_cancellations(
    mut commands: Commands,
    mut stage: ResMut<StageState>,
    mut waits: ResMut<PendingWaits>,
    mut dialogue_state: ResMut<DialogueState>,
    mut pending_cancels: ResMut<PendingAnimationCancels>,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    mut voice_state: ResMut<VoiceState>,
    mut pending_characters: ResMut<PendingCharacterShows>,
    mut tweens: Query<(Entity, Option<&SpriteActor>, &mut VisualTween)>,
    mut bgm_fades: Query<(Entity, &mut BgmFade)>,
    mut motion_queries: ParamSet<(
        Query<'_, '_, &'static mut Transform, With<WorldCamera>>,
        Query<
            '_,
            '_,
            (
                Entity,
                &'static mut Transform,
                Option<&'static mut CharacterJumpEffect>,
                Option<&'static mut CharacterShakeEffect>,
                Option<&'static mut CharacterTimelineEffect>,
            ),
            Without<WorldCamera>,
        >,
    )>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    if pending_cancels.ids.is_empty() {
        return;
    }

    let cancelled = pending_cancels.ids.drain(..).collect::<HashSet<_>>();
    for id in &cancelled {
        animations.completed.insert(id.clone());
    }

    waits.items.retain(|wait| {
        if wait
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            commands.write_message(ScriptResponseMessage {
                request: wait.done,
                response: ScriptResponse::Continue,
            });
            false
        } else {
            true
        }
    });

    if dialogue_state
        .waiting
        .as_ref()
        .and_then(|waiting| waiting.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        if let Some(waiting) = dialogue_state.waiting.take() {
            complete_dialogue_wait(&mut commands, &mut animations, waiting);
        }
    }

    if let Some(shake) = shake_state.active.as_mut()
        && shake
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        for mut camera in &mut motion_queries.p0() {
            camera.translation.x = 0.0;
            camera.translation.y = 0.0;
        }
        complete_missing_animation(&mut animations, shake.animation_id.take());
        shake_state.active = None;
    }

    if voice_state
        .active
        .as_ref()
        .and_then(|voice| voice.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }

    pending_characters.items.retain_mut(|item| {
        if item
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
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
            false
        } else {
            true
        }
    });

    for (entity, actor, mut tween) in &mut tweens {
        if tween
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if tween.despawn_on_finish
                && let Some(actor) = actor
            {
                stage.sprites.insert(actor.id.clone(), entity);
            }
            complete_missing_animation(&mut animations, tween.animation_id.take());
            commands.entity(entity).try_remove::<VisualTween>();
        }
    }

    for (entity, mut fade) in &mut bgm_fades {
        if fade
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, fade.animation_id.take());
            commands.entity(entity).try_remove::<BgmFade>();
        }
    }

    for (entity, mut transform, jump, shake, timeline) in &mut motion_queries.p1() {
        let mut reset_translation = false;
        let origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin));

        if let Some(mut effect) = jump
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_remove::<CharacterJumpEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = shake
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_remove::<CharacterShakeEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = timeline
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if let Some(final_keyframe) = effect.keyframes.last() {
                stage
                    .character_positions
                    .insert(effect.actor_id.clone(), final_keyframe.position);
            }
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands
                .entity(entity)
                .try_remove::<CharacterTimelineEffect>();
            reset_translation = true;
        }

        if reset_translation {
            if let Some(origin) = origin {
                transform.translation = origin;
            }
        }
    }

    for (entity, mut transition) in &mut transitions {
        if transition
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.transition == Some(entity) {
                stage.transition = None;
            }
            complete_missing_animation(&mut animations, transition.animation_id.take());
            commands.entity(entity).try_despawn();
        }
    }

    for (entity, mut effect) in &mut effects {
        if effect
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            complete_missing_animation(&mut animations, effect.animation_id.take());
            commands.entity(entity).try_despawn();
        }
    }
}
