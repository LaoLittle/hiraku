use super::*;
use crate::{
    script::replay::{ReplayEvent, ReplayEvents, ReplayJournal, ReplayPoint},
    state::SaveGameData,
};
use std::{collections::HashMap, sync::Arc};

#[derive(Serialize, Deserialize)]
struct CallFrame {
    script: String,
    snapshot: u32,
}

#[derive(Serialize, Deserialize)]
struct History {
    id: u64,
    checkpoint: Option<u32>,
    text: Vec<hiraku_text::template::LocalizableText>,
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    state: SaveGameData,
    snapshot: Option<u32>,
    callers: Vec<CallFrame>,
    replay: Option<Journal>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    entry_script: String,
    random_seed: u64,
    destination: Option<ReplayPoint>,
    complete: bool,
    tail: Option<u32>,
}

#[derive(PartialEq, Serialize, Deserialize)]
struct ReplayNode {
    previous: Option<u32>,
    event: Arc<ReplayEvent>,
}

#[derive(Default)]
struct ReplayLookup {
    content: HashMap<[u8; 32], Vec<u32>>,
    shared: HashMap<(Option<u32>, usize), (Arc<ReplayEvent>, u32)>,
}

#[derive(Serialize, Deserialize)]
struct Tables {
    format: u32,
    heaps: Vec<ObjectHeap>,
    references: Vec<u32>,
    snapshots: Vec<StoryRuntimeSnapshot>,
    current: Option<u32>,
    callers: Vec<CallFrame>,
    history: Vec<History>,
    checkpoints: Vec<Checkpoint>,
    replay_nodes: Vec<ReplayNode>,
    text_contexts: Vec<Arc<hiraku_text::template::TextSnapshot>>,
    text_references: Vec<u32>,
}

fn invalid(message: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidSave(format!("invalid snapshot tables: {message}"))
}

fn index(len: usize) -> Result<u32, StorageError> {
    u32::try_from(len).map_err(|_| invalid("index exceeds u32"))
}

impl Tables {
    fn snapshot(&mut self, snapshot: StoryRuntimeSnapshot) -> Result<u32, StorageError> {
        if let Some(id) = self.snapshots.iter().position(|s| s == &snapshot) {
            return index(id);
        }
        let id = index(self.snapshots.len())?;
        self.snapshots.push(snapshot);
        Ok(id)
    }

    fn callers(
        &mut self,
        frames: Vec<ScriptCallFrameSnapshot>,
    ) -> Result<Vec<CallFrame>, StorageError> {
        frames
            .into_iter()
            .map(|frame| {
                Ok(CallFrame {
                    script: frame.script,
                    snapshot: self.snapshot(frame.snapshot)?,
                })
            })
            .collect()
    }

    fn journal(
        &mut self,
        journal: ReplayJournal,
        lookup: &mut ReplayLookup,
    ) -> Result<Journal, StorageError> {
        let mut tail = None;
        for event in journal.events.iter_shared() {
            let identity = (tail, Arc::as_ptr(event) as usize);
            if let Some((_, id)) = lookup.shared.get(&identity) {
                tail = Some(*id);
                continue;
            }
            let node = ReplayNode {
                previous: tail,
                event: event.clone(),
            };
            let bytes = bhson::to_vec(&node).map_err(invalid)?;
            let hash = *blake3::hash(&bytes).as_bytes();
            let candidates = lookup.content.entry(hash).or_default();
            let id = match candidates
                .iter()
                .copied()
                .find(|id| self.replay_nodes[*id as usize] == node)
            {
                Some(id) => id,
                None => {
                    let id = index(self.replay_nodes.len())?;
                    self.replay_nodes.push(node);
                    candidates.push(id);
                    id
                }
            };
            lookup.shared.insert(identity, (event.clone(), id));
            tail = Some(id);
        }
        Ok(Journal {
            version: journal.version,
            entry_script: journal.entry_script,
            random_seed: journal.random_seed,
            destination: journal.destination,
            complete: journal.complete,
            tail,
        })
    }
}

