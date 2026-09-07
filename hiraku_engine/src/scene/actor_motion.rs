//! Actor-space motion is projected once onto every cached expression part.
use super::*;
use crate::script::actor_motion::{ActorMotion, ActorOffset};

pub(super) fn start(
    motions: &mut BTreeMap<String, ActorMotion>,
    animations: &mut AnimationState,
    actor: String,
    revision: u64,
    transition: ActorOffset,
    completion: Option<String>,
) {
    if let Some(previous) = motions.get_mut(&actor) {
        // Restored task effects reattach, rather than restarting at frame zero.
        if previous.revision == revision {
            if previous.finished {
                complete_missing_animation(animations, completion);
            } else {
                previous.animation_id = completion;
            }
            return;
        }
        complete_missing_animation(animations, previous.animation_id.take());
    }
    let origin = motions.get(&actor).map_or([0.0; 2], |motion| motion.offset);
    let mut motion = ActorMotion::new(revision, transition, origin);
    motion.animation_id = completion;
    motions.insert(actor, motion);
}

pub(crate) fn animate(
    time: Res<Time>,
    mut shared: ResMut<SceneSharedState>,
    stage: Res<StageState>,
    mut animations: ResMut<AnimationState>,
    roots: Query<(&character::ActorPlacement, &Children)>,
    mut parts: Query<(&character_composite::LogicalCharacterPart, &mut Transform)>,
) {
    for (actor, motion) in &mut shared.0.actor_motions {
        let Some(root) = stage.character_roots.get(actor) else {
            continue;
        };
        let Ok((placement, children)) = roots.get(*root) else {
            continue;
        };
        if !stage.character_active_parts.contains_key(actor) {
            motion.finished = true;
            motion.offset = [0.0; 2];
        } else {
            motion.advance(time.delta_secs());
        }
        if motion.finished {
            complete_missing_animation(&mut animations, motion.animation_id.take());
        }
        // Recompute from the authoritative base, never add onto last frame's
        // transform. Incoming, outgoing and cached parts share the same sample.
        let offset = Vec2::from_array(motion.offset);
        for child in children.iter() {
            let Ok((part, mut transform)) = parts.get_mut(child) else {
                continue;
            };
            let xy = placement.current.translation.truncate()
                + part.0.offset * placement.current.scale.x
                + offset;
            transform.translation.x = xy.x;
            transform.translation.y = xy.y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_completion_reattaches_and_retarget_completes_old_request() {
        let mut motions = BTreeMap::new();
        let mut animations = AnimationState::default();
        let transition = ActorOffset {
            target: [0.0, 8.0],
            animation: crate::script::AnimationSpec::Linear(1.0, false),
        };
        start(
            &mut motions,
            &mut animations,
            "alice".into(),
            1,
            transition,
            Some("old".into()),
        );
        motions
            .get_mut("alice")
            .expect("motion exists")
            .advance(0.5);
        start(
            &mut motions,
            &mut animations,
            "alice".into(),
            1,
            transition,
            Some("restored".into()),
        );
        assert_eq!(motions["alice"].elapsed, 0.5);
        start(
            &mut motions,
            &mut animations,
            "alice".into(),
            2,
            transition,
            None,
        );
        assert!(animations.completed.contains("restored"));
        assert_eq!(motions["alice"].origin, [0.0, 4.0]);
    }
}
