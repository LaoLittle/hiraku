//! Optional package dependency metadata. Analysis belongs to `hiraku-tools`;
//! this module only defines the wire contract consumed by asset hosts.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DEPENDENCY_MANIFEST: &str = "dependencies.manifest.hson";
pub const DEPENDENCY_VERSION: u32 = 3;

/// Source-indexed speculative control flow, never executable script code.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGraph {
    pub entry: Option<usize>,
    pub functions: BTreeMap<String, usize>,
    pub nodes: Vec<ResourceNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceNode {
    pub span: [usize; 2],
    pub images: BTreeSet<String>,
    pub next: Vec<usize>,
    pub calls: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DependencyManifest {
    pub windows: BTreeMap<String, ResourceGraph>,
    /// Exported function name -> owning script; local functions take precedence.
    pub exports: BTreeMap<String, String>,
    /// Estimated decoded RGBA bytes, excluding the GPU copy. Unknown formats
    /// are omitted and remain demand-loaded under bounded preloading.
    #[serde(default)]
    pub image_bytes: BTreeMap<String, u64>,
    pub version: u32,
    /// Package-relative image paths, not texture-region names.
    pub scripts: BTreeMap<String, BTreeSet<String>>,
    /// Union of statically known UI images. Hosts may warm these selectively;
    /// this is not a requirement to retain every UI texture for the session.
    pub resident: BTreeSet<String>,
    /// Computed resource expressions which could not be narrowed statically.
    /// Unresolved queries stay demand-loaded in both stories and UI rather
    /// than preloading the entire resource family.
    pub conservative: BTreeMap<String, BTreeSet<String>>,
}
