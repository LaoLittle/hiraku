//! Module-local literal strings. These IDs are not symbol or runtime object IDs.
use indexmap::IndexSet;
use lasso::{Spur, ThreadedRodeo};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// Shared runtime identity, deliberately absent from bytecode and snapshots.
/// Only literals and symbols are inserted; transient computed strings are not.
#[derive(Clone, Debug)]
pub struct SharedStrings(Arc<ThreadedRodeo>);

impl Default for SharedStrings {
    fn default() -> Self {
        static POOL: OnceLock<SharedStrings> = OnceLock::new();
        POOL.get_or_init(|| Self::with_budget(8 * 1024 * 1024))
            .clone()
    }
}

impl SharedStrings {
    pub fn with_budget(bytes: usize) -> Self {
        Self(Arc::new(ThreadedRodeo::with_capacity_and_memory_limits(
            lasso::Capacity::new(
                0,
                std::num::NonZeroUsize::new(bytes.clamp(1, 4096)).expect("nonzero bucket"),
            ),
            lasso::MemoryLimits::for_memory_usage(bytes.max(1)),
        )))
    }

    pub(crate) fn intern(&self, value: &str) -> Option<Spur> {
        // The byte budget does not include hash table entries. Bound these too.
        self.0.get(value).or_else(|| {
            (self.0.len() < 65_536)
                .then(|| self.0.try_get_or_intern(value).ok())
                .flatten()
        })
    }
    pub(crate) fn get(&self, value: &str) -> Option<Spur> {
        self.0.get(value)
    }
    pub(crate) fn resolve(&self, key: Spur) -> &str {
        self.0.resolve(&key)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn prepare(&self, strings: &StringPool, symbols: &crate::SymbolManifest) {
        for value in strings.strings().iter().chain(symbols.symbols()) {
            self.intern(value);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StringId(pub u32);

/// Serialized with its bytecode; IDs follow deterministic first-use order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    pub fn get(&self, id: StringId) -> Option<&str> {
        self.strings.get(id.0 as usize).map(String::as_str)
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }
}

#[derive(Default)]
pub(crate) struct StringPoolBuilder(IndexSet<String>);

impl StringPoolBuilder {
    pub fn intern(&mut self, value: String) -> StringId {
        let (index, _) = self.0.insert_full(value);
        StringId(u32::try_from(index).expect("string pool exceeds u32 capacity"))
    }

    pub fn finish(self) -> StringPool {
        StringPool {
            strings: self.0.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_shared_across_threads() {
        let pool = SharedStrings::with_budget(4096);
        let threads = (0..8)
            .map(|_| {
                let pool = pool.clone();
                std::thread::spawn(move || pool.intern("alice").expect("pool capacity"))
            })
            .collect::<Vec<_>>();
        let keys = threads
            .into_iter()
            .map(|thread| thread.join().expect("thread succeeds"))
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| *key == keys[0]));
        assert_eq!(pool.len(), 1);
        assert!(SharedStrings::default().shares_storage_with(&SharedStrings::default()));
    }

    #[test]
    fn literals_are_exact_and_module_local() {
        let mut first = StringPoolBuilder::default();
        let literal = first.intern("Hello, alice 🌸".into());
        assert_eq!(first.intern("Hello, alice 🌸".into()), literal);
        assert_ne!(first.intern("hello, alice 🌸".into()), literal);
        let first = first.finish();
        let mut second = StringPoolBuilder::default();
        let other = second.intern("bob".into());
        assert_eq!(
            literal, other,
            "IDs are local, not process-global identities"
        );
        assert_eq!(first.get(literal), Some("Hello, alice 🌸"));
        assert_eq!(second.finish().get(other), Some("bob"));
        assert_eq!(first.get(StringId(u32::MAX)), None);
    }
}
