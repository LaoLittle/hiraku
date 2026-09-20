use super::*;
use hiraku_script::{
    RenderOptions, ScriptSource, Value,
    native::{HksBindable, NativeError, NativeRegistry},
};

#[hiraku_script::hks_module]
mod api {
    use super::*;
    #[hks]
    fn display(_context: &mut (), _value: HksBindable<String>) -> Result<(), NativeError> {
        Ok(())
    }
}

fn document(source: &str) -> UiDocument {
    UiDocument::compile(
        "memory://alice.ui.hks",
        vec![ScriptSource {
            path: "memory://alice.ui.hks".into(),
            namespace: None,
            source: source.into(),
        }],
        &NativeRegistry::<()>::new().manifest(),
        RenderOptions::plain(),
    )
    .expect("compile synthetic UI document")
}

#[test]
fn entry_contract_uses_declaration_provenance_not_native_symbol_numbers() {
    use hiraku_script::{BuiltinManifest, SymbolInterner};
    let compile = |origin: &str, field: &str, prefix: bool| {
        let mut symbols = SymbolInterner::new();
        if prefix {
            symbols.intern("unrelatedHostSymbol");
            symbols.intern("anotherHostSymbol");
        }
        let manifest = BuiltinManifest::new(Vec::<(String, hiraku_script::BuiltinId)>::new())
            .with_type_metadata(symbols.manifest(), Default::default(), Vec::new());
        UiDocument::compile(
            "form.ui.hks",
            vec![
                ScriptSource {
                    path: origin.into(),
                    namespace: None,
                    source: format!("global struct FormInput {{ name: {field} }}"),
                },
                ScriptSource {
                    path: "form.ui.hks".into(),
                    namespace: None,
                    source: "@ui\nglobal fn main(input: FormInput, reply: (FormInput) -> Unit) {}"
                        .into(),
                },
            ],
            &manifest,
            RenderOptions::plain(),
        )
        .expect("compile entry contract")
    };
    let story_side = compile("contracts.hks", "String", false);
    let ui_side = compile("contracts.hks", "String", true);
    assert_ne!(
        story_side.entry_signature(),
        ui_side.entry_signature(),
        "local symbol IDs differ"
    );
    assert_eq!(
        story_side.entry_contract().expect("contract"),
        ui_side.entry_contract().expect("contract")
    );
    let story = hiraku_script::compile_project(
        vec![
            ScriptSource {
                path: "contracts.hks".into(),
                namespace: None,
                source: "global struct FormInput { name: String }".into(),
            },
            ScriptSource {
                path: "story.hks".into(),
                namespace: None,
                source: "global fn form(input: FormInput, reply: (FormInput) -> Unit) {}".into(),
            },
        ],
        &NativeRegistry::<()>::new().manifest(),
    )
    .expect("compile independent story contract");
    let module = story.paths["story.hks"];
    let symbol = story.program.modules[module.0 as usize]
        .bytecode
        .symbols
        .find("form")
        .expect("function symbol");
    assert_eq!(
        Some(
            story
                .function_contract(module, symbol)
                .expect("story contract")
        ),
        ui_side.entry_contract().expect("UI contract")
    );
    assert!(
        story
            .function_contract(hiraku_script::ModuleId(u32::MAX), symbol)
            .is_err()
    );
    assert!(
        story
            .function_contract(module, hiraku_script::SymbolId(u32::MAX))
            .is_err()
    );
    for different in [
        compile("other.hks", "String", false),
        compile("contracts.hks", "Int", false),
    ] {
        assert_ne!(
            story_side.entry_contract().expect("contract"),
            different.entry_contract().expect("contract"),
            "same type name is insufficient when provenance or schema differs"
        );
    }
}

#[test]
fn modal_contract_infers_result_from_reply_not_render_return() {
    let shared = ScriptSource {
        path: "shared/profile.hks".into(), namespace: None,
        source: "global type DisplayName = String\nglobal struct ProfileInput { name: DisplayName }\nglobal enum ProfileResult { saved(DisplayName), cancelled }".into(),
    };
    let manifest = NativeRegistry::<()>::new().manifest();
    let ui = UiDocument::compile("profile.ui.hks", vec![shared.clone(), ScriptSource {
        path: "profile.ui.hks".into(), namespace: None,
        source: "@ui\nglobal fn editProfile(input: ProfileInput, reply: (ProfileResult) -> Unit) { reply(.saved(input.name)) }".into(),
    }], &manifest, RenderOptions::plain()).expect("typed UI authoring");
    let story = hiraku_script::compile_project(
        vec![
            shared,
            ScriptSource {
                path: "story.hks".into(),
                namespace: None,
                source:
                    "global fn expectedOpen(input: ProfileInput) -> ProfileResult { .cancelled }"
                        .into(),
            },
        ],
        &manifest,
    )
    .expect("caller signature");
    let module = story.paths["story.hks"];
    let symbol = story.program.modules[module.0 as usize]
        .bytecode
        .symbols
        .find("expectedOpen")
        .expect("function");
    assert_eq!(
        ui.modal_contract().expect("derive input/result"),
        story
            .function_contract(module, symbol)
            .expect("caller contract")
    );
    assert_ne!(
        ui.entry_contract().expect("render contract"),
        Some(ui.modal_contract().expect("modal contract"))
    );

    let invalid_reply = UiDocument::compile(
        "profile.ui.hks",
        vec![ScriptSource {
            path: "profile.ui.hks".into(),
            namespace: None,
            source: "@ui\nglobal fn editProfile(reply: (String) -> Unit) { reply(123) }".into(),
        }],
        &manifest,
        RenderOptions::plain(),
    )
    .expect_err("wrong reply type must fail during compilation");
    assert!(invalid_reply.to_string().contains("String"));
}

