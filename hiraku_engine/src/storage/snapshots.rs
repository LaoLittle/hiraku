use hiraku_script::{ObjectHeap, bhson};
use serde::{Deserialize, Serialize};

use super::StorageError;
use crate::{script::StoryRuntimeSnapshot, state::ScriptCallFrameSnapshot};

mod tables;

#[derive(Serialize, Deserialize)]
struct SharedSnapshots {
    heaps: Vec<ObjectHeap>,
    references: Vec<u32>,
    current: Option<StoryRuntimeSnapshot>,
    callers: Vec<ScriptCallFrameSnapshot>,
    history: Vec<crate::state::HistoryRecord>,
    text_contexts: Vec<std::sync::Arc<hiraku_text::template::TextSnapshot>>,
    text_references: Vec<u32>,
}

fn visit_heaps(
    current: &mut Option<StoryRuntimeSnapshot>,
    callers: &mut [ScriptCallFrameSnapshot],
    history: &mut [crate::state::HistoryRecord],
    visitor: &mut impl FnMut(&mut ObjectHeap),
) {
    if let Some(current) = current {
        current.visit_heaps(visitor);
    }
    for caller in callers {
        caller.snapshot.visit_heaps(visitor);
    }
    for record in history {
        if let Some(checkpoint) = &mut record.checkpoint {
            let checkpoint = std::sync::Arc::make_mut(checkpoint);
            visit_heaps(
                &mut checkpoint.vm_snapshot,
                &mut checkpoint.script_call_stack,
                &mut [],
                visitor,
            );
        }
    }
}

pub(super) fn encode(
    current: &Option<StoryRuntimeSnapshot>,
    callers: &[ScriptCallFrameSnapshot],
    history: &[crate::state::HistoryRecord],
) -> Result<Vec<u8>, StorageError> {
    let mut saved = SharedSnapshots {
        heaps: Vec::new(),
        references: Vec::new(),
        current: current.clone(),
        callers: callers.to_vec(),
        history: history.to_vec(),
        text_contexts: Vec::new(),
        text_references: Vec::new(),
    };
    visit_heaps(
        &mut saved.current,
        &mut saved.callers,
        &mut saved.history,
        &mut |heap| {
            let index = saved
                .heaps
                .iter()
                .position(|previous| previous == heap)
                .unwrap_or_else(|| {
                    saved.heaps.push(heap.clone());
                    saved.heaps.len() - 1
                });
            saved
                .references
                .push(u32::try_from(index).expect("snapshot heap table fits u32"));
            *heap = ObjectHeap::default();
        },
    );
    for record in &mut saved.history {
        for text in &mut record.text {
            let index = saved
                .text_contexts
                .iter()
                .position(|context| context == &text.context)
                .unwrap_or_else(|| {
                    saved.text_contexts.push(text.context.clone());
                    saved.text_contexts.len() - 1
                });
            saved
                .text_references
                .push(u32::try_from(index).expect("text context table fits u32"));
            text.context = Default::default();
        }
    }
    tables::encode(saved)
}

pub(super) fn decode(
    bytes: &[u8],
) -> Result<
    (
        Option<StoryRuntimeSnapshot>,
        Vec<ScriptCallFrameSnapshot>,
        Vec<crate::state::HistoryRecord>,
    ),
    StorageError,
