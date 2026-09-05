//! Module-local literal strings. These IDs are not symbol or runtime object IDs.
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

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
