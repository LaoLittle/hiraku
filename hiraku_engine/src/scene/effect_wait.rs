//! ECS-owned completion predicates for scene effects without a dedicated token.
use super::*;

#[derive(Debug, Clone)]
pub enum SceneEffect {
    Picture(super::pictures::PictureCommand),
    HideCharacter(Option<String>),
    ShowCharacter(String),
}

#[derive(Component)]
pub struct SceneEffectWait {
    pub effect: SceneEffect,
    pub done: ScriptRequestId,
}

pub fn complete(
    mut commands: Commands,
    mut runtime: ResMut<ScriptRuntimeState>,
    shared: Res<SceneSharedState>,
    stage: Res<StageState>,
    assets: Res<AssetServer>,
    groups: Query<&super::character_composite::CharacterGroup>,
    placements: Query<(&super::character::ActorPlacement, &Children)>,
    tweens: Query<&VisualTween>,
    pending: Res<PendingCharacterShows>,
    waits: Query<(Entity, &SceneEffectWait)>,
    mut responses: MessageWriter<ScriptResponseMessage>,
) {
    use super::pictures::PictureCommand as P;
    for (entity, wait) in &waits {
        if !runtime.task_requests.contains_key(&wait.done) {
            commands.entity(entity).try_despawn();
            continue;
        }
        let busy = match &wait.effect {
            SceneEffect::ShowCharacter(actor) => {
                pending.items.iter().any(|show| &show.actor_id == actor)
                    || stage.character_roots.get(actor).is_some_and(|root| {
                        groups.get(*root).is_ok_and(|g| g.is_animating())
                            || placements.get(*root).is_ok_and(|(placement, children)| {
                                placement.is_animating()
                                    || children.iter().any(|child| {
                                        tweens.get(child).is_ok_and(|t| !t.timer.is_finished())
                                    })
                            })
                    })
            }
            SceneEffect::HideCharacter(actor) => {
                stage.character_roots.iter().any(|(name, root)| {
                    actor.as_ref().is_none_or(|id| id == name)
                        && groups.get(*root).is_ok_and(|group| group.is_animating())
                })
            }
            SceneEffect::Picture(P::Clear) => false,
            SceneEffect::Picture(command) => {
                let (P::Show { id, .. }
                | P::Hide { id, .. }
                | P::Move { id, .. }
                | P::AnimateX { id, .. }
                | P::Tint { id, .. }
                | P::Blur { id, .. }) = command
                else {
                    unreachable!()
                };
                if let Some(picture) = shared.0.pictures.get(id) {
                    let handle: Handle<Image> = assets.load(picture.path.clone());
                    if !matches!(command, P::Hide { .. })
                        && matches!(
                            assets.load_state(handle.id()),
                            bevy::asset::LoadState::Failed(_)
                        )
                    {
                        crate::script::emit_script_diagnostic(
                            "picture animation failed",
                            &format!("failed to load `{}`", picture.path),
                        );
                        runtime.story = None;
                        commands.entity(entity).try_despawn();
                        continue;
                    }
                    match command {
                        P::Show { .. } => {
                            !assets.is_loaded_with_dependencies(handle.id())
                                || picture.fade.is_some()
                        }
                        P::Hide { .. } => picture.fade.is_some(),
                        P::Blur { .. } => picture.blur_tween.is_some(),
                        P::Tint { .. } => picture.tint_tween.is_some(),
                        P::Move { .. } | P::AnimateX { .. } => picture.motion.is_some(),
                        P::Clear => false,
                    }
                } else {
                    false
                }
            }
        };
        if !busy {
            responses.write(ScriptResponseMessage {
                request: wait.done,
                response: ScriptResponse::Continue,
            });
            commands.entity(entity).try_despawn();
        }
    }
}
