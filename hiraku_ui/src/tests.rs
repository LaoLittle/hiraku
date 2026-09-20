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
