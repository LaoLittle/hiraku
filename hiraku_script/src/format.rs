//! Conservative CST formatter. Newline boundaries and token spelling are semantic
//! in HKS, so this pass never reflows statements or rewrites strings/comments.
use crate::{cst::SyntaxTree, lex::TokenKind, parse::ParseError};

#[derive(Clone, Copy, Debug)]
pub struct FormatOptions {
    pub indent_width: usize,
    pub insert_spaces: bool,
}
impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent_width: 4,
            insert_spaces: true,
        }
    }
}
pub fn format_source(source: &str, options: FormatOptions) -> Result<String, Vec<ParseError>> {
    let tree = SyntaxTree::parse(source);
    format_tree(&tree, options)
}

/// Format an existing immutable editor snapshot without lexing/parsing again.
pub fn format_tree(tree: &SyntaxTree, options: FormatOptions) -> Result<String, Vec<ParseError>> {
    if !tree.errors.is_empty() {
        return Err(tree.errors.clone());
    }
    let source = &tree.source;
    let mut output = String::new();
    let mut depth: usize = 0;
    let mut line_start = true;
    let mut space = false;
    for token in &tree.tokens {
        let text = tree.token_text(token);
        match token.kind {
            TokenKind::Whitespace => {
                if !line_start {
                    space = true;
                }
                continue;
            }
            TokenKind::NewLine => {
                // The shared lexer treats CR as whitespace and LF as NewLine.
                if text == "\n"
                    && token.span.start > 0
                    && source.as_bytes()[token.span.start - 1] == b'\r'
                    && !output.ends_with('\r')
                {
                    output.push('\r');
                }
                output.push_str(text);
                line_start = true;
                space = false;
                continue;
            }
            TokenKind::CloseBrace | TokenKind::CloseBracket | TokenKind::CloseParen => {
                depth = depth.saturating_sub(1)
            }
            _ => {}
        }
        if line_start {
            if options.insert_spaces {
                output.push_str(&" ".repeat(depth * options.indent_width.clamp(1, 16)));
            } else {
                output.push_str(&"\t".repeat(depth));
            }
        } else if space {
            output.push(' ');
        }
        output.push_str(text);
        space = false;
        line_start = text.ends_with('\n');
        if matches!(
            token.kind,
            TokenKind::OpenBrace | TokenKind::OpenBracket | TokenKind::OpenParen
        ) {
            depth += 1;
        }
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push_str(if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_library_formatting_is_idempotent_and_keeps_tokens() {
        let source = include_str!("std/core.hks");
        let formatted = format_source(source, FormatOptions::default()).expect("format script std");
        assert_eq!(
            format_source(&formatted, FormatOptions::default()).expect("second pass"),
            formatted
        );
        let significant = |source: &str| {
            let tree = SyntaxTree::parse(source);
            tree.tokens
                .iter()
                .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::NewLine))
                .map(|token| (token.kind, tree.token_text(token).to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(significant(source), significant(&formatted));
    }
    #[test]
    fn idempotent_and_preserves_nontrivia_tokens() {
        let source = "// @block(entry)\nwhile true {  \n  let  alice = \"Hello ${name ?: \"Bob\"}\"\n  actor\n .show()\n}\n";
        let result = format_source(source, FormatOptions::default()).expect("format");
        assert!(result.contains("    let alice"));
        assert_eq!(
            format_source(&result, FormatOptions::default()).expect("twice"),
            result
        );
        let tokens = |s: &str| {
            SyntaxTree::parse(s)
                .tokens
                .into_iter()
                .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::NewLine))
                .map(|t| (t.kind, s[t.span.range()].to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(tokens(source), tokens(&result));
    }
    #[test]
    fn invalid_source_is_not_rewritten_and_crlf_is_preserved() {
        assert!(format_source("let alice =", FormatOptions::default()).is_err());
        assert_eq!(
            format_source("if true {\r\n\"Hello\"\r\n}\r\n", FormatOptions::default())
                .expect("format"),
            "if true {\r\n    \"Hello\"\r\n}\r\n"
        );
        let comment = "// @block(entry)\r\n\"Hello\" // comment\r\n";
        assert_eq!(
            format_source(comment, FormatOptions::default()).expect("comment CRLF"),
            comment
        );
    }
}
