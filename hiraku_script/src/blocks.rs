//! Editor metadata embedded in HKS comments without affecting runtime semantics.

use std::collections::BTreeSet;

use crate::{SymbolId, SymbolInterner, SymbolManifest, lex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub SymbolId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBlock {
    pub id: BlockId,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDocument {
    pub symbols: SymbolManifest,
    pub preamble: String,
    pub blocks: Vec<SourceBlock>,
}

impl BlockDocument {
    pub fn block_name(&self, id: BlockId) -> Option<&str> {
        self.symbols.resolve(id.0)
    }

    pub fn render(&self) -> String {
        let mut output = self.preamble.trim_end().to_string();
        for block in &self.blocks {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            let name = self
                .block_name(block.id)
                .expect("block IDs originate from this document's manifest");
            output.push_str("// @block(");
            output.push_str(name);
            output.push_str(")\n");
            output.push_str(block.source.trim());
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockDocumentError {
    EmptyId { offset: usize },
    InvalidMarker { offset: usize },
    DuplicateId(String),
}

impl std::fmt::Display for BlockDocumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyId { offset } => write!(formatter, "empty block ID at byte {offset}"),
            Self::InvalidMarker { offset } => {
                write!(formatter, "invalid block marker at byte {offset}")
            }
            Self::DuplicateId(id) => write!(formatter, "duplicate block ID `{id}`"),
        }
    }
}

impl std::error::Error for BlockDocumentError {}

/// Splits top-level `// @block(id)` comments into stable editor blocks.
/// Markers inside braces or string literals are deliberately ignored.
pub fn parse_block_document(source: &str) -> Result<BlockDocument, BlockDocumentError> {
    let mut offset = 0usize;
    let mut brace_depth = 0usize;
    let mut markers = Vec::<(String, usize, usize)>::new();
    for token in lex::tokenize(source) {
        let start = offset;
        offset += token.len as usize;
        match token.kind {
            lex::TokenKind::OpenBrace => brace_depth += 1,
            lex::TokenKind::CloseBrace => brace_depth = brace_depth.saturating_sub(1),
            lex::TokenKind::LineComment { .. } if brace_depth == 0 => {
                let comment = &source[start..offset];
                let line_start = source[..start]
                    .rfind('\n')
                    .map_or(0, |line_break| line_break + 1);
                let line_leading = source[line_start..start].trim().is_empty();
                if line_leading && let Some(id) = parse_marker(comment, start)? {
                    let content_start = source[offset..]
                        .strip_prefix("\r\n")
                        .map(|_| offset + 2)
                        .or_else(|| source[offset..].strip_prefix('\n').map(|_| offset + 1))
                        .unwrap_or(offset);
                    markers.push((id, start, content_start));
                }
            }
            _ => {}
        }
    }

    let mut interner = SymbolInterner::new();
    if markers.is_empty() {
        let id = BlockId(interner.intern("main"));
        return Ok(BlockDocument {
            symbols: interner.manifest(),
            preamble: String::new(),
            blocks: vec![SourceBlock {
                id,
                source: source.trim().to_string(),
            }],
        });
    }

    let mut seen = BTreeSet::new();
    let mut blocks = Vec::with_capacity(markers.len());
    for (index, (name, _, content_start)) in markers.iter().enumerate() {
        if !seen.insert(name.clone()) {
            return Err(BlockDocumentError::DuplicateId(name.clone()));
        }
        let end = markers
            .get(index + 1)
            .map(|(_, marker_start, _)| *marker_start)
            .unwrap_or(source.len());
        blocks.push(SourceBlock {
            id: BlockId(interner.intern(name)),
            source: source[*content_start..end].trim().to_string(),
        });
    }
    Ok(BlockDocument {
        symbols: interner.manifest(),
        preamble: source[..markers[0].1].trim().to_string(),
        blocks,
    })
}

fn parse_marker(comment: &str, offset: usize) -> Result<Option<String>, BlockDocumentError> {
    let body = comment
        .strip_prefix("//")
        .expect("called only for a line comment")
        .trim();
    if !body.starts_with("@block") {
        return Ok(None);
    }
    let Some(id) = body
        .strip_prefix("@block(")
        .and_then(|body| body.strip_suffix(')'))
        .map(str::trim)
    else {
        return Err(BlockDocumentError::InvalidMarker { offset });
    };
    if id.is_empty() {
        return Err(BlockDocumentError::EmptyId { offset });
    }
    if !id
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(BlockDocumentError::InvalidMarker { offset });
    }
    Ok(Some(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_markers_roundtrip_as_editor_metadata() {
        let source = r#"global route = "a"

// @block(intro)
"Hello"
"World"

// @block(next)
char("alice"): "Next"
"#;
        let document = parse_block_document(source).expect("block document parses");
        assert_eq!(document.preamble, "global route = \"a\"");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.block_name(document.blocks[0].id), Some("intro"));
        assert_eq!(document.block_name(document.blocks[1].id), Some("next"));
        let rendered = document.render();
        assert!(crate::parse_program(&rendered).is_ok());
        assert_eq!(
            parse_block_document(&rendered).expect("rendered blocks parse"),
            document
        );
    }

    #[test]
    fn markers_inside_control_flow_and_strings_do_not_split_blocks() {
        let source = r#"// @block(main)
if true {
    // @block(not-a-top-level-block)
    "// @block(not-a-comment)"
}
"#;
        let document = parse_block_document(source).expect("block document parses");
        assert_eq!(document.blocks.len(), 1);
        assert!(document.blocks[0].source.contains("not-a-top-level-block"));
    }

    #[test]
    fn inline_marker_text_is_an_ordinary_comment() {
        let source = "\"line\" // @block(not-metadata)\n\"next\"";
        let document = parse_block_document(source).expect("inline comment remains source");
        assert_eq!(document.blocks.len(), 1);
        assert_eq!(document.block_name(document.blocks[0].id), Some("main"));
    }

    #[test]
    fn duplicate_block_ids_are_rejected() {
        let error = parse_block_document("// @block(a)\n\"one\"\n// @block(a)\n\"two\"")
            .expect_err("duplicate IDs must be rejected");
        assert_eq!(error, BlockDocumentError::DuplicateId("a".into()));
    }
}
