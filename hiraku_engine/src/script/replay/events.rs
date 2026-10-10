use super::ReplayEvent;
use serde::{Deserialize, Serialize};
use std::{ops::Index, sync::Arc};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReplayEvents(Arc<Vec<Arc<ReplayEvent>>>);

impl ReplayEvents {
    pub(crate) fn from_shared(events: Vec<Arc<ReplayEvent>>) -> Self {
        Self(Arc::new(events))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&ReplayEvent> {
        self.0.get(index).map(Arc::as_ref)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &ReplayEvent> + ExactSizeIterator {
        self.0.iter().map(Arc::as_ref)
    }

    pub(crate) fn iter_shared(&self) -> impl Iterator<Item = &Arc<ReplayEvent>> {
        self.0.iter()
    }

    pub fn push(&mut self, event: ReplayEvent) {
        Arc::make_mut(&mut self.0).push(Arc::new(event));
    }

    pub(super) fn last_mut(&mut self) -> Option<&mut ReplayEvent> {
        Arc::make_mut(&mut self.0).last_mut().map(Arc::make_mut)
    }
}

impl Index<usize> for ReplayEvents {
    type Output = ReplayEvent;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_share_events_and_detach_only_modified_data() {
        let mut events = ReplayEvents::default();
        events.push(ReplayEvent::Dialogue {
            count: 1,
            digest: "alice".into(),
        });
        let snapshot = events.clone();
        assert!(Arc::ptr_eq(&events.0, &snapshot.0));
        events.push(ReplayEvent::Dialogue {
            count: 1,
            digest: "bob".into(),
        });
        assert_eq!(snapshot.len(), 1);
        assert!(Arc::ptr_eq(&events.0[0], &snapshot.0[0]));
        let before = events.clone();
        let Some(ReplayEvent::Dialogue { count, .. }) = events.last_mut() else {
            panic!("dialogue");
        };
        *count = 2;
        assert!(matches!(
            before.get(1),
            Some(ReplayEvent::Dialogue { count: 1, .. })
        ));
        assert!(matches!(
            events.get(1),
            Some(ReplayEvent::Dialogue { count: 2, .. })
        ));
        assert!(Arc::ptr_eq(&events.0[0], &before.0[0]));
    }

    #[test]
    fn shared_events_keep_the_plain_sequence_wire_format() {
        let mut events = ReplayEvents::default();
        events.push(ReplayEvent::Dialogue {
            count: 2,
            digest: "alice".into(),
        });
        let bytes = hiraku_script::hson::to_vec(&events).expect("encode shared events");
        let plain: Vec<ReplayEvent> =
            hiraku_script::hson::from_slice(&bytes).expect("ordinary sequence");
        assert_eq!(plain.as_slice(), &[events[0].clone()]);
        let restored: ReplayEvents =
            hiraku_script::hson::from_slice(&bytes).expect("shared sequence");
        assert_eq!(restored, events);
    }
}
