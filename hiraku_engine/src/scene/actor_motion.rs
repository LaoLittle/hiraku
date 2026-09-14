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
    let origin = motions.get(&actor).map_or([0.0; 2], |motion| {
        if motion.transition.oscillation.is_some() {
            motion.origin
        } else {
            motion.offset
        }
    });
    let mut motion = ActorMotion::new(revision, transition, origin);
    motion.animation_id = completion;
    motions.insert(actor, motion);
}

pub(super) fn finish(motion: &mut ActorMotion, animations: &mut AnimationState) {
    motion.finish();
    complete_missing_animation(animations, motion.animation_id.take());
}

pub(crate) fn animate(
    mut redraw: crate::redraw::Redraw,
    time: crate::scene::playback::StoryTime,
    mut shared: ResMut<SceneSharedState>,
    stage: Res<StageState>,
    mut animations: ResMut<AnimationState>,
    roots: Query<(&character::ActorPlacement, &Children)>,
    mut parts: Query<(&character_composite::LogicalCharacterPart, &mut Transform)>,
) {
    for (actor, motion) in &mut shared.0.actor_motions {
        if !motion.finished {
            redraw.request();
        }
        if !stage.character_active_parts.contains_key(actor)
            && !stage
                .pending_character_restore
                .iter()
                .any(|part| part.id.starts_with(&format!("character::{actor}::")))
        {
            finish(motion, &mut animations);
        }
        let Some(root) = stage.character_roots.get(actor) else {
            continue;
        };
        let Ok((placement, children)) = roots.get(*root) else {
            continue;
        };
        if stage.character_active_parts.contains_key(actor) {
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
    fn concurrent_recorded_jump_sequences_play_every_step_and_restore_mid_jump() {
        use crate::script::capabilities::StoryEffect;
        use crate::script::{StoryRuntime, StoryRuntimeEvent, compile_story_bytecode};

        let code = compile_story_bytecode(
            "jumps.hks",
            r#"
            let alice = char("alice")
            let bob = char("bob")
            alice.show()
            bob.show()
            let first = seq {
                alice.offset(.pos(0, 86.4)).time(0.15).easing(.easeOut)
                alice.offset(.pos(0, -28.8)).time(0.15).easing(.easeIn)
                alice.offset(.pos(0, 86.4)).time(0.15).easing(.easeOut)
                alice.offset(.pos(0, -28.8)).time(0.15).easing(.easeIn)
                alice.offset(.pos(0, 0)).time(0.1).easing(.easeOut)
            }
            let second = seq {
                bob.offset(.pos(0, 86.4)).time(0.15).easing(.easeOut)
                bob.offset(.pos(0, -28.8)).time(0.15).easing(.easeIn)
                bob.offset(.pos(0, 86.4)).time(0.15).easing(.easeOut)
                bob.offset(.pos(0, -28.8)).time(0.15).easing(.easeIn)
                bob.offset(.pos(0, 0)).time(0.1).easing(.easeOut)
            }
            first.await()
            second.await()
            log("done")
        "#,
        )
        .expect("compile independent jump plans");

        for restore in [false, true] {
            let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
            let mut motions = BTreeMap::<String, ActorMotion>::new();
            let mut animations = AnimationState::default();
            let mut active = BTreeMap::new();
            let mut targets = BTreeMap::<String, Vec<f32>>::new();
            let mut done = false;
            for tick in 0..30 {
                while let Some(event) = runtime.step().expect("advance script") {
                    match event {
                        StoryRuntimeEvent::TaskEffect { task, effect } => {
                            let StoryEffect::ActorMotion {
                                actor_id,
                                revision,
                                transition,
                            } = &effect
                            else {
                                panic!("expected offset");
                            };
                            let reattach = motions
                                .get(actor_id)
                                .is_some_and(|motion| motion.revision == *revision);
                            let previous_offset = motions
                                .get(actor_id)
                                .map_or([0.0; 2], |motion| motion.offset);
                            start(
                                &mut motions,
                                &mut animations,
                                actor_id.clone(),
                                *revision,
                                *transition,
                                Some(format!("{actor_id}:{revision}")),
                            );
                            if !reattach {
                                assert!(
                                    !motions[actor_id].finished,
                                    "each new step must start a tween"
                                );
                                assert_eq!(motions[actor_id].origin, previous_offset);
                                targets
                                    .entry(actor_id.clone())
                                    .or_default()
                                    .push(transition.target[1]);
                            }
                            active.insert(task, effect);
                        }
                        StoryRuntimeEvent::Effect(StoryEffect::Log(message)) => {
                            assert_eq!(message, "done");
                            done = true;
                        }
                        StoryRuntimeEvent::Completed(_) => break,
                        _ => {}
                    }
                }
                if done {
                    break;
                }
                if tick == 0 {
                    assert_eq!(
                        active.len(),
                        2,
                        "both plans start without waiting for each other"
                    );
                }
                if restore && tick == 1 {
                    let snapshot = hiraku_script::hson::to_vec(
                        &runtime.snapshot().expect("snapshot at host wait"),
                    )
                    .expect("encode runtime");
                    runtime = StoryRuntime::restore(
                        code.clone(),
                        hiraku_script::hson::from_slice(&snapshot).expect("decode runtime"),
                    )
                    .expect("restore runtime");
                    let snapshot =
                        hiraku_script::hson::to_vec(&motions).expect("encode scene motions");
                    motions =
                        hiraku_script::hson::from_slice(&snapshot).expect("restore scene motions");
                    animations = AnimationState::default();
                    continue;
                }
                for motion in motions.values_mut() {
                    motion.advance(0.075);
                }
                let completed = active
                    .iter()
                    .filter_map(|(task, effect)| {
                        let StoryEffect::ActorMotion { actor_id, .. } = effect else {
                            return None;
                        };
                        motions[actor_id]
                            .finished
                            .then_some((*task, effect.clone()))
                    })
                    .collect::<Vec<_>>();
                for (task, effect) in completed {
                    runtime
                        .complete_task_effect(task, &effect)
                        .expect("complete ECS motion");
                    active.remove(&task);
                }
            }
            assert!(done, "both sequences must complete");
            for actor in ["alice", "bob"] {
                assert_eq!(targets[actor], vec![86.4, -28.8, 86.4, -28.8, 0.0]);
                assert_eq!(motions[actor].offset, [0.0; 2]);
            }
        }
    }

    #[test]
    fn hidden_motion_without_render_root_finishes_at_target_and_releases_wait() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneSharedState>()
            .init_resource::<StageState>()
            .init_resource::<AnimationState>()
            .add_systems(Update, animate);
        let mut motion = ActorMotion::new(
            1,
            ActorOffset {
                oscillation: None,
                target: [4.0, 8.0],
                animation: crate::script::AnimationSpec::Linear(1.0, false),
            },
            [0.0; 2],
        );
        motion.animation_id = Some("alice-motion".into());
        app.world_mut()
            .resource_mut::<SceneSharedState>()
            .0
            .actor_motions
            .insert("alice".into(), motion);
        app.update();
        let shared = app.world().resource::<SceneSharedState>();
        assert_eq!(shared.0.actor_motions["alice"].offset, [4.0, 8.0]);
        assert!(shared.0.actor_motions["alice"].finished);
        assert!(
            app.world()
                .resource::<AnimationState>()
                .completed
                .contains("alice-motion")
        );
    }

    #[test]
    fn restored_completion_reattaches_and_retarget_completes_old_request() {
        let mut motions = BTreeMap::new();
        let mut animations = AnimationState::default();
        let transition = ActorOffset {
            oscillation: None,
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
