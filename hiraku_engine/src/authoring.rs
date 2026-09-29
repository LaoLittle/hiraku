//! Read-only script API descriptions for editor and language-service integrations.
//!
//! These descriptions come from the same native registries and embedded HKS
//! modules used by the runtime. They do not instantiate a VM, grant capabilities,
//! or execute project source.

use hiraku_script::{BuiltinManifest, ScriptSource};

/// Native signatures and standard-library declarations for one script domain.
///
/// Completion clients must omit native functions with a required capability
/// (and the reserved `intrinsics` namespace), since those are implementation
/// details of privileged library modules rather than author-facing operations.
/// Script sources retain their namespace so imported names resolve consistently
/// with actual compilation. Project globals and imports must be added separately.
pub struct ScriptApi {
    pub manifest: BuiltinManifest,
    pub sources: Vec<ScriptSource>,
}

/// Describe story (`false`) or declarative UI (`true`) scripting without loading
/// game assets. Reuse this result; native registration need not run per keystroke.
pub fn script_api(ui: bool) -> ScriptApi {
    if ui {
        let (manifest, source) = crate::script::ui_authoring_api();
        ScriptApi {
            manifest,
            sources: vec![source],
        }
    } else {
        ScriptApi {
            manifest: crate::script::capabilities::story_manifest(),
            sources: vec![crate::script::dialogue_source()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_description_uses_runtime_signatures_and_script_actor_declarations() {
        let api = script_api(false);
        assert_eq!(api.manifest, crate::script::capabilities::story_manifest());
        let module = &api.sources[0];
        assert!(module.namespace.is_none());
        let program = hiraku_script::parse_program(&module.source).expect("embedded story library");
        assert!(program.statements.iter().any(|statement| {
            matches!(statement, hiraku_script::Stmt::Struct { name, .. } if name == "Actor")
        }));
    }

    #[test]
    fn ui_description_exposes_widgets_and_read_only_model_types() {
        let api = script_api(true);
        assert!(api.manifest.globals().contains_key("dialogue"));
        assert!(api.manifest.globals().contains_key("time"));
        assert!(api.manifest.resolve("button").is_some());
        let module = &api.sources[0];
        assert_eq!(module.namespace.as_deref(), Some("ui.widgets"));
        hiraku_script::parse_program(&module.source).expect("embedded UI library");
    }
}
