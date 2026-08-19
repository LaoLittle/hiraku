use hiraku_script::hson::{self, HsonMap, HsonValue};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HsonDataError {
    #[error("failed to parse HSON data `{path}`: {message}")]
    Parse { path: String, message: String },
    #[error("HSON data `{path}` must contain exactly one map")]
    ExpectedMap { path: String },
}

/// Parses a declarative `.hson` document using the engine-independent HSON
/// implementation from `hiraku_script`.
pub fn evaluate_hson_map(path: &str, source: &str) -> Result<HsonMap, HsonDataError> {
    match hson::parse(source).map_err(|error| HsonDataError::Parse {
        path: path.to_string(),
        message: error.to_string(),
    })? {
        HsonValue::Map(map) => Ok(map),
        _ => Err(HsonDataError::ExpectedMap {
            path: path.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_declarative_hson_map() {
        let data = evaluate_hson_map(
            "settings.hson",
            ".{ startup: \"startup.story.hks\", fonts: .{ path: \"fonts\" } }",
        )
        .expect("declarative HSON should parse");
        assert_eq!(data["startup"].as_str(), Some("startup.story.hks"));
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
    }
}
