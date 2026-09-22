use hiraku_script::{
    ScriptType,
    native::{FromHksValue, HksScriptType, NativeError, NativeRegistry},
    runtime::{BuiltinCall, Value},
};
use serde::{Deserialize, Serialize};

/// Complete configuration for a terminal jump; there is no deferred builder.
pub(crate) struct NavigationOptions {
    reset: NavigationResetValue,
    preload: Option<bool>,
}

impl HksScriptType for NavigationOptions {
    fn hks_script_type<C>(registry: &mut NativeRegistry<C>) -> ScriptType {
        ScriptType::Record(std::collections::BTreeMap::from([
            (
                "reset".into(),
                Option::<NavigationResetValue>::hks_script_type(registry),
            ),
            ("preload".into(), Option::<bool>::hks_script_type(registry)),
        ]))
    }
}

impl FromHksValue for NavigationOptions {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        let Value::Map(fields) = value else {
            return Err(NativeError::message("goto options must be a record"));
        };
        if fields.keys().any(|key| key != "reset" && key != "preload") {
            return Err(NativeError::message(
                "goto options accept only `reset` and `preload`",
            ));
        }
        Ok(Self {
            reset: fields
                .get("reset")
                .map(Option::<NavigationResetValue>::from_hks_value)
                .transpose()?
                .flatten()
                .unwrap_or(NavigationResetValue::None),
            preload: fields
                .get("preload")
                .map(Option::<bool>::from_hks_value)
                .transpose()?
                .flatten(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationKind {
    Goto,
    Call,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationReset {
    #[default]
    None,
    Presentation,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationRequest {
    pub path: String,
    pub kind: NavigationKind,
    pub reset: NavigationReset,
    /// None inherits the execution context; ordinary navigation preloads.
    #[serde(default)]
    pub preload: Option<bool>,
    #[serde(default)]
    pub origin: Option<String>,
}

impl NavigationRequest {
    pub(crate) fn should_preload(&self, caller: Option<&super::StoryRuntime>) -> bool {
        self.preload.unwrap_or_else(|| {
            self.kind != NavigationKind::Call || caller.is_none_or(|story| story.preload_calls)
        })
    }

    pub(crate) fn from_goto_call(call: &BuiltinCall) -> Result<Self, NativeError> {
        if !(1..=2).contains(&call.arguments.len()) {
            return Err(NativeError::message(
                "story.goto expects a path and optional options record",
            ));
        }
        let path = String::from_hks_value(&call.arguments[0].value)?;
        let options = call
            .arguments
            .get(1)
            .map(|arg| Option::<NavigationOptions>::from_hks_value(&arg.value))
            .transpose()?
            .flatten();
        let mut request = Self::goto(path)?;
        if let Some(options) = options {
            request.reset = options.reset.into();
            request.preload = options.preload;
        }
        Ok(request)
    }

    pub fn goto(path: String) -> Result<Self, NativeError> {
        validate_path(&path)?;
        Ok(Self {
            path,
            kind: NavigationKind::Goto,
            reset: NavigationReset::None,
            preload: None,
            origin: None,
        })
    }

    pub fn call(path: String) -> Result<Self, NativeError> {
        validate_path(&path)?;
        Ok(Self {
            path,
            kind: NavigationKind::Call,
            reset: NavigationReset::None,
            preload: None,
            origin: None,
        })
    }

    pub fn with_origin(mut self, origin: Option<String>) -> Self {
        self.origin = origin;
        self
    }
}

fn validate_path(path: &str) -> Result<(), NativeError> {
    if path.trim().is_empty() {
        return Err(NativeError::message("story path must not be empty"));
    }
    Ok(())
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationResetValue {
    None,
    Presentation,
    Session,
}

impl NavigationResetValue {
    #[getter]
    fn none() -> NavigationResetValue { Self::None }
    #[getter]
    fn presentation() -> NavigationResetValue { Self::Presentation }
    #[getter]
    fn session() -> NavigationResetValue { Self::Session }
}
}

impl From<NavigationResetValue> for NavigationReset {
    fn from(value: NavigationResetValue) -> Self {
        match value {
            NavigationResetValue::None => Self::None,
            NavigationResetValue::Presentation => Self::Presentation,
            NavigationResetValue::Session => Self::Session,
        }
    }
}
