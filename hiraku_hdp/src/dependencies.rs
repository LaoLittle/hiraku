//! Optional package dependency metadata. Analysis belongs to `hiraku-tools`;
//! this module only defines the wire contract consumed by asset hosts.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DEPENDENCY_MANIFEST: &str = "dependencies.manifest.hson";
pub const DEPENDENCY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DependencyManifest {
    pub version: u32,
    /// Package-relative image paths, not texture-region names.
    pub scripts: BTreeMap<String, BTreeSet<String>>,
    pub resident: BTreeSet<String>,
    /// Computed resource expressions which could not be narrowed statically.
    /// Story queries include their resource family conservatively; open-ended
    /// UI queries stay lazy rather than permanently pinning all game textures.
    pub conservative: BTreeMap<String, BTreeSet<String>>,
}
