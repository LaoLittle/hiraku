//! Typed, transport-independent document analysis shared by LSP and the editor.
//! No disk IO or engine knowledge: unsaved source is always authoritative.
use hiraku_script::{
    cst::SyntaxTree,
    format::{FormatOptions, format_tree},
    span::Span,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

pub fn diagnostics(tree: &SyntaxTree) -> Vec<Diagnostic> {
    let mut result: Vec<_> = tree
        .errors
        .iter()
        .map(|error| Diagnostic {
            span: error.span,
            severity: Severity::Error,
            code: "HKS-PARSE",
            message: error.message.clone(),
        })
        .collect();
    if let Some(ast) = &tree.ast {
        result.extend(ast.warnings.iter().map(|warning| Diagnostic {
            span: warning.span,
            severity: Severity::Warning,
            code: "HKS-SYNTAX",
            message: warning.message.clone(),
        }));
    }
    result
}

/// Parse once and optionally format on a worker, returning only UI-facing data.
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    /// None when not requested or when syntax is invalid. Never a partial edit.
    pub formatted: Option<String>,
}

pub fn analyze(source: &str, formatting: Option<FormatOptions>) -> Analysis {
    let tree = SyntaxTree::parse(source);
    Analysis {
        diagnostics: diagnostics(&tree),
        formatted: formatting.and_then(|options| format_tree(&tree, options).ok()),
    }
}

pub fn analyze_hson(source: &str, formatting: Option<FormatOptions>) -> Analysis {
    let value = match hiraku_script::hson::parse(source) {
        Ok(value) => value,
        Err(error) => {
            return Analysis {
                diagnostics: error
                    .diagnostics(hiraku_script::SourceId::new("<hson>"))
                    .into_iter()
                    .map(|diagnostic| {
                        let span = diagnostic
                            .labels
                            .first()
                            .map(|label| Span::new_with_end(label.span.start, label.span.end))
                            .unwrap_or(Span::new_with_end(0, 0));
                        Diagnostic {
                            span,
                            severity: Severity::Error,
                            code: "HSON",
                            message: diagnostic.message,
                        }
                    })
                    .collect(),
                formatted: None,
            };
        }
    };
    let formatted = formatting.and_then(|options| {
        let tree = SyntaxTree::parse_literal(source);
        let output = format_tree(&tree, options).ok()?;
        (hiraku_script::hson::parse(&output).ok()? == value).then_some(output)
    });
    Analysis {
        diagnostics: vec![],
        formatted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hson_format_preserves_comments_and_inert_templates() {
        let source =
            "// Alice\r\n.{name: \"${alice}\", parts: [\"body\", \"head\"], offset: (1, 2)}\r\n";
        let result = analyze_hson(source, Some(FormatOptions::default()));
        assert!(result.diagnostics.is_empty());
        let formatted = result.formatted.expect("valid HSON should format");
        assert!(formatted.contains("// Alice\r\n"));
        assert!(formatted.contains("\"${alice}\""));
        assert_eq!(
            hiraku_script::hson::parse(source).unwrap(),
            hiraku_script::hson::parse(&formatted).unwrap()
        );
        assert_eq!(
            analyze_hson(&formatted, Some(FormatOptions::default()))
                .formatted
                .as_deref(),
            Some(formatted.as_str())
        );
    }

    #[test]
    fn hson_rejects_executable_data_without_partial_formatting() {
        for source in [
            ".{name: char(\"alice\")}",
            ".{name: alice}",
            ".{name: \"Alice\", value: }",
        ] {
            let result = analyze_hson(source, Some(FormatOptions::default()));
            assert!(!result.diagnostics.is_empty(), "{source}");
            assert!(result.formatted.is_none());
            for diagnostic in result.diagnostics {
                assert!(source.is_char_boundary(diagnostic.span.start));
                assert!(source.is_char_boundary(diagnostic.span.end));
            }
        }
    }
    #[test]
    fn invalid_draft_has_diagnostics_but_never_a_partial_format() {
        let result = analyze("let alice =", Some(FormatOptions::default()));
        assert!(!result.diagnostics.is_empty());
        assert!(result.formatted.is_none());
    }
    #[test]
    fn typed_service_and_cst_agree() {
        let source = "fn hello() {\n\"Alice\"\n}";
        let result = analyze(source, Some(FormatOptions::default()));
        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.formatted.as_deref(),
            Some("fn hello() {\n    \"Alice\"\n}\n")
        );
    }
}