pub(super) fn encode(saved: SharedSnapshots) -> Result<Vec<u8>, StorageError> {
    let mut tables = Tables {
        format: 1,
        heaps: saved.heaps,
        references: saved.references,
        snapshots: Vec::new(),
        current: None,
        callers: Vec::new(),
        history: Vec::new(),
        checkpoints: Vec::new(),
        replay_nodes: Vec::new(),
        text_contexts: saved.text_contexts,
        text_references: saved.text_references,
    };
    tables.current = saved.current.map(|s| tables.snapshot(s)).transpose()?;
    tables.callers = tables.callers(saved.callers)?;
    let mut journals = ReplayLookup::default();
    let mut checkpoints = HashMap::new();
    for record in saved.history {
        let checkpoint = record
            .checkpoint
            .map(|checkpoint| {
                let identity = Arc::as_ptr(&checkpoint) as usize;
                if let Some((_, id)) = checkpoints.get(&identity) {
                    return Ok(*id);
                }
                let mut state = Arc::unwrap_or_clone(checkpoint.clone());
                if !state.history_records.is_empty() || !state.dialogue_history.is_empty() {
                    return Err(invalid("nested history checkpoint"));
                }
                let snapshot = state
                    .vm_snapshot
                    .take()
                    .map(|s| tables.snapshot(s))
                    .transpose()?;
                let callers = tables.callers(std::mem::take(&mut state.script_call_stack))?;
                let replay = state
                    .replay
                    .take()
                    .map(|j| tables.journal(j, &mut journals))
                    .transpose()?;
                let id = index(tables.checkpoints.len())?;
                tables.checkpoints.push(Checkpoint {
                    state,
                    snapshot,
                    callers,
                    replay,
                });
                // Retain the Arc as well as its address: allocation addresses can
                // otherwise be recycled while later records are being processed.
                checkpoints.insert(identity, (checkpoint, id));
                Ok(id)
            })
            .transpose()?;
        tables.history.push(History {
            id: record.id,
            checkpoint,
            text: record.text,
        });
    }
    bhson::to_vec(&tables).map_err(|error| {
        StorageError::InvalidSave(format!("failed to encode execution snapshots: {error}"))
    })
}

