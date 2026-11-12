//! Read-only script API descriptions for editor and language-service integrations.
//!
//! These descriptions come from the same native registries and embedded HKS
//! modules used by the runtime. They do not instantiate a VM, grant capabilities,
//! or execute project source.

use hiraku_script::{BuiltinManifest, ScriptSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptDomain {
    Story,
    Ui,
}

pub fn script_domain(path: &str) -> Option<ScriptDomain> {
    path.ends_with(".hks").then(|| {
        if path.ends_with(".ui.hks") || path.starts_with("ui/") || path.contains("/ui/") {
            ScriptDomain::Ui
        } else {
            ScriptDomain::Story
        }
    })
}

pub fn check_story_sources(sources: Vec<ScriptSource>) -> Vec<hiraku_script::Diagnostic> {
    check_story_sources_impl(sources, None)
}

/// Runs a trusted tooling pass with the same standard library and capability
/// policy as normal story compilation. Observations are valid only when this
/// check returns no error diagnostics; the pass never executes script code.
pub fn check_story_sources_with_hir_pass(
    sources: Vec<ScriptSource>,
    pass: &mut dyn hiraku_script::hir::HirPass,
) -> Vec<hiraku_script::Diagnostic> {
    check_story_sources_impl(sources, Some(pass))
}

fn check_story_sources_impl(
    sources: Vec<ScriptSource>,
    pass: Option<&mut dyn hiraku_script::hir::HirPass>,
) -> Vec<hiraku_script::Diagnostic> {
    let mut diagnostics = Vec::new();
    for source in &sources {
        if let Ok(program) = hiraku_script::parse_program(&source.source) {
            diagnostics.extend(
                program
                    .warnings
                    .iter()
                    .map(|warning| warning.diagnostic(hiraku_script::SourceId::new(&source.path))),
            );
        }
    }
    if let Err(errors) = crate::script::compile_library_project_with_pass(sources, pass) {
        diagnostics.extend(errors.into_iter().map(|error| {
            let source = hiraku_script::SourceId::new(error.path);
            let mut diagnostic = error.error.diagnostic(source.clone());
            if diagnostic.labels.is_empty() {
                diagnostic
                    .labels
                    .push(hiraku_script::DiagnosticLabel::primary(source, 0..0));
            }
            diagnostic
        }));
    }
    diagnostics
}

pub fn check_ui_source(path: &str, source: &str) -> Vec<hiraku_script::Diagnostic> {
    let mut diagnostics = Vec::new();
    if let Ok(program) = hiraku_script::parse_program(source) {
        diagnostics.extend(
            program
                .warnings
                .iter()
                .map(|warning| warning.diagnostic(hiraku_script::SourceId::new(path))),
        );
    }
    diagnostics.extend(crate::script::check_ui_source(path, source));
    diagnostics
}

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
            sources: vec![source, crate::script::profile_source()],
        }
    } else {
        ScriptApi {
            manifest: crate::script::capabilities::story_manifest(),
            sources: vec![
                crate::script::dialogue_source(),
                crate::script::profile_source(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_project_errors_keep_cross_module_types_and_source_ranges() {
        let errors = check_story_sources(vec![
            ScriptSource {
                path: "common.hks".into(),
                namespace: None,
                source: "global fn alice() -> Int { 7 }".into(),
            },
            ScriptSource {
                path: "main.hks".into(),
                namespace: None,
                source: "let name: String = alice()".into(),
            },
        ]);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|error| {
            error.labels.iter().any(|label| {
                label.source.as_str() == "main.hks" && label.span.end > label.span.start
            })
        }));
        assert!(errors.iter().any(|error| error.message.contains("String")));
        assert!(errors.iter().all(|error| !error.message.contains('\u{1b}')));
    }
    #[test]
    fn authoring_checks_use_real_story_libraries_without_granting_user_intrinsic_access() {
        let source = |text: &str| {
            vec![ScriptSource {
                path: "alice.hks".into(),
                namespace: None,
                source: text.into(),
            }]
        };
        assert!(
            check_story_sources(source(
                "let alice = char(\"alice\"); alice: \"Hello\"; while true {} "
            ))
            .is_empty()
        );
        assert!(!check_story_sources(source("intrinsics.engine.narrate(\"Hello\")")).is_empty());
    }

    #[test]
    fn authoring_hir_observation_preserves_the_intrinsic_capability_policy() {
        #[derive(Default)]
        struct Observer(usize);
        impl hiraku_script::hir::HirPass for Observer {
            fn run<'hir>(
                &mut self,
                _: &'hir hiraku_script::HirArena,
                _: &mut hiraku_script::HirProgram<'hir>,
                _: &BuiltinManifest,
            ) -> Result<(), Vec<hiraku_script::CompileError>> {
                self.0 += 1;
                Ok(())
            }
        }
        let source = |text: &str| {
            vec![ScriptSource {
                path: "alice.hks".into(),
                namespace: None,
                source: text.into(),
            }]
        };
        let mut observer = Observer::default();
        assert!(
            check_story_sources_with_hir_pass(
                source("let alice = char(\"alice\"); alice.focus(false)"),
                &mut observer
            )
            .is_empty()
        );
        assert!(observer.0 > 0);
        assert!(
            !check_story_sources_with_hir_pass(
                source("intrinsics.engine.narrate(\"Hello\")"),
                &mut observer
            )
            .is_empty()
        );
    }
    #[test]
    fn structured_ui_errors_include_entry_rules_and_parse_errors() {
        let source = "@ui global fn alice() {}\n@ui global fn bob() {}";
        let errors = check_ui_source("alice.ui.hks", source);
        assert!(
            errors
                .iter()
                .any(|error| error.code.as_deref() == Some("HKS-UI")
                    && error.message.contains("only one"))
        );
        assert!(errors[0].labels[0].span.start >= source.find("@ui global fn bob").unwrap());
        let errors = check_ui_source("alice.ui.hks", "import ui.widgets.*\ncanvas { text( }");
        assert!(
            errors
                .iter()
                .any(|error| error.code.as_deref() == Some("HKS-PARSE"))
        );
        assert!(
            check_ui_source(
                "alice.ui.hks",
                "import ui.widgets.*\ncanvas { text(\"Alice\") }"
            )
            .is_empty()
        );
    }
    #[test]
    fn script_domains_match_runtime_project_routing() {
        for path in [
            "ui/alice.hks",
            "screens/alice.ui.hks",
            "hdp://main.hdp/ui/alice.hks",
        ] {
            assert_eq!(script_domain(path), Some(ScriptDomain::Ui));
        }
        assert_eq!(
            script_domain("scripts/alice.hks"),
            Some(ScriptDomain::Story)
        );
        assert_eq!(script_domain("alice.char.hson"), None);
    }

    #[test]
    fn persistent_objects_have_the_same_typed_api_in_both_domains() {
        for ui in [false, true] {
            let mut api = script_api(ui);
            api.sources.push(ScriptSource {
                path: "main.hks".into(),
                namespace: None,
                source: r#"
                    struct Alice { name: String, score: Int }
                    let fallback = Alice.{ name: "alice", score: 0 }
                    let loaded: Alice = profile.read("alice", fallback)
                    profile.write("alice", loaded)
                "#
                .into(),
            });
            let mut policy = hiraku_script::ProjectLinkPolicy::default();
            if !ui {
                let path = &api.sources[0].path;
                policy.grant(path, "scene.actor");
                policy.grant(path, "dialogue.write");
            }
            hiraku_script::compile_project_with_policy(api.sources, &api.manifest, &policy)
                .expect("persistent API compiles and links for both story and UI");
        }
    }

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
