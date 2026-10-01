mod document;
mod library;
mod parser;
mod runtime;
pub mod template;

pub use document::{Document, Ruby, TextStyle};
pub use parser::{parse, parse_with_snapshot};

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} at byte {}", .range.start)]
pub struct TextError {
    pub message: String,
    pub range: Range<usize>,
}

impl TextError {
    pub(crate) fn new(message: impl Into<String>, range: Range<usize>) -> Self {
        Self {
            message: message.into(),
            range,
        }
    }

    pub fn diagnostic(&self, source: hiraku_errors::SourceId) -> hiraku_errors::Diagnostic {
        hiraku_errors::Diagnostic::error(self.message.clone())
            .with_code("TEXT")
            .with_label(hiraku_errors::DiagnosticLabel::primary(
                source,
                self.range.clone(),
            ))
    }
}

pub fn character_count(source: &str) -> usize {
    parse(source).map_or_else(|_| source.chars().count(), |document| document.styles.len())
}
