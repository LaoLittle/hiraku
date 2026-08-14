use rhai::{Dynamic, Engine, Map};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RhaiDataError {
    #[error("failed to evaluate Rhai data `{path}`: {message}")]
    Evaluation { path: String, message: String },
    #[error("Rhai data `{path}` must evaluate to a map")]
    ExpectedMap { path: String },
}

/// Evaluates an authored data document without registering gameplay APIs.
///
/// `eval_expression` deliberately restricts documents to one expression. This
/// keeps the format declarative while retaining Rhai's maps and arrays.
pub fn evaluate_rhai_map(path: &str, source: &str) -> Result<Map, RhaiDataError> {
    let mut engine = Engine::new();
    engine.set_max_operations(50_000);
    engine.set_max_call_levels(16);
    engine.set_max_expr_depths(64, 64);

    let value =
        engine
            .eval_expression::<Dynamic>(source)
            .map_err(|error| RhaiDataError::Evaluation {
                path: path.to_string(),
                message: error.to_string(),
            })?;
    value
        .try_cast::<Map>()
        .ok_or_else(|| RhaiDataError::ExpectedMap {
            path: path.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_declarative_map_expression() {
        let data = evaluate_rhai_map("settings.rhai", "#{ startup: \"startup.rhai\" }")
            .expect("map expression should load");

        assert_eq!(data["startup"].clone_cast::<String>(), "startup.rhai");
    }

    #[test]
    fn rejects_non_map_data() {
        let error = evaluate_rhai_map("settings.rhai", "[1, 2, 3]")
            .expect_err("arrays cannot be root data documents");

        assert!(matches!(error, RhaiDataError::ExpectedMap { .. }));
    }

    #[test]
    fn rejects_procedural_documents() {
        let error = evaluate_rhai_map("settings.rhai", "let startup = \"startup.rhai\"; startup")
            .expect_err("data documents must be one expression");

        assert!(matches!(error, RhaiDataError::Evaluation { .. }));
    }
}
