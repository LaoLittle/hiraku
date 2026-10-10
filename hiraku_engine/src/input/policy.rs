use hiraku_script::{
    ScriptType,
    native::{FromHksValue, HksScriptType, NativeError, NativeRegistry},
    runtime::Value,
};
use serde::{Deserialize, Serialize};

use crate::state::StoredValue;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InputPermissions {
    pub advance: bool,
    pub pause: bool,
    pub fast_forward: bool,
    pub skip_movie: bool,
    pub auto: bool,
    pub visibility: bool,
    pub history: bool,
    pub rollback: bool,
}

impl Default for InputPermissions {
    fn default() -> Self {
        Self {
            advance: true,
            pause: true,
            fast_forward: true,
            skip_movie: true,
            auto: true,
            visibility: true,
            history: true,
            rollback: true,
        }
    }
}

impl InputPermissions {
    fn fields(&self) -> [(&'static str, bool); 8] {
        [
            ("advance", self.advance),
            ("pause", self.pause),
            ("fastForward", self.fast_forward),
            ("skipMovie", self.skip_movie),
            ("auto", self.auto),
            ("visibility", self.visibility),
            ("history", self.history),
            ("rollback", self.rollback),
        ]
    }

    pub(crate) fn model(&self, blocked: bool) -> StoredValue {
        StoredValue::Map(
            self.fields()
                .into_iter()
                .map(|(name, allowed)| (name.into(), StoredValue::Bool(allowed && !blocked)))
                .collect(),
        )
    }

    pub(crate) fn model_type() -> ScriptType {
        ScriptType::Record(
            Self::default()
                .fields()
                .into_iter()
                .map(|(name, _)| (name.into(), ScriptType::Bool))
                .collect(),
        )
    }
}

impl HksScriptType for InputPermissions {
    fn hks_script_type<C>(_: &mut NativeRegistry<C>) -> ScriptType {
        ScriptType::Record(
            Self::default()
                .fields()
                .into_iter()
                .map(|(name, _)| {
                    (
                        name.into(),
                        ScriptType::Optional(Box::new(ScriptType::Bool)),
                    )
                })
                .collect(),
        )
    }
}

impl FromHksValue for InputPermissions {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        let Value::Map(fields) = value else {
            return Err(NativeError::message("input permissions require a record"));
        };
        let mut result = Self::default();
        for (name, value) in fields.iter() {
            let target = match name.as_str() {
                "advance" => &mut result.advance,
                "pause" => &mut result.pause,
                "fastForward" => &mut result.fast_forward,
                "skipMovie" => &mut result.skip_movie,
                "auto" => &mut result.auto,
                "visibility" => &mut result.visibility,
                "history" => &mut result.history,
                "rollback" => &mut result.rollback,
                _ => {
                    return Err(NativeError::message(format!(
                        "unknown input permission `{name}`"
                    )));
                }
            };
            *target = Option::<bool>::from_hks_value(value)?.unwrap_or(true);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn partial_configuration_defaults_to_allowed_and_global_lock_wins() {
        let value = Value::Map(BTreeMap::from([("auto".into(), Value::Bool(false))]));
        let policy = InputPermissions::from_hks_value(&value).expect("permissions");
        assert!(!policy.auto);
        assert!(policy.advance);
        let StoredValue::Map(model) = policy.model(true) else {
            panic!("model record")
        };
        assert!(
            model
                .values()
                .all(|value| matches!(value, StoredValue::Bool(false)))
        );
        assert!(
            InputPermissions::from_hks_value(&Value::Map(BTreeMap::from([(
                "unknown".into(),
                Value::Bool(true)
            )])))
            .is_err()
        );
    }
}