> {
    let mut saved = tables::decode(bytes)?;
    let mut references = saved.references.into_iter();
    let mut invalid = false;
    let mut ids = std::collections::BTreeSet::new();
    if saved.history.iter().any(|record| {
        record.id == 0
            || !ids.insert(record.id)
            || record.checkpoint.as_ref().is_some_and(|checkpoint| {
                !checkpoint.history_records.is_empty() || !checkpoint.dialogue_history.is_empty()
            })
    }) {
        return Err(StorageError::InvalidSave(
            "invalid history checkpoint table".into(),
        ));
    }
    visit_heaps(
        &mut saved.current,
        &mut saved.callers,
        &mut saved.history,
        &mut |heap| {
            if *heap != ObjectHeap::default() {
                invalid = true;
            }
            match references
                .next()
                .and_then(|id| saved.heaps.get(id as usize))
            {
                Some(shared) => *heap = shared.clone(),
                None => invalid = true,
            }
        },
    );
    if invalid || references.next().is_some() {
        return Err(StorageError::InvalidSave(
            "invalid snapshot heap reference table".into(),
        ));
    }
    let mut references = saved.text_references.into_iter();
    for record in &mut saved.history {
        for text in &mut record.text {
            if !text.context.values.is_empty() {
                return Err(StorageError::InvalidSave(
                    "unexpected inline text context".into(),
                ));
            }
            text.context = references
                .next()
                .and_then(|id| saved.text_contexts.get(id as usize))
                .cloned()
                .ok_or_else(|| {
                    StorageError::InvalidSave("invalid history text context reference".into())
                })?;
        }
    }
    if references.next().is_some() {
        return Err(StorageError::InvalidSave(
            "excess history text context references".into(),
        ));
    }
    Ok((saved.current, saved.callers, saved.history))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> StoryRuntimeSnapshot {
        let code = crate::script::compile_story_bytecode(
            "alice.hks",
            "struct Player { score: Int }\nglobal var alice: Player = .{ score: 1 }\n\"Ready\"",
        )
        .expect("compile synthetic script");
        let mut runtime = crate::script::StoryRuntime::new(code).expect("runtime");
        while let Some(event) = runtime.step().expect("step") {
            if matches!(event, crate::script::StoryRuntimeEvent::Wait(_)) {
                break;
            }
        }
        runtime.snapshot().expect("snapshot")
    }

    #[test]
    fn invalid_snapshot_values_return_an_error_instead_of_panicking() {
        let records = vec![crate::state::HistoryRecord {
            id: 1,
            checkpoint: None,
            text: vec![hiraku_text::template::LocalizableText {
                key: None,
                source: "Alice".into(),
                context: std::sync::Arc::new(hiraku_text::template::TextSnapshot {
                    values: std::collections::BTreeMap::from([(
                        "score".into(),
                        std::sync::Arc::new(hiraku_text::template::TextValue::Float(f64::NAN)),
                    )]),
                }),
            }],
        }];
        let error = encode(&None, &[], &records).expect_err("non-finite values are invalid");
        assert!(
            error
                .to_string()
                .contains("failed to encode execution snapshots")
        );
        let data = crate::state::SaveGameData {
            history_records: records,
            ..Default::default()
        };
        assert!(super::super::proto::SaveGameData::try_from(&data).is_err());
    }

    #[test]
    fn repeated_snapshot_heaps_are_written_once_and_restore_as_shared_generations() {
        let snapshot = fixture();
        let callers = vec![
            ScriptCallFrameSnapshot {
                script: "alice.hks".into(),
                snapshot: snapshot.clone(),
            };
            4
        ];
        let bytes = encode(&Some(snapshot.clone()), &callers, &[]).expect("encode snapshots");
        let saved = tables::decode(&bytes).expect("shared snapshot encoding");
        assert_eq!(
            saved
                .heaps
                .iter()
                .filter(|heap| heap.live_objects() > 0)
                .count(),
            1
        );
        assert!(saved.references.len() > saved.heaps.len());
        let (mut current, mut restored_callers, mut history) = decode(&bytes).expect("decode");
        assert_eq!(current, Some(snapshot));
        assert_eq!(restored_callers, callers);
        let mut generations = Vec::new();
        visit_heaps(
            &mut current,
            &mut restored_callers,
            &mut history,
            &mut |heap| {
                if heap.live_objects() > 0 {
                    generations.push(heap.clone());
                }
            },
        );
        assert_eq!(generations.len(), 5);
        assert!(
            generations
                .iter()
                .all(|heap| heap.shares_snapshot_with(&generations[0]))
        );
        generations[0].allocate(hiraku_script::Value::Int(42));
        assert!(!generations[0].shares_snapshot_with(&generations[1]));
        assert_eq!(
            generations[0].live_objects(),
            generations[1].live_objects() + 1
        );
    }

    #[test]
    fn history_checkpoints_and_translation_contexts_restore_shared_storage() {
        let snapshot = fixture();
        let context = std::sync::Arc::new(hiraku_text::template::TextSnapshot::default());
        let records = (1..=2)
            .map(|id| crate::state::HistoryRecord {
                id,
                checkpoint: Some(std::sync::Arc::new(crate::state::SaveGameData {
                    vm_snapshot: Some(snapshot.clone()),
                    resume_script: "alice.hks".into(),
                    ..Default::default()
                })),
                text: vec![hiraku_text::template::LocalizableText {
                    key: Some("greeting".into()),
                    source: "Hello".into(),
                    context: context.clone(),
                }],
            })
            .collect::<Vec<_>>();
        let bytes = encode(&Some(snapshot.clone()), &[], &records).expect("encode history");
        let saved = tables::decode(&bytes).expect("wire format");
        assert_eq!(saved.text_contexts.len(), 1);
        let (current, _, restored) = decode(&bytes).expect("history roundtrip");
        assert_eq!(current, Some(snapshot.clone()));
        assert_eq!(restored.len(), 2);
        for record in &restored {
            let checkpoint = record.checkpoint.as_ref().expect("checkpoint");
            assert_eq!(checkpoint.vm_snapshot, Some(snapshot.clone()));
            assert!(checkpoint.history_records.is_empty());
        }
        assert!(std::sync::Arc::ptr_eq(
            &restored[0].text[0].context,
            &restored[1].text[0].context
        ));
    }

    #[test]
    fn corrupt_reference_tables_are_rejected() {
        let bytes = encode(&Some(fixture()), &[], &[]).expect("encode fixture");
        for corruption in 0..3 {
            let mut saved = tables::decode(&bytes).expect("fixture");
            match corruption {
                0 => {
                    saved.references.pop();
                }
                1 => {
                    saved.references[0] = u32::MAX;
                }
                _ => saved.references.push(0),
            }
            assert!(decode(&tables::encode(saved).expect("corrupted fixture")).is_err());
        }
    }
}
