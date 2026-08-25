use hiraku_errors::{Diagnostic, DiagnosticLabel, RenderOptions, SourceMap, render_diagnostics};
use hiraku_script::hson::{self, HsonMap, HsonValue};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HsonDataError {
    #[error("{message}")]
    Parse { path: String, message: String },
    #[error("{message}")]
    ExpectedMap { path: String, message: String },
}

/// Parses a declarative `.hson` document using the engine-independent HSON
/// implementation from `hiraku_script`.
pub fn evaluate_hson_map(path: &str, source: &str) -> Result<HsonMap, HsonDataError> {
    match hson::parse(source).map_err(|error| HsonDataError::Parse {
        path: path.to_string(),
        message: error.render_with_options(path, source, RenderOptions::terminal()),
    })? {
        HsonValue::Map(map) => Ok(map),
        _ => {
            let mut sources = SourceMap::new();
            let source_id = sources.insert(path, source);
            let diagnostic = Diagnostic::error("expected an HSON map at the document root")
                .with_code("HSON-ROOT")
                .with_label(DiagnosticLabel::primary(source_id, 0..source.len()))
                .with_help("wrap the document fields in `.{ ... }`");
            Err(HsonDataError::ExpectedMap {
                path: path.to_string(),
                message: render_diagnostics(&[diagnostic], &sources, RenderOptions::terminal()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_declarative_hson_map() {
        let data = evaluate_hson_map(
            "settings.hson",
            ".{ startup: \"startup.hks\", fonts: .{ path: \"fonts\" } }",
        )
        .expect("declarative HSON should parse");
        assert_eq!(data["startup"].as_str(), Some("startup.hks"));
        assert_eq!(
            data["fonts"]
                .as_map()
                .and_then(|fonts| fonts["path"].as_str()),
            Some("fonts")
        );
    }

    #[test]
    fn accepts_nested_tuples_and_maps() {
        let data = evaluate_hson_map(
            "character.hson",
            ".{ slots: (\"body\", \"face\"), offset: (12.5, -3.0) }",
        )
        .expect("nested HSON should parse");
        assert_eq!(
            data["slots"].as_array().expect("slots tuple")[0].as_str(),
            Some("body")
        );
        assert_eq!(
            data["offset"].as_array().expect("offset tuple")[1].as_f64(),
            Some(-3.0)
        );
    }

    #[test]
    fn rejects_procedural_documents() {
        let error = evaluate_hson_map("settings.hson", "let startup = \"startup\"")
            .expect_err("procedural HKS must not be accepted as HSON");
        assert!(matches!(error, HsonDataError::Parse { .. }));
        let rendered = error.to_string();
        assert!(rendered.contains("[HSON-PARSE]") || rendered.contains("[HSON-VALUE]"));
        assert!(rendered.contains("settings.hson:1:"));
    }

    #[test]
    fn reports_a_non_map_root_with_source_context() {
        let error = evaluate_hson_map("settings.hson", "[1, 2, 3]")
            .expect_err("settings root must be a map");
        let rendered = error.to_string();
        assert!(rendered.contains("[HSON-ROOT]"));
        assert!(rendered.contains("settings.hson:1:1"));
        assert!(rendered.contains("wrap the document fields in `.{ ... }`"));
    }
}
