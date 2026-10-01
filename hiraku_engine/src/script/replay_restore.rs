use super::{
    ScriptBootstrap, StoryRuntime, StoryRuntimeEvent,
    capabilities::{StoryEffect, StoryWait},
    navigation::{NavigationKind, NavigationReset},
    replay::{InputKind, ReplayPoint, boundary},
};
use crate::{storage::UserSettings, vfs::VfsResource};
use hiraku_script::Value;

pub(super) fn prepare(
    vfs: &VfsResource,
    mut bootstrap: ScriptBootstrap,
    settings: &UserSettings,
) -> Result<ScriptBootstrap, String> {
    let journal = bootstrap
        .replay
        .as_ref()
        .ok_or("save has no replay journal")?;
    let mut cursor = journal.playback().map_err(|e| e.to_string())?;
    let mut path = journal.entry_script.clone();
    let mut story = create(vfs, &path)?;
    story.set_globals(super::capabilities::engine_globals(settings));
    let mut callers: Vec<(String, StoryRuntime)> = Vec::new();
    let mut dialogue = String::new();
    let mut idle_steps = 0;
    for _ in 0..1_000_000 {
        story.replay_inputs(cursor.native_inputs(&path));
        let event = story.step().map_err(|e| e.to_string())?;
        let (inputs, unrecordable) = story.take_native_trace();
        if unrecordable {
            return Err("native input cannot be restored deterministically".into());
        }
        for (signature, value) in inputs {
            let point = ReplayPoint {
                script: path.clone(),
                signature,
            };
            let recorded = cursor
                .input(InputKind::Native, &point)
                .map_err(|e| e.to_string())?;
            if recorded != value {
                return Err("recorded native input changed during replay".into());
            }
        }
        let Some(mut event) = event else {
            idle_steps += 1;
            if idle_steps >= 128 {
                return Err("deterministic replay stalled before an external boundary".into());
            }
            continue;
        };
        idle_steps = 0;
        if let Some(point) = boundary(&path, &mut dialogue, &event)? {
            if cursor.exhausted() {
                cursor.finish(&point).map_err(|e| e.to_string())?;
                story.finish_replay();
                bootstrap.startup_script = path;
                bootstrap.snapshot = Some(story.snapshot().map_err(|e| e.to_string())?);
                bootstrap.call_stack = callers
                    .into_iter()
                    .map(|(script, mut story)| {
                        story.finish_replay();
                        Ok(crate::state::ScriptCallFrameSnapshot {
                            script,
                            snapshot: story.snapshot().map_err(|e| e.to_string())?,
                        })
                    })
                    .collect::<Result<_, String>>()?;
                bootstrap.values.clear();
                return Ok(bootstrap);
            }
            if cursor.next_kind() == Some(&InputKind::Navigation) {
                let crate::state::StoredValue::String(request) = cursor
                    .input(InputKind::Navigation, &point)
                    .map_err(|e| e.to_string())?
                else {
                    return Err("invalid recorded UI navigation".into());
                };
                event = StoryRuntimeEvent::Effect(StoryEffect::Navigate(
                    hiraku_script::hson::from_str(&request).map_err(|e| e.to_string())?,
                ));
            } else {
                let value = match &event {
                    StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance) => {
                        cursor.dialogue(&point).map_err(|e| e.to_string())?;
                        Value::Unit
                    }
                    StoryRuntimeEvent::Choice { .. } => super::stored_value_to_hks(
                        cursor
                            .input(InputKind::Choice, &point)
                            .map_err(|e| e.to_string())?,
                    ),
                    StoryRuntimeEvent::OpenUi { .. }
                        if cursor.next_kind() == Some(&InputKind::UiUnit) =>
                    {
                        cursor
                            .input(InputKind::UiUnit, &point)
                            .map_err(|e| e.to_string())?;
                        Value::Unit
                    }
                    StoryRuntimeEvent::OpenUi { .. } => super::stored_value_to_hks(
                        cursor
                            .input(InputKind::UiResult, &point)
                            .map_err(|e| e.to_string())?,
                    ),
                    StoryRuntimeEvent::RandomInt { .. } => super::stored_value_to_hks(
                        cursor
                            .input(InputKind::Random, &point)
                            .map_err(|e| e.to_string())?,
                    ),
                    _ => return Err("unsupported replay boundary".into()),
                };
                story.resume(value).map_err(|e| e.to_string())?;
                continue;
            }
        }
        match event {
            StoryRuntimeEvent::TaskEffect { task, effect } => {
                story
                    .complete_task_effect(task, &effect)
                    .map_err(|e| e.to_string())?;
            }
            StoryRuntimeEvent::Wait(_) => story.resume(Value::Unit).map_err(|e| e.to_string())?,
            StoryRuntimeEvent::Effect(StoryEffect::Navigate(request)) => {
                let next_path = vfs
                    .0
                    .resolve_path(request.origin.as_deref().or(Some(&path)), &request.path);
                let mut next = if let Some(program) = story.program_for_path(&next_path) {
                    StoryRuntime::new(program).map_err(|e| e.to_string())?
                } else {
                    create(vfs, &next_path)?
                };
                let mut globals = if request.reset == NavigationReset::Session {
                    Default::default()
                } else {
                    story.globals().clone()
                };
                globals.extend(super::capabilities::engine_globals(settings));
                next.set_globals(globals);
                if request.reset != NavigationReset::Session {
                    next.inherit_native_state(
                        &story,
                        request.reset == NavigationReset::Presentation,
                    );
                }
                if request.kind == NavigationKind::Call {
                    callers.push((path, story));
                } else {
                    callers.clear();
                }
                path = next_path;
                story = next;
            }
            StoryRuntimeEvent::Completed(_) => {
                let Some((caller_path, mut caller)) = callers.pop() else {
                    return Err("story completed before the saved destination".into());
                };
                let mut globals = caller.globals().clone();
                globals.extend(story.globals().clone());
                caller.set_globals(globals);
                caller.inherit_native_state(&story, false);
                story = caller;
                path = caller_path;
            }
            StoryRuntimeEvent::Effect(StoryEffect::Exit) => {
                return Err("story exited before the saved destination".into());
            }
            _ => (),
        }
    }
    Err("deterministic replay exceeded its execution budget".into())
}

