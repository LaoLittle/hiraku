//! Versioned external-input tape. Independent of VM PCs and presentation timing.
//!
//! Playback must run against an isolated candidate scene: a compressed dialogue
//! run is authenticated at its end, not after each intermediate continuation.
use crate::state::StoredValue;
use serde::{Deserialize, Serialize};

pub const JOURNAL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InputKind {
    Choice,
    UiResult,
    Random,
    Time,
}

/// A semantic call identity, not a bytecode offset. Signature includes inputs
/// such as choice labels/enabled flags or random bounds, excluding the result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayPoint {
    pub script: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReplayEvent {
    Dialogue {
        count: u64,
        digest: String,
    },
    Input {
        kind: InputKind,
        point: ReplayPoint,
        value: StoredValue,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayJournal {
    pub version: u32,
    pub entry_script: String,
    pub random_seed: u64,
    pub events: Vec<ReplayEvent>,
    /// Exact pending boundary. An unanswered choice is a destination, not an input.
    pub destination: Option<ReplayPoint>,
    /// False for partial recordings or sessions started from a legacy checkpoint.
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ReplayError {
    #[error("unsupported replay journal version {0}")]
    Version(u32),
    #[error("save has no complete replay history from its entry script")]
    Incomplete,
    #[error("replay diverged at event {event}: {reason}")]
    Diverged { event: usize, reason: String },
    #[error("cannot encode replay point: {0}")]
    Encoding(String),
    #[error("dialogue replay counter overflow")]
    Overflow,
}

fn extend_digest(previous: &str, point: &ReplayPoint) -> Result<String, ReplayError> {
    let encoded =
        hiraku_script::hson::to_vec(point).map_err(|e| ReplayError::Encoding(e.to_string()))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"hiraku/replay/dialogue/v1\0");
    hash.update(previous.as_bytes());
    hash.update(&encoded);
    Ok(hash.finalize().to_hex().to_string())
}

impl ReplayJournal {
    pub fn new(entry_script: String, random_seed: u64) -> Self {
        Self {
            version: JOURNAL_VERSION,
            entry_script,
            random_seed,
            events: Vec::new(),
            destination: None,
            complete: false,
        }
    }

    pub fn dialogue(&mut self, point: &ReplayPoint) -> Result<(), ReplayError> {
        if let Some(ReplayEvent::Dialogue { count, digest }) = self.events.last_mut() {
            let next = count.checked_add(1).ok_or(ReplayError::Overflow)?;
            let next_digest = extend_digest(digest, point)?;
            *count = next;
            *digest = next_digest;
        } else {
            self.events.push(ReplayEvent::Dialogue {
                count: 1,
                digest: extend_digest("", point)?,
            });
        }
        self.destination = None;
        Ok(())
    }

    pub fn input(&mut self, kind: InputKind, point: ReplayPoint, value: StoredValue) {
        self.events.push(ReplayEvent::Input { kind, point, value });
        self.destination = None;
    }

    pub fn playback(&self) -> Result<ReplayCursor<'_>, ReplayError> {
        if self.version != JOURNAL_VERSION {
            return Err(ReplayError::Version(self.version));
        }
        if !self.complete {
            return Err(ReplayError::Incomplete);
        }
        Ok(ReplayCursor {
            journal: self,
            event: 0,
            dialogue_count: 0,
            digest: String::new(),
        })
    }
}

pub struct ReplayCursor<'a> {
    journal: &'a ReplayJournal,
    event: usize,
    dialogue_count: u64,
    digest: String,
}

