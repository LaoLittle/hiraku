use std::{collections::BTreeMap, sync::Arc};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position, exported_module};
use thiserror::Error;

use crate::{state::StoredValue, texture::TextureCatalog, ui::ScreenSpec};

use super::{parse_screen_spec, ui};

/// Immutable story data exposed to UI scripts during one render invocation.
#[derive(Clone, Debug, Default)]
pub struct UiContext {
    story: BTreeMap<String, StoredValue>,
}

impl UiContext {
    pub fn new(story: BTreeMap<String, StoredValue>) -> Self {
        Self { story }
    }

    pub fn story_value(&self, key: &str) -> Option<&StoredValue> {
        self.story.get(key)
    }
}

/// A semantic UI result. The story runtime, not the UI runtime, decides its effect.
#[derive(Clone, Debug, PartialEq)]
pub struct UiIntent {
    pub screen: String,
    pub value: StoredValue,
}

#[derive(Debug, Error)]
pub enum UiScriptError {
    #[error("failed to evaluate UI script: {0}")]
    Evaluation(String),
    #[error("UI script did not return a screen map: {0}")]
    InvalidScreen(String),
}

/// Evaluates a UI-only Rhai script into a declarative screen tree.
///
/// The engine deliberately registers only UI constructors and read-only story getters.
/// Story commands, ECS access, and mutable runtime state are not available here.
pub fn evaluate_ui_script(
    source: &str,
    context: &UiContext,
    textures: &TextureCatalog,
) -> Result<ScreenSpec, UiScriptError> {
    let story = Arc::new(context.story.clone());
    let mut engine = Engine::new_raw();
    engine.register_static_module("ui", exported_module!(ui::Ui).into());
    register_story_getters(&mut engine, story);

    let screen = engine
        .eval::<Map>(source)
        .map_err(|error| UiScriptError::Evaluation(error.to_string()))?;
    parse_screen_spec(textures, screen)
        .map_err(|error| UiScriptError::InvalidScreen(error.to_string()))
}

fn register_story_getters(engine: &mut Engine, story: Arc<BTreeMap<String, StoredValue>>) {
    let string_values = story.clone();
    engine.register_fn(
        "story_string",
        move |key: ImmutableString| -> Result<ImmutableString, Box<EvalAltResult>> {
            match string_values.get(key.as_str()) {
                Some(StoredValue::String(value)) => Ok(value.clone().into()),
                Some(_) => Err(ui_error(format!("story variable `{key}` is not a string"))),
                None => Err(ui_error(format!("story variable `{key}` was not found"))),
            }
        },
    );

    let bool_values = story.clone();
    engine.register_fn(
        "story_bool",
        move |key: ImmutableString| -> Result<bool, Box<EvalAltResult>> {
            match bool_values.get(key.as_str()) {
                Some(StoredValue::Bool(value)) => Ok(*value),
                Some(_) => Err(ui_error(format!("story variable `{key}` is not a bool"))),
                None => Err(ui_error(format!("story variable `{key}` was not found"))),
            }
        },
    );

    let int_values = story.clone();
    engine.register_fn(
        "story_int",
        move |key: ImmutableString| -> Result<i64, Box<EvalAltResult>> {
            match int_values.get(key.as_str()) {
                Some(StoredValue::Int(value)) => Ok(*value),
                Some(_) => Err(ui_error(format!("story variable `{key}` is not an int"))),
                None => Err(ui_error(format!("story variable `{key}` was not found"))),
            }
        },
    );

    engine.register_fn(
        "story_float",
        move |key: ImmutableString| -> Result<f64, Box<EvalAltResult>> {
            match story.get(key.as_str()) {
                Some(StoredValue::Float(value)) => Ok(*value),
                Some(StoredValue::Int(value)) => Ok(*value as f64),
                Some(_) => Err(ui_error(format!("story variable `{key}` is not numeric"))),
                None => Err(ui_error(format!("story variable `{key}` was not found"))),
            }
        },
    );
}

fn ui_error(message: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(message),
        Position::NONE,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_a_screen_with_read_only_story_values() {
        let context = UiContext::new(BTreeMap::from([
            ("affection".to_string(), StoredValue::Int(7)),
            ("route".to_string(), StoredValue::String("ema".to_string())),
        ]));
        let screen = evaluate_ui_script(
            r#"
                let affection = story_int("affection");
                let route = story_string("route");
                ui::screen(#{ title: route }, [
                    ui::text(`Affection: ${affection}`, #{}),
                    ui::button("Back", "back", #{}),
                ])
            "#,
            &context,
            &TextureCatalog::default(),
        )
        .unwrap();

        assert_eq!(screen.title.as_deref(), Some("ema"));
        assert_eq!(context.story_value("affection"), Some(&StoredValue::Int(7)));
    }

    #[test]
    fn rejects_story_commands_in_ui_scripts() {
        let error = evaluate_ui_script(
            r#"narrate("not available")"#,
            &UiContext::default(),
            &TextureCatalog::default(),
        )
        .unwrap_err();
        assert!(matches!(error, UiScriptError::Evaluation(_)));
    }

    #[test]
    fn current_settings_ui_reads_the_runtime_volume() {
        let context = UiContext::new(BTreeMap::from([(
            "bgm_volume".to_string(),
            StoredValue::Float(0.8),
        )]));
        let source =
            include_str!("../../../../manosabars/assets/main_hdp_contents/ui/settings.ui.rhai");
        let screen = evaluate_ui_script(source, &context, &TextureCatalog::default()).unwrap();
        assert_eq!(screen.title.as_deref(), Some("Settings (BGM: 0.8)"));
    }
}