fn create(vfs: &VfsResource, path: &str) -> Result<StoryRuntime, String> {
    let source = vfs.0.read_text(path).map_err(|e| e.to_string())?;
    let program = super::compile_story_program(&vfs.0, path, &source)?;
    StoryRuntime::new(program).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        script::{ScriptRuntimeState, start_story_runtime},
        state::{SaveGameData, StoredValue},
    };
    use std::{path::PathBuf, sync::Arc};

    fn fixture(files: &[(&str, &str)]) -> VfsResource {
        let mut builder = hiraku_hdp::PackageBuilder::new();
        for (path, source) in files {
            builder
                .add_file(*path, source.as_bytes())
                .expect("fixture file");
        }
        let package = builder
            .build(hiraku_hdp::PackOptions::default())
            .expect("fixture package");
        let archive =
            hiraku_hdp::Archive::from_bytes(Arc::<[u8]>::from(package.volumes[0].clone()))
                .expect("fixture archive");
        let store = crate::vfs::HdpArchiveStore::default();
        store
            .publish(Arc::new(archive), PathBuf::from("fixture.hdp"))
            .expect("publish fixture");
        VfsResource(
            crate::vfs::HdpVfs::new_with_config_and_store(
                PathBuf::new(),
                "settings.hson",
                "alice.hks",
                store,
            )
            .into(),
        )
    }

    fn record(files: &[(&str, &str)], advances: usize) -> SaveGameData {
        let vfs = fixture(files);
        let path = "hdp://fixture.hdp/alice.hks";
        let mut story = create(&vfs, path).expect("original runtime");
        let mut journal = super::super::replay::ReplayJournal::recording(path.into());
        let mut dialogue = String::new();
        let mut continued = 0;
        for _ in 0..1000 {
            let event = story.step().expect("original step");
            let (inputs, incomplete) = story.take_native_trace();
            assert!(!incomplete);
            for (signature, value) in inputs {
                journal.input(
                    InputKind::Native,
                    ReplayPoint {
                        script: path.into(),
                        signature,
                    },
                    value,
                );
            }
            let Some(event) = event else { continue };
            let Some(point) = boundary(path, &mut dialogue, &event).expect("original boundary")
            else {
                if let StoryRuntimeEvent::TaskEffect { task, effect } = event {
                    story
                        .complete_task_effect(task, &effect)
                        .expect("complete animation");
                }
                continue;
            };
            if matches!(event, StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance)) {
                if continued == advances {
                    journal.destination = Some(point);
                    return SaveGameData {
                        resume_script: path.into(),
                        vm_snapshot: Some(story.snapshot().expect("snapshot")),
                        replay: Some(journal),
                        ..Default::default()
                    };
                }
                journal.dialogue(&point).expect("record advance");
                story.resume(Value::Unit).expect("advance");
                continued += 1;
            } else if let StoryRuntimeEvent::Choice { .. } = event {
                journal.input(InputKind::Choice, point, StoredValue::Int(1));
                story.resume(Value::Int(1)).expect("select");
            } else if let StoryRuntimeEvent::RandomInt { min, max } = event {
                journal.destination = Some(point);
                let value = journal.random_int(min, max);
                story.resume(Value::Int(value)).expect("random draw");
            } else {
                panic!("unexpected fixture boundary: {event:?}")
            }
        }
        panic!("fixture did not reach destination");
    }

    #[test]
    fn changed_bytecode_replays_choices_random_draws_and_local_captures() {
        let source = r#"
            global var result = randomInt(1, 100)
            var answer = 0
            choice {
                option("Alice") { answer = 1 }
                option("Bob") { answer = 2 }
            }
            result += answer
            "first"
            "second"
        "#;
        let save = record(&[("alice.hks", source)], 1);
        let changed = format!("let unused = 17\n{source}");
        let vfs = fixture(&[("alice.hks", &changed)]);
        let mut live = ScriptRuntimeState::default();
        start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect("replay changed bytecode");
        let value = live.story.as_ref().expect("recovered runtime").globals()["result"].clone();
        let draw = save
            .replay
            .as_ref()
            .expect("journal")
            .events
            .iter()
            .find_map(|event| match event {
                super::super::replay::ReplayEvent::Input {
                    kind: InputKind::Random,
                    value: StoredValue::Int(value),
                    ..
                } => Some(*value),
                _ => None,
            })
            .expect("recorded random draw");
        assert_eq!(value, Value::Int(draw + 2));
        assert!(matches!(
            live.story
                .as_mut()
                .expect("runtime")
                .step()
                .expect("restored boundary"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        ));
    }

    #[test]
    fn diverged_dialogue_does_not_replace_live_runtime() {
        let save = record(&[("alice.hks", "\"first\"\n\"second\"")], 1);
        let vfs = fixture(&[("alice.hks", "\"changed\"\n\"second\"")]);
        let mut live = ScriptRuntimeState::default();
        live.current_script = Some("memory://bob.hks".into());
        let error = start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect_err("dialogue mismatch");
        assert!(error.contains("dialogue sequence changed"), "{error}");
        assert_eq!(live.current_script.as_deref(), Some("memory://bob.hks"));
        assert!(live.story.is_none());
    }

    #[test]
    fn replay_preserves_an_unanswered_choice() {
        let source = "choice { option(\"Alice\") { \"alice\" } option(\"Bob\") { \"bob\" } }";
        let old = fixture(&[("alice.hks", source)]);
        let path = "hdp://fixture.hdp/alice.hks";
        let mut story = create(&old, path).expect("original");
        let event = story.step().expect("step").expect("choice");
        let mut journal = super::super::replay::ReplayJournal::recording(path.into());
        journal.destination = boundary(path, &mut String::new(), &event).expect("point");
        let save = SaveGameData {
            resume_script: path.into(),
            vm_snapshot: Some(story.snapshot().expect("snapshot")),
            replay: Some(journal),
            ..Default::default()
        };
        let changed = format!("let unused = 1\n{source}");
        let vfs = fixture(&[("alice.hks", &changed)]);
        let mut live = ScriptRuntimeState::default();
        start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect("pending choice recovered");
        assert!(matches!(
            live.story
                .as_mut()
                .expect("runtime")
                .step()
                .expect("choice"),
            Some(StoryRuntimeEvent::Choice { .. })
        ));
    }

    #[test]
    fn replay_can_follow_ui_navigation_while_dialogue_is_waiting() {
        let vfs = fixture(&[("alice.hks", "\"title\""), ("bob.hks", "\"saved\"")]);
        let path = "hdp://fixture.hdp/alice.hks";
        let mut story = create(&vfs, path).expect("original runtime");
        let mut dialogue = String::new();
        let mut point = None;
        for _ in 0..100 {
            let Some(event) = story.step().expect("step") else {
                continue;
            };
            point = boundary(path, &mut dialogue, &event).expect("boundary");
            if point.is_some() {
                break;
            }
        }
        let mut journal = super::super::replay::ReplayJournal::recording(path.into());
        let request = super::super::navigation::NavigationRequest::goto("bob.hks".into())
            .expect("navigation");
        journal.input(
            InputKind::Navigation,
            point.expect("dialogue wait"),
            StoredValue::String(
                hiraku_script::hson::to_string(&request).expect("encode navigation"),
            ),
        );
        boundary(
            "hdp://fixture.hdp/bob.hks",
            &mut dialogue,
            &StoryRuntimeEvent::Effect(StoryEffect::Say {
                speaker: String::new(),
                text: "saved".into(),
            }),
        )
        .expect("saved dialogue");
        journal.destination = boundary(
            "hdp://fixture.hdp/bob.hks",
            &mut dialogue,
            &StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance),
        )
        .expect("destination");
        let save = SaveGameData {
            resume_script: "hdp://fixture.hdp/bob.hks".into(),
            replay: Some(journal),
            ..Default::default()
        };
        let mut live = ScriptRuntimeState::default();
        start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect("UI navigation replayed without advancing abandoned dialogue");
        assert_eq!(
            live.current_script.as_deref(),
            Some("hdp://fixture.hdp/bob.hks")
        );
    }

    #[test]
    fn replay_uses_recorded_profile_reads_and_does_not_write_to_storage() {
        let source =
            "global var result = profile.readInt(\"\")\nprofile.writeInt(\"\", result)\n\"saved\"";
        let vfs = fixture(&[("alice.hks", source)]);
        let path = "hdp://fixture.hdp/alice.hks";
        let symbols = hiraku_script::symbol::SymbolManifest::default();
        let arguments = vec![
            hiraku_script::persistence::to_value(&Value::String(String::new()), &symbols)
                .expect("portable arguments"),
        ];
        let signature = format!(
            "native:profile.readInt:{}",
            hiraku_script::hson::to_string(&arguments).expect("signature")
        );
        let result = hiraku_script::persistence::to_value(&Value::Int(42), &symbols)
            .expect("portable result");
        let mut journal = super::super::replay::ReplayJournal::recording(path.into());
        journal.input(
            InputKind::Native,
            ReplayPoint {
                script: path.into(),
                signature,
            },
            StoredValue::String(hiraku_script::hson::to_string(&result).expect("result")),
        );
        let mut dialogue = String::new();
        boundary(
            path,
            &mut dialogue,
            &StoryRuntimeEvent::Effect(StoryEffect::Say {
                speaker: String::new(),
                text: "saved".into(),
            }),
        )
        .expect("dialogue");
        journal.destination = boundary(
            path,
            &mut dialogue,
            &StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance),
        )
        .expect("destination");
        let save = SaveGameData {
            resume_script: path.into(),
            replay: Some(journal),
            ..Default::default()
        };
        let mut live = ScriptRuntimeState::default();
        start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect("empty keys must never reach persistent storage during replay");
        assert_eq!(
            live.story.as_ref().expect("runtime").globals()["result"],
            Value::Int(42)
        );
    }

    #[test]
    fn replay_rebuilds_changed_script_call_stack() {
        let alice = "global var result = 0\nstory.call(\"bob.hks\")\nresult += 1\n\"after call\"";
        let bob = "global var calleeResult = 7\n\"inside call\"";
        let old = fixture(&[("alice.hks", alice), ("bob.hks", bob)]);
        let path = "hdp://fixture.hdp/alice.hks";
        let mut caller = create(&old, path).expect("caller");
        let mut navigation = false;
        for _ in 0..100 {
            if matches!(
                caller.step().expect("caller step"),
                Some(StoryRuntimeEvent::Effect(StoryEffect::Navigate(_)))
            ) {
                navigation = true;
                break;
            }
        }
        assert!(navigation);
        let mut callee = create(&old, "hdp://fixture.hdp/bob.hks").expect("callee");
        callee.set_globals(caller.globals().clone());
        let mut dialogue = String::new();
        let mut journal = super::super::replay::ReplayJournal::recording(path.into());
        for _ in 0..100 {
            let Some(event) = callee.step().expect("callee step") else {
                continue;
            };
            if let Some(point) =
                boundary("hdp://fixture.hdp/bob.hks", &mut dialogue, &event).expect("boundary")
            {
                journal.destination = Some(point);
                break;
            }
        }
        let save = SaveGameData {
            resume_script: "hdp://fixture.hdp/bob.hks".into(),
            vm_snapshot: Some(callee.snapshot().expect("callee snapshot")),
            script_call_stack: vec![crate::state::ScriptCallFrameSnapshot {
                script: path.into(),
                snapshot: caller.snapshot().expect("caller snapshot"),
            }],
            replay: Some(journal),
            ..Default::default()
        };
        let changed = format!("let unused = 1\n{alice}");
        let vfs = fixture(&[("alice.hks", &changed), ("bob.hks", bob)]);
        let mut live = ScriptRuntimeState::default();
        start_story_runtime(
            &vfs,
            &mut live,
            ScriptBootstrap::from_save(&save).expect("bootstrap"),
            &UserSettings::default(),
        )
        .expect("recover caller too");
        assert_eq!(
            live.current_script.as_deref(),
            Some("hdp://fixture.hdp/bob.hks")
        );
        assert_eq!(live.call_stack.len(), 1);
        assert_eq!(
            live.story.as_ref().expect("callee").globals()["calleeResult"],
            Value::Int(7)
        );
    }
}