impl ReplayCursor<'_> {
    fn mismatch(&self, reason: &str) -> ReplayError {
        ReplayError::Diverged {
            event: self.event,
            reason: reason.into(),
        }
    }

    pub fn dialogue(&mut self, point: &ReplayPoint) -> Result<(), ReplayError> {
        let Some(ReplayEvent::Dialogue { count, digest }) = self.journal.events.get(self.event)
        else {
            return Err(self.mismatch("expected a recorded dialogue continuation"));
        };
        let next = self
            .dialogue_count
            .checked_add(1)
            .ok_or(ReplayError::Overflow)?;
        let candidate = extend_digest(&self.digest, point)?;
        if next > *count || (next == *count && candidate != *digest) {
            return Err(self.mismatch("dialogue sequence changed"));
        }
        if next == *count {
            self.event += 1;
            self.dialogue_count = 0;
            self.digest.clear();
        } else {
            self.dialogue_count = next;
            self.digest = candidate;
        }
        Ok(())
    }

    /// Never invokes a random generator, clock, or UI while replaying.
    pub fn input(
        &mut self,
        kind: InputKind,
        point: &ReplayPoint,
    ) -> Result<StoredValue, ReplayError> {
        let Some(ReplayEvent::Input {
            kind: expected,
            point: recorded,
            value,
        }) = self.journal.events.get(self.event)
        else {
            return Err(self.mismatch("expected a recorded external input"));
        };
        if expected != &kind || recorded != point {
            return Err(self.mismatch("external input kind or signature changed"));
        }
        self.event += 1;
        Ok(value.clone())
    }

    pub fn finish(&self, destination: &ReplayPoint) -> Result<(), ReplayError> {
        if self.event != self.journal.events.len()
            || self.journal.destination.as_ref() != Some(destination)
        {
            return Err(self.mismatch("saved destination was not reached"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(signature: &str) -> ReplayPoint {
        ReplayPoint {
            script: "memory://scene.hks".into(),
            signature: signature.into(),
        }
    }
    #[test]
    fn ordered_inputs_and_compressed_dialogues_roundtrip() {
        let mut journal = ReplayJournal::new("memory://entry.hks".into(), 123);
        journal.input(InputKind::Random, point("rand(0,10)"), StoredValue::Int(3));
        journal.input(
            InputKind::Choice,
            point("alice|bob"),
            StoredValue::String("bob".into()),
        );
        for line in ["a", "b", "c", "d"] {
            journal.dialogue(&point(line)).expect("record dialogue");
        }
        journal.input(InputKind::Time, point("time()"), StoredValue::Float(12.5));
        journal.complete = true;
        journal.destination = Some(point("pending choice"));
        assert_eq!(journal.events.len(), 4);
        let data = hiraku_script::hson::to_vec(&journal).expect("encode journal");
        let restored: ReplayJournal =
            hiraku_script::hson::from_slice(&data).expect("decode journal");
        assert_eq!(journal, restored);
        let mut replay = restored.playback().expect("complete history");
        assert_eq!(
            replay
                .input(InputKind::Random, &point("rand(0,10)"))
                .expect("random"),
            StoredValue::Int(3)
        );
        assert!(replay.input(InputKind::Time, &point("time()")).is_err());
        assert_eq!(
            replay
                .input(InputKind::Choice, &point("alice|bob"))
                .expect("choice"),
            StoredValue::String("bob".into())
        );
        for line in ["a", "b", "c", "d"] {
            replay.dialogue(&point(line)).expect("same dialogue");
        }
        assert!(replay.finish(&point("pending choice")).is_err());
        replay
            .input(InputKind::Time, &point("time()"))
            .expect("time");
        replay
            .finish(&point("pending choice"))
            .expect("exact destination");
    }
    #[test]
    fn changed_dialogue_and_partial_histories_are_not_silently_accepted() {
        let mut journal = ReplayJournal::new("memory://entry.hks".into(), 0);
        assert!(matches!(journal.playback(), Err(ReplayError::Incomplete)));
        journal.dialogue(&point("alice")).expect("record");
        journal.complete = true;
        assert!(
            journal
                .playback()
                .expect("history")
                .dialogue(&point("bob"))
                .is_err()
        );
        journal.version += 1;
        assert!(matches!(journal.playback(), Err(ReplayError::Version(_))));
    }
}