#[test]
fn modal_contract_explains_invalid_entry_shapes() {
    for source in [
        "let name = \"Alice\"",
        "@ui\nglobal fn main() {}",
        "@ui\nglobal fn main(reply: () -> Unit) {}",
        "@ui\nglobal fn main(reply: (String, Int) -> Unit) {}",
        "@ui\nglobal fn main(reply: (String) -> Int) {}",
        "@ui\nglobal fn main(reply: (String) -> Unit) -> Int { 1 }",
    ] {
        assert!(document(source).modal_contract().is_err(), "{source}");
    }
    assert!(
        document("@ui\nglobal fn main(reply: (Unit) -> Unit) {}")
            .modal_contract()
            .is_ok()
    );
}

#[test]
fn shared_nominal_input_contract_is_checked_before_ui_execution() {
    use hiraku_script::{ScriptType, SymbolId};
    let document = UiDocument::compile(
        "form.ui.hks",
        vec![
            ScriptSource { path: "contracts.hks".into(), namespace: None,
                source: "global struct FormInput { name: String }".into() },
            ScriptSource { path: "form.ui.hks".into(), namespace: None,
                source: "global var label = \"\"\n@ui\nglobal fn main(input: FormInput) { label = input.name }".into() },
        ], &NativeRegistry::<()>::new().manifest(), RenderOptions::plain(),
    ).expect("shared declaration compiles with UI");
    let signature = document.entry_signature().expect("entry contract");
    let ScriptType::Struct { name, .. } = signature.parameters[0] else {
        panic!("nominal input");
    };
    let input = |type_id, value| Value::Typed {
        type_id,
        value: Box::new(Value::Map([("name".into(), value)].into_iter().collect())),
    };
    for invalid in [
        input(name, Value::Int(1)),
        input(SymbolId(name.0 + 1), Value::String("alice".into())),
        Value::Map(
            [("name".into(), Value::String("alice".into()))]
                .into_iter()
                .collect(),
        ),
    ] {
        assert!(
            document.invocation(vec![invalid]).is_err(),
            "invalid input must fail before rendering"
        );
    }
    let globals = compose(
        document
            .invocation(vec![input(name, Value::String("alice".into()))])
            .expect("typed input"),
        &NativeRegistry::<()>::new(),
        &mut (),
        &Default::default(),
        &document.owned_globals,
        1000,
        |_| {},
    )
    .expect("render valid input");
    assert_eq!(globals["label"], Value::String("alice".into()));
}

#[test]
fn parameterized_ui_rejects_wrong_arity_and_scalar_type_at_invocation() {
    let document = document("@ui\nglobal fn main(value: Float) {}");
    assert!(document.invocation(vec![]).is_err());
    assert!(document.invocation(vec![Value::Int(1)]).is_err());
    assert!(document.invocation(vec![Value::Number(1.0)]).is_ok());
}

#[test]
fn document_entry_receives_arguments_and_commits_state() {
    let document = document(
        "global var label = \"alice\"\n@ui\nglobal fn main(name: String) { label = name }",
    );
    let registry = NativeRegistry::<()>::new();
    let mut globals = compose(
        document
            .initializer()
            .expect("initializer VM")
            .expect("function entry initializer"),
        &registry,
        &mut (),
        &Default::default(),
        &document.owned_globals,
        1_000,
        |_| {},
    )
    .expect("initialize document");
    assert_eq!(globals["label"], Value::String("alice".into()));
    let mut commits = 0;
    globals = compose(
        document
            .invocation(vec![Value::String("bob".into())])
            .expect("entry VM"),
        &registry,
        &mut (),
        &globals,
        &document.owned_globals,
        1_000,
        |_| commits += 1,
    )
    .expect("invoke entry");
    assert_eq!(globals["label"], Value::String("bob".into()));
    assert!(commits > 0);
}