pub(super) fn decode(bytes: &[u8]) -> Result<SharedSnapshots, StorageError> {
    let tables: Tables = bhson::from_slice(bytes).map_err(invalid)?;
    if tables.format != 1 {
        return Err(invalid("unsupported table version"));
    }
    for (id, node) in tables.replay_nodes.iter().enumerate() {
        if node
            .previous
            .is_some_and(|previous| previous as usize >= id)
        {
            return Err(invalid("replay references must point backwards"));
        }
    }
    let snapshot = |id: u32| {
        tables
            .snapshots
            .get(id as usize)
            .cloned()
            .ok_or_else(|| invalid("unknown snapshot index"))
    };
    let callers = |frames: Vec<CallFrame>| {
        frames
            .into_iter()
            .map(|frame| {
                Ok(ScriptCallFrameSnapshot {
                    script: frame.script,
                    snapshot: snapshot(frame.snapshot)?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()
    };
    let mut checkpoints = Vec::new();
    for checkpoint in tables.checkpoints {
        let mut state = checkpoint.state;
        if state.vm_snapshot.is_some()
            || !state.script_call_stack.is_empty()
            || state.replay.is_some()
            || !state.history_records.is_empty()
            || !state.dialogue_history.is_empty()
        {
            return Err(invalid("unexpected inline checkpoint state"));
        }
        state.vm_snapshot = checkpoint.snapshot.map(snapshot).transpose()?;
        state.script_call_stack = callers(checkpoint.callers)?;
        state.replay = checkpoint
            .replay
            .map(|journal| {
                let mut events = Vec::new();
                let mut tail = journal.tail;
                while let Some(id) = tail {
                    let node = tables
                        .replay_nodes
                        .get(id as usize)
                        .ok_or_else(|| invalid("unknown replay index"))?;
                    events.push(node.event.clone());
                    tail = node.previous;
                }
                events.reverse();
                Ok::<_, StorageError>(ReplayJournal {
                    version: journal.version,
                    entry_script: journal.entry_script,
                    random_seed: journal.random_seed,
                    destination: journal.destination,
                    complete: journal.complete,
                    events: ReplayEvents::from_shared(events),
                })
            })
            .transpose()?;
        checkpoints.push(Arc::new(state));
    }
    let history = tables
        .history
        .into_iter()
        .map(|record| {
            Ok(crate::state::HistoryRecord {
                id: record.id,
                text: record.text,
                checkpoint: record
                    .checkpoint
                    .map(|id| {
                        checkpoints
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| invalid("unknown checkpoint index"))
                    })
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    Ok(SharedSnapshots {
        heaps: tables.heaps,
        references: tables.references,
        current: tables.current.map(snapshot).transpose()?,
        callers: callers(tables.callers)?,
        history,
        text_contexts: tables.text_contexts,
        text_references: tables.text_references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> SharedSnapshots {
        SharedSnapshots {
            heaps: Vec::new(),
            references: Vec::new(),
            current: None,
            callers: Vec::new(),
            history: Vec::new(),
            text_contexts: Vec::new(),
            text_references: Vec::new(),
        }
    }

    #[test]
    fn history_replay_prefixes_are_stored_once_and_roundtrip_exactly() {
        let mut saved = empty();
        let mut journal = ReplayJournal::new("alice.hks".into(), 42);
        for id in 1..=100 {
            journal.events.push(ReplayEvent::Input {
                kind: crate::script::replay::InputKind::Choice,
                point: ReplayPoint {
                    script: "alice.hks".into(),
                    signature: format!("choice-{id}"),
                },
                value: crate::state::StoredValue::String("bob".repeat(100)),
            });
            saved.history.push(crate::state::HistoryRecord {
                id,
                checkpoint: Some(Arc::new(SaveGameData {
                    replay: Some(journal.clone()),
                    ..Default::default()
                })),
                text: Vec::new(),
            });
        }
        let inline_size = bhson::to_vec(&saved).expect("baseline").len();
        let expected = saved.history.clone();
        let bytes = encode(saved).expect("indexed encoding");
        let wire: Tables = bhson::from_slice(&bytes).expect("tables");
        assert_eq!(wire.replay_nodes.len(), 100, "not 5050 repeated events");
        assert!(
            bytes.len() * 5 < inline_size,
            "{} vs {inline_size}",
            bytes.len()
        );
        let restored = decode(&bytes).expect("decode");
        let first = restored.history[0]
            .checkpoint
            .as_ref()
            .expect("first checkpoint");
        let last = restored.history[99]
            .checkpoint
            .as_ref()
            .expect("last checkpoint");
        assert!(
            Arc::ptr_eq(
                first
                    .replay
                    .as_ref()
                    .expect("first journal")
                    .events
                    .iter_shared()
                    .next()
                    .expect("first event"),
                last.replay
                    .as_ref()
                    .expect("last journal")
                    .events
                    .iter_shared()
                    .next()
                    .expect("shared event"),
            ),
            "restoring prefixes must not duplicate event payloads"
        );
        for (before, after) in expected.iter().zip(&restored.history) {
            assert_eq!(
                before.checkpoint.as_ref().expect("before").replay,
                after.checkpoint.as_ref().expect("after").replay
            );
        }
    }

    #[test]
    fn repeated_snapshots_use_indices_and_invalid_indices_are_rejected() {
        let code =
            crate::script::compile_story_bytecode("alice.hks", "\"Ready\"").expect("compile");
        let mut story = crate::script::StoryRuntime::new(code).expect("runtime");
        while let Some(event) = story.step().expect("step") {
            if matches!(event, crate::script::StoryRuntimeEvent::Wait(_)) {
                break;
            }
        }
        let snapshot = story.snapshot().expect("snapshot");
        let mut saved = empty();
        saved.current = Some(snapshot.clone());
        saved.callers = vec![
            ScriptCallFrameSnapshot {
                script: "alice.hks".into(),
                snapshot
            };
            4
        ];
        let bytes = encode(saved).expect("encode");
        let mut wire: Tables = bhson::from_slice(&bytes).expect("tables");
        assert_eq!(wire.snapshots.len(), 1);
        assert!(
            wire.callers
                .iter()
                .all(|frame| Some(frame.snapshot) == wire.current)
        );
        wire.current = Some(u32::MAX);
        assert!(decode(&bhson::to_vec(&wire).expect("corrupt fixture")).is_err());
        wire.current = None;
        wire.replay_nodes.push(ReplayNode {
            previous: Some(0),
            event: Arc::new(ReplayEvent::Dialogue {
                count: 1,
                digest: "alice".into(),
            }),
        });
        assert!(decode(&bhson::to_vec(&wire).expect("cyclic fixture")).is_err());
    }
}
