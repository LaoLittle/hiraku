//! ECS-owned completion predicates for scene effects without a dedicated token.
use super::*;

#[derive(Debug, Clone)]
pub enum SceneEffect {
    Spatial(crate::stage::runtime::StageCommand),
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
    spatial: Res<crate::stage::runtime::StageRuntime>,
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
            SceneEffect::Spatial(command) => {
                if let Some(error) = &spatial.error {
                    crate::script::emit_script_diagnostic("stage execution failed", error);
                    runtime.story = None;
                    commands.entity(entity).try_despawn();
                    continue;
                }
                shared.0.spatial_stage.as_ref().is_some_and(|state| {
                    use crate::stage::runtime::StageCommand;
                    match command {
                        StageCommand::Close { .. } => false,
                        StageCommand::Clip { id, .. } => state.id == *id && !spatial.ready(state),
                        StageCommand::Open { id, .. } | StageCommand::Place { id, .. } => {
                            state.id == *id && !spatial.ready(state)
                        }
                        StageCommand::Camera { id, view, name, .. } => {
                            state.id == *id
                                && (!spatial.ready(state)
                                    || state.views.get(view).is_some_and(|v| {
                                        v.request.as_ref().is_some_and(|r| &r.0 == name)
                                            || (v.camera_name.as_ref() == Some(name)
                                                && v.tween.is_some())
                                    }))
                        }
                        StageCommand::View { id, view, .. } => {
                            state.id == *id
                                && (!spatial.ready(state)
                                    || state.views.get(view).is_some_and(|v| v.fade.is_some()))
                        }
                    }
                })
            }
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
            SceneEffect::Picture(P::Clear | P::StopMotion { .. } | P::Noise { .. }) => false,
            SceneEffect::Picture(command) => {
                let (P::Show { id, .. }
                | P::Hide { id, .. }
                | P::Exit { id, .. }
                | P::Transform { id, .. }
                | P::AnimateX { id, .. }
                | P::Tint { id, .. }
                | P::Blur { id, .. }) = command
                else {
                    unreachable!()
                };
                if let Some(picture) = shared.0.pictures.get(id) {
                    let handle: Handle<Image> =
                        crate::texture::load_static_image(&assets, picture.path.clone());
                    if !matches!(command, P::Hide { .. } | P::Exit { .. })
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
                        P::Hide { .. } | P::Exit { .. } => picture.fade.is_some(),
                        P::Blur { .. } => picture.blur_tween.is_some(),
                        P::Tint { .. } => picture.tint_tween.is_some(),
                        P::Transform { .. } | P::AnimateX { .. } => picture.motion.is_some(),
                        P::Clear | P::StopMotion { .. } | P::Noise { .. } => false,
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
