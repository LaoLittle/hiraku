//! Source metadata is separate from executable instructions and VM state.
use crate::{Span, vm::CodeLocation};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSource {
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeDebugInfo {
    pub track_caller: bool,
    pub locations: BTreeMap<usize, Span>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugInfo {
    pub source: Option<DebugSource>,
    pub entry: CodeDebugInfo,
    pub functions: Vec<CodeDebugInfo>,
    pub regions: Vec<CodeDebugInfo>,
}

impl DebugInfo {
    pub fn span(&self, code: CodeLocation, pc: usize) -> Option<Span> {
        let code = match code {
            CodeLocation::Entry => &self.entry,
            CodeLocation::Function(index) => self.functions.get(index as usize)?,
            CodeLocation::Region(index) => self.regions.get(index as usize)?,
        };
        code.locations.get(&pc).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackTraceFrame {
    pub function: String,
    pub pc: usize,
    pub span: Option<Span>,
    pub source: Option<Arc<DebugSource>>,
}

impl StackTraceFrame {
    pub fn location(&self) -> String {
        let Some(source) = &self.source else {
            return format!("{} (pc {})", self.function, self.pc);
        };
        let Some(span) = self.span else {
            return format!("{}({}:pc {})", self.function, source.path, self.pc);
        };
        let mut offset = span.start.min(source.text.len());
        while !source.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &source.text[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("{}({} {line}:{column})", self.function, source.path)
    }
}