#[test]
fn file_entry_and_invocation_failures_are_explicit() {
    let document = document("global var count = 1\ncount += 1");
    assert!(document.initializer().expect("check initializer").is_none());
    assert!(matches!(
        document.invocation(vec![Value::String("alice".into())]),
        Err(UiInvocationError::MissingEntry)
    ));
    let globals = compose(
        document.invocation(vec![]).expect("file entry"),
        &NativeRegistry::<()>::new(),
        &mut (),
        &Default::default(),
        &document.owned_globals,
        1_000,
        |_| {},
    )
    .expect("execute file entry");
    assert_eq!(globals["count"], Value::Int(2));
    let endless = self::document("while true {}");
    assert!(matches!(
        compose(
            endless.invocation(vec![]).expect("loop entry"),
            &NativeRegistry::<()>::new(),
            &mut (),
            &Default::default(),
            &endless.owned_globals,
            30,
            |_| {},
        ),
        Err(CompositionError::BudgetExceeded)
    ));
    let bare = self::document("\"alice\"");
    assert!(matches!(
        compose(
            bare.invocation(vec![]).expect("bare string entry"),
            &NativeRegistry::<()>::new(),
            &mut (),
            &Default::default(),
            &bare.owned_globals,
            100,
            |_| {},
        ),
        Err(CompositionError::BareString)
    ));
}

#[test]
fn composition_cannot_write_host_owned_state() {
    let document =
        document("global var label = \"alice\"\n@ui\nglobal fn main() { label = \"bob\" }");
    let globals = [("label".into(), Value::String("alice".into()))]
        .into_iter()
        .collect();
    let error = compose(
        document.invocation(vec![]).expect("entry"),
        &NativeRegistry::<()>::new(),
        &mut (),
        &globals,
        &Default::default(),
        1_000,
        |_| {},
    )
    .expect_err("a mount cannot write globals it does not own");
    assert!(matches!(error, CompositionError::Vm(_)));
    assert_eq!(globals["label"], Value::String("alice".into()));
}

#[test]
fn offline_compilation_checks_entrypoint_rules_and_locations() {
    for source in [
        "@ui\nfn main() {}",
        "@ui\nglobal fn alice() {}\n@ui\nglobal fn bob() {}",
    ] {
        let error = UiDocument::compile(
            "memory://alice.ui.hks",
            vec![ScriptSource {
                path: "memory://alice.ui.hks".into(),
                namespace: None,
                source: source.into(),
            }],
            &NativeRegistry::<()>::new().manifest(),
            RenderOptions::plain(),
        )
        .expect_err("invalid UI entry must fail before mounting");
        assert!(error.to_string().contains("entrypoint"));
    }
    let error = UiDocument::compile(
        "memory://bob.ui.hks",
        vec![ScriptSource {
            path: "memory://bob.ui.hks".into(),
            namespace: None,
            source: "let value: String = true".into(),
        }],
        &NativeRegistry::<()>::new().manifest(),
        RenderOptions::plain(),
    )
    .expect_err("type mismatch");
    assert!(error.to_string().contains("memory://bob.ui.hks"));
    assert!(error.to_string().contains("String"));
}

#[test]
fn typed_pass_extracts_properties_and_keeps_type_errors() {
    let mut registry = NativeRegistry::<()>::new();
    api::register_hks(&mut registry).expect("register property primitive");
    let manifest = registry.manifest();
    let mut pass = UiCompiler::default();
    let source = ScriptSource {
        path: "memory://alice.hks".into(),
        namespace: None,
        source: "global var label = \"alice\"\ndisplay(label)".into(),
    };
    let first = hiraku_script::project::compile_project_with_hir_pass(
        vec![source.clone()],
        &manifest,
        &mut pass,
    )
    .expect("compile");
    let plan = &pass.plans["memory://alice.hks"];
    assert_eq!(
        plan.sites
            .iter()
            .filter(|s| s.kind == RegionKind::Property)
            .count(),
        1
    );
    assert!(!plan.structural_globals.contains("label"));
    assert!(plan.read_globals.contains("label"));
    assert!(!plan.read_globals.contains("time"));
    let mut next = UiCompiler::default();
    let second =
        hiraku_script::project::compile_project_with_hir_pass(vec![source], &manifest, &mut next)
            .expect("compile again");
    assert_eq!(
        first.program.modules[0]
            .bytecode
            .fingerprint()
            .expect("fingerprint"),
        second.program.modules[0]
            .bytecode
            .fingerprint()
            .expect("fingerprint")
    );
    let invalid = ScriptSource {
        path: "memory://bob.hks".into(),
        namespace: None,
        source: "display(123)".into(),
    };
    assert!(
        hiraku_script::project::compile_project_with_hir_pass(vec![invalid], &manifest, &mut next)
            .is_err(),
        "property lifting must never bypass ordinary argument type checking"
    );
}
