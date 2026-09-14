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

#[cfg(test)]
mod tests {
    use super::*;
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
