use hiraku_script::{HksHandle, native::NativeError};
use serde::{Deserialize, Serialize};

pub(crate) const NAVIGATION_HANDLE_TYPE: u32 = 4;

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
    #[serde(default)]
    pub origin: Option<String>,
}

impl NavigationRequest {
    pub fn goto(path: String) -> Result<Self, NativeError> {
        validate_path(&path)?;
        Ok(Self {
            path,
            kind: NavigationKind::Goto,
            reset: NavigationReset::None,
            origin: None,
        })
    }

    pub fn call(path: String) -> Result<Self, NativeError> {
        validate_path(&path)?;
        Ok(Self {
            path,
            kind: NavigationKind::Call,
            reset: NavigationReset::None,
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

#[derive(Clone, Copy, HksHandle)]
#[hks(name = "Navigation", handle_type = NAVIGATION_HANDLE_TYPE)]
pub(crate) struct NavigationHandle(pub(crate) u64);

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
