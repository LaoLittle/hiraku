//! Platform-independent project compilation: discover sources in the embedding,
//! then collect every export before checking any module body.
use crate::{
    BuiltinManifest, Bytecode, CompileError, FunctionSignature, LinkedProgram, ModuleId, Program,
    SymbolId, SymbolManifest,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct ScriptSource {
    pub path: String,
    pub namespace: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectInterface {
    pub(crate) receiver_functions: std::collections::BTreeSet<SymbolId>,
    pub(crate) structs: Vec<crate::Stmt>,
    pub(crate) statement_hooks: std::collections::BTreeSet<SymbolId>,
    pub(crate) type_parameters: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub(crate) symbols: SymbolManifest,
    pub(crate) functions: BTreeMap<SymbolId, FunctionSignature>,
}

#[derive(Clone, Debug)]
pub struct CompiledProject {
    pub paths: BTreeMap<String, ModuleId>,
    pub program: LinkedProgram,
}

#[derive(Clone, Debug)]
pub struct ProjectError {
    pub path: String,
    pub error: CompileError,
}

/// Host-owned grants for exact source paths. Never loaded from script metadata
/// or snapshots; the embedding must authenticate its library sources first.
#[derive(Clone, Debug, Default)]
pub struct ProjectLinkPolicy {
    grants: BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl ProjectLinkPolicy {
    pub fn grant(&mut self, path: impl Into<String>, capability: impl Into<String>) {
        self.grants
            .entry(path.into())
            .or_default()
            .insert(capability.into());
    }
}

impl ProjectInterface {
    /// Compile an independent library with an existing module's symbol identities.
    /// This imports no functions and grants no native capabilities.
    pub fn with_symbols(symbols: SymbolManifest) -> Self {
        Self {
            symbols,
            ..Self::default()
        }
    }
}

pub fn compile_project_with_policy(
    sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
    policy: &ProjectLinkPolicy,
) -> Result<CompiledProject, Vec<ProjectError>> {
    compile_project_impl(sources, natives, None, Some(policy))
}

pub fn compile_project(
    sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
) -> Result<CompiledProject, Vec<ProjectError>> {
    compile_project_impl(sources, natives, None, None)
}

pub fn compile_project_with_hir_pass(
    sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
    pass: &mut dyn crate::hir::HirPass,
) -> Result<CompiledProject, Vec<ProjectError>> {
    compile_project_impl(sources, natives, Some(pass), None)
}

fn compile_project_impl(
    mut sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
    mut pass: Option<&mut dyn crate::hir::HirPass>,
    policy: Option<&ProjectLinkPolicy>,
) -> Result<CompiledProject, Vec<ProjectError>> {
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    let mut errors = Vec::new();
    let mut parsed = Vec::<Program>::new();
    let mut paths = BTreeMap::new();
    for (index, source) in sources.iter().enumerate() {
        if paths
            .insert(source.path.clone(), ModuleId(index as u32))
            .is_some()
        {
            errors.push(ProjectError {
                path: source.path.clone(),
                error: CompileError {
                    message: "duplicate script path".into(),
                    span: None,
                },
            });
        }
        match crate::parse_program(&source.source) {
            Ok(program) => parsed.push(program),
            Err(parse_errors) => {
                for error in parse_errors {
                    errors.push(ProjectError {
                        path: source.path.clone(),
                        error: CompileError {
                            message: error.message,
                            span: Some(error.span),
                        },
                    });
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut interface = ProjectInterface {
        receiver_functions: Default::default(),
        structs: Vec::new(),
        statement_hooks: Default::default(),
        type_parameters: BTreeMap::new(),
        symbols: natives.symbols().clone(),
        functions: BTreeMap::new(),
    };
    let mut type_owners = BTreeMap::new();
    for (source, program) in sources.iter().zip(&parsed) {
        for declaration in &program.statements {
            let crate::Stmt::Struct {
                exported: true,
                name,
                span,
                ..
            } = declaration
            else {
                continue;
            };
            if let Some(previous) = type_owners.insert(name.clone(), source.path.clone()) {
                errors.push(ProjectError {
                    path: source.path.clone(),
                    error: CompileError {
                        message: format!(
                            "exported type `{name}` is already declared in `{previous}`"
                        ),
                        span: Some(*span),
                    },
                });
            } else {
                interface.structs.push(declaration.clone());
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    for (source, program) in sources.iter().zip(&parsed) {
        if let Err(declarations) = crate::hir::collect_project_exports(
            program,
            source.namespace.as_deref(),
            natives,
            &mut interface,
        ) {
            errors.extend(declarations.into_iter().map(|error| ProjectError {
                path: source.path.clone(),
                error: CompileError {
                    message: error.message,
                    span: Some(error.span),
                },
            }));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut modules = Vec::<(Option<String>, Bytecode)>::new();
    for (source, program) in sources.iter().zip(&parsed) {
        let mut hash = blake3::Hasher::new();
        hash.update(source.path.as_bytes());
        hash.update(&[0]);
        hash.update(source.source.as_bytes());
        let source_hash = u64::from_le_bytes(
            hash.finalize().as_bytes()[..8]
                .try_into()
                .expect("hash has eight bytes"),
        );
        let compiled = if let Some(pass) = pass.as_deref_mut() {
            pass.begin_module(&source.path);
            crate::vm::compile_with_hir_pass(program, source_hash, natives, &interface, pass)
        } else {
            crate::vm::compile_with_project_interface(program, source_hash, natives, &interface)
        };
        match compiled {
            Ok(mut bytecode) => {
                bytecode.debug.source = Some(crate::debug::DebugSource {
                    path: source.path.clone(),
                    text: source.source.clone(),
                });
                modules.push((source.namespace.clone(), bytecode));
            }
            Err(compilation) => errors.extend(compilation.into_iter().map(|error| ProjectError {
                path: source.path.clone(),
                error,
            })),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut link_policy = crate::LinkPolicy::default();
    if let Some(policy) = policy {
        for (path, capabilities) in &policy.grants {
            let Some(module) = paths.get(path) else {
                errors.push(ProjectError {
                    path: path.clone(),
                    error: CompileError {
                        message: "capability grant refers to a missing module".into(),
                        span: None,
                    },
                });
                continue;
            };
            for capability in capabilities {
                link_policy.grant(*module, capability.clone());
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let program = crate::link_named_modules_with_policy(modules, natives, &link_policy).map_err(
        |link_errors| {
            link_errors
                .into_iter()
                .map(|error| ProjectError {
                    path: sources[error.module.0 as usize].path.clone(),
                    error: CompileError {
                        message: error.message,
                        span: None,
                    },
                })
                .collect::<Vec<_>>()
        },
    )?;
    Ok(CompiledProject { paths, program })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(path: &str, source: &str) -> ScriptSource {
        ScriptSource {
            path: path.into(),
            source: source.into(),
            namespace: None,
        }
    }

    #[test]
    fn script_owned_builder_exports_type_methods_and_commit_without_native_handles() {
        let mut registry = crate::native::NativeRegistry::<()>::new();
        let submit = registry
            .register_fn(
                "intrinsics.scene.submit",
                |_: &mut (), _: String, _: f64, _: String| Ok(()),
            )
            .expect("primitive");
        registry
            .require_capability(submit, "scene.write")
            .expect("capability");
        let library = r#"
            global struct Actor { name: String, x: Float, emotion: String, dirty: Bool }
            global fn actor(name: String) -> Actor {
                Actor.{ name: name, x: 0.0, emotion: "normal", dirty: false }
            }
            extend Actor {
                global fn at(self, x: Float) -> Actor { self.x = x; self.dirty = true; self }
                global fn e(self, emotion: String) -> Actor { self.emotion = emotion; self.dirty = true; self }
            }
            @statementCommit
            global fn onActorCommit(actor: Actor) {
                if actor.dirty {
                    intrinsics.scene.submit(actor.name, actor.x, actor.emotion)
                    actor.dirty = false
                }
            }
        "#;
        let mut policy = ProjectLinkPolicy::default();
        policy.grant("z-library.hks", "scene.write");
        let project = compile_project_with_policy(
            vec![
                source(
                    "a-entry.hks",
                    r#"
                let alice: Actor = actor("alice")
                let same = alice
                let offset = 4
                alice.at(offset).e("happy")
                same.e("sad")
            "#,
                ),
                source("z-library.hks", library),
            ],
            &registry.manifest(),
            &policy,
        )
        .expect("script type is available before its provider is compiled");
        let program = project.program;
        let mut vm =
            crate::LinkedVm::new(program.clone(), project.paths["a-entry.hks"]).expect("VM");
        let mut calls = Vec::new();
        loop {
            match vm.step().expect("script builder runs") {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    assert_eq!(
                        call.arguments[0].value,
                        crate::Value::String("alice".into())
                    );
                    assert_eq!(call.arguments[1].value, crate::Value::Number(4.0));
                    calls.push(call.arguments[2].value.clone());
                    let bytes = crate::hson::to_vec(&vm.snapshot())
                        .expect("save builder and handler frame");
                    vm = crate::LinkedVm::restore(
                        crate::hson::from_slice(&bytes).expect("decode"),
                        program.clone(),
                    )
                    .expect("restore shared script object");
                    vm.resume(crate::Value::Unit).expect("resume submit");
                }
                Some(crate::LinkedVmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(
            calls,
            vec![
                crate::Value::String("happy".into()),
                crate::Value::String("sad".into())
            ]
        );
    }

    #[test]
    fn exported_struct_identity_and_method_arguments_are_statically_checked() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let library = r#"
            global struct Actor { x: Float }
            extend Actor { global fn at(self, x: Float) -> Actor { self.x = x; self } }
        "#;
        for (entry, expected) in [
            ("let a: Actor = Actor.{ x: 0.0 }\na.at(\"bad\")", "Float"),
            (
                "struct Actor { x: Float }",
                "conflicts with an exported type",
            ),
            ("global struct Actor { x: Float }", "already declared"),
        ] {
            let errors = compile_project(
                vec![source("entry.hks", entry), source("library.hks", library)],
                &manifest,
            )
            .err()
            .expect("invalid public type usage is rejected");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains(expected)),
                "{errors:?}"
            );
        }
        compile_project(
            vec![
                source("entry.hks", "let a: Actor = Actor.{ x: 0.0 }\na.at(2)"),
                source("library.hks", library),
            ],
            &manifest,
        )
        .expect("public struct literals and contextual float literals compile");
    }

    #[test]
    fn exported_protocol_operator_links_to_a_privileged_library() {
        let mut registry = crate::native::NativeRegistry::<()>::new();
        let builtin = registry
            .register_fn(
                "intrinsics.dialogue.say",
                |_: &mut (), _: String, _: crate::native::TextTemplate| Ok(()),
            )
            .expect("register");
        registry
            .require_capability(builtin, "dialogue.write")
            .expect("capability");
        let manifest = registry.manifest();
        let library = source(
            "std/dialogue.hks",
            r#"
            extend String: Colon<TextTemplate> {
                type Output = Unit
                global fn colon(self, text: TextTemplate) { intrinsics.dialogue.say(self, text) }
            }
        "#,
        );
        let mut policy = ProjectLinkPolicy::default();
        policy.grant("std/dialogue.hks", "dialogue.write");
        let project = compile_project_with_policy(
            vec![
                source("entry.hks", "\"alice\": \"Hello ${1 + 2}\""),
                library,
            ],
            &manifest,
            &policy,
        )
        .expect("external protocol compiles and links");
        let mut vm = crate::LinkedVm::new(project.program, project.paths["entry.hks"]).expect("VM");
        loop {
            match vm.step().expect("protocol executes") {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    assert_eq!(
                        call.arguments[0].value,
                        crate::Value::String("alice".into())
                    );
                    assert_eq!(
                        call.arguments[1].value,
                        crate::Value::TextTemplate("Hello ${1 + 2}".into())
                    );
                    break;
                }
                Some(crate::LinkedVmEvent::Completed(_)) | None => panic!("expected host call"),
                _ => {}
            }
        }
    }

    #[test]
    fn exported_statement_commit_is_an_ordinary_linked_call() {
        let manifest = BuiltinManifest::new([("submit", crate::BuiltinId(1))]);
        let project = compile_project(vec![
            source("entry.hks", "4"),
            source("library.hks", "@statementCommit\nglobal fn commit(value: Int) -> Unit { submit(value)\nvalue }"),
        ], &manifest).expect("exported hook compiles");
        let mut vm = crate::LinkedVm::new(project.program, project.paths["entry.hks"]).expect("VM");
        let mut calls = 0;
        loop {
            match vm.step().expect("hook runs") {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    assert_eq!(call.arguments[0].value, crate::Value::Number(4.0));
                    calls += 1;
                    vm.resume(crate::Value::Unit).expect("host resumes");
                }
                Some(crate::LinkedVmEvent::Completed(_)) => break,
                _ => {}
            }
        }
        assert_eq!(calls, 1);
    }

    #[test]
    fn arbitrary_native_modules_share_the_same_link_permissions() {
        for module in ["intrinsics.audio", "intrinsics.graphics", "host.services"] {
            let mut registry = crate::native::NativeRegistry::<()>::new();
            let name = format!("{module}.invoke");
            let builtin = registry
                .register_fn(&name, |_: &mut (), _: String| Ok(()))
                .expect("native module function registers");
            registry
                .require_capability(builtin, "service.invoke")
                .expect("permission registers");
            let manifest = registry.manifest();
            let library = source(
                "std/service.hks",
                &format!("global fn invoke(value: String) {{ {name}(value) }}"),
            );
            let mut policy = ProjectLinkPolicy::default();
            policy.grant("std/service.hks", "service.invoke");
            compile_project_with_policy(
                vec![library.clone(), source("entry.hks", "invoke(\"alice\")")],
                &manifest,
                &policy,
            )
            .expect("authorized module wrapper links");
            let errors = compile_project_with_policy(
                vec![library, source("entry.hks", &format!("{name}(\"bob\")"))],
                &manifest,
                &policy,
            )
            .expect_err("untrusted module is denied");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains("service.invoke")),
                "{errors:?}"
            );
            let value = source("std/service.hks", &format!("let callback = {name}"));
            assert!(
                compile_project_with_policy(vec![value], &manifest, &policy).is_err(),
                "privileged functions must not escape as values"
            );
        }
    }

    #[test]
    fn module_capability_grants_follow_paths_not_input_order() {
        let mut registry = crate::native::NativeRegistry::<()>::new();
        let builtin = registry
            .register_fn("intrinsics.engine.say", |_: &mut (), _: String| Ok(()))
            .expect("native registers");
        registry
            .require_capability(builtin, "dialogue.write")
            .expect("capability registers");
        let natives = registry.manifest();
        let library = source(
            "std/dialogue.hks",
            "global fn say(text: String) { intrinsics.engine.say(text) }",
        );
        let entry = source("entry.hks", "say(\"alice\")");
        assert!(compile_project(vec![library.clone(), entry.clone()], &natives).is_err());
        let mut policy = ProjectLinkPolicy::default();
        policy.grant("std/dialogue.hks", "dialogue.write");
        for sources in [
            vec![library.clone(), entry.clone()],
            vec![entry, library.clone()],
        ] {
            compile_project_with_policy(sources, &natives, &policy)
                .expect("authorized library links");
        }
        let errors = compile_project_with_policy(
            vec![
                library,
                source("entry.hks", "intrinsics.engine.say(\"bob\")"),
            ],
            &natives,
            &policy,
        )
        .expect_err("calling a wrapper does not grant its privileges");
        assert!(errors.iter().any(
            |error| error.path == "entry.hks" && error.error.message.contains("dialogue.write")
        ));
        policy.grant("missing.hks", "dialogue.write");
        assert!(
            compile_project_with_policy(vec![source("entry.hks", "()")], &natives, &policy)
                .is_err()
        );
    }

    #[test]
    fn declarations_are_available_before_any_body_is_checked() {
        let natives = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let project = compile_project(
            vec![
                source("a.hks", "let name: String = greet(1)"),
                source(
                    "z.hks",
                    "global fn greet(index: Int) -> String { \"alice\" }",
                ),
            ],
            &natives,
        )
        .expect("consumer compiles before provider body");
        let mut vm =
            crate::LinkedVm::new(project.program, project.paths["a.hks"]).expect("entry starts");
        while vm.step().expect("cross-module call executes").is_some() {}
        for body in ["let name: Int = greet(1)", "greet(false)"] {
            assert!(
                compile_project(
                    vec![
                        source("a.hks", body),
                        source(
                            "z.hks",
                            "global fn greet(index: Int) -> String { \"alice\" }"
                        )
                    ],
                    &natives
                )
                .is_err()
            );
        }
    }

    #[test]
    fn numeric_rechecking_preserves_imported_generic_signatures_and_symbols() {
        let natives = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let project = compile_project(vec![
            source("entry.hks", r#"
                import helpers.*
                let width = 12
                let constrained: Float = width
                if identity<Float>(scale(constrained)) != 24.0 {
                    panic("imported generic call returned the wrong value")
                }
            "#),
            ScriptSource {
                path: "helpers.hks".into(), namespace: Some("helpers".into()),
                source: "global fn identity<T>(value: T) -> T { value }\nglobal fn scale(value: Float) -> Float { value * 2 }".into(),
            },
        ], &natives).expect("numeric rechecking retains the entire imported interface");
        let mut vm = crate::LinkedVm::new(project.program, project.paths["entry.hks"]).expect("VM");
        for _ in 0..1000 {
            if let Some(crate::LinkedVmEvent::Completed(value)) = vm.step().expect("execute") {
                assert_eq!(value, crate::Value::Unit);
                return;
            }
        }
        panic!("execution did not finish");
    }

    #[test]
    fn std_result_can_cross_modules() {
        let natives = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let project = compile_project(vec![
            source("entry.hks", "let value: Result<Int, String> = make()\nwhen value { .success(n) -> n\n.error(e) -> 0 }"),
            source("api.hks", "global fn make() -> Result<Int, String> { .success(42) }"),
        ], &natives).expect("std result links across modules");
        let mut vm = crate::LinkedVm::new(project.program, project.paths["entry.hks"]).expect("VM");
        for _ in 0..1000 {
            if matches!(
                vm.step().expect("execute"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
        }
        panic!("execution did not complete");
    }

    #[test]
    fn generic_module_exports_infer_result_and_check_runtime_casts() {
        let natives = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for literal in ["1", "\"alice\""] {
            let project = compile_project(
                vec![
                    source(
                        "entry.hks",
                        &format!("let result: Int = api.convert({literal})"),
                    ),
                    ScriptSource {
                        path: "api.hks".into(),
                        namespace: Some("api".into()),
                        source: "global fn convert<T>(value: Any) -> T { value as! T }".into(),
                    },
                ],
                &natives,
            )
            .expect("generic module compiles");
            let mut vm =
                crate::LinkedVm::new(project.program, project.paths["entry.hks"]).expect("VM");
            let mut failed = false;
            loop {
                match vm.step() {
                    Err(crate::LinkedVmError::Vm(crate::VmError::CastFailed(_))) => {
                        failed = true;
                        break;
                    }
                    Ok(Some(crate::LinkedVmEvent::Completed(_))) | Ok(None) => break,
                    Ok(_) => {}
                    Err(error) => panic!("{error}"),
                }
            }
            assert_eq!(failed, literal != "1");
        }
    }

    #[test]
    fn compilation_is_independent_of_discovery_order() {
        let natives = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let sources = vec![
            source("z.hks", "global fn greet() -> String { \"bob\" }"),
            source("a.hks", "let name: String = greet()"),
        ];
        let first = compile_project(sources.clone(), &natives).expect("project compiles");
        let second = compile_project(sources.into_iter().rev().collect(), &natives)
            .expect("reordered project compiles");
        assert_eq!(first.paths, second.paths);
        for (left, right) in first
            .program
            .modules
            .iter()
            .zip(second.program.modules.iter())
        {
            assert_eq!(left.bytecode, right.bytecode);
            assert_eq!(left.fingerprint, right.fingerprint);
        }
    }

    #[test]
    fn restored_null_assertion_includes_cross_module_callers() {
        let natives = BuiltinManifest::new([("checkpoint", crate::BuiltinId(1))]);
        let provider = "fn require(value: Int?) -> Int {\n    checkpoint()\n    value!\n}\nglobal fn second() -> Int { require(null) }";
        let project = compile_project(
            vec![
                source("entry.hks", "fn outer() -> Int { second() }\nouter()"),
                source("provider.hks", provider),
            ],
            &natives,
        )
        .expect("compile");
        let mut vm =
            crate::LinkedVm::new(project.program.clone(), project.paths["entry.hks"]).expect("VM");
        let mut restored = false;
        for _ in 0..1000 {
            match vm.step() {
                Ok(Some(crate::LinkedVmEvent::Call(_))) => {
                    vm = crate::LinkedVm::restore(vm.snapshot(), project.program.clone())
                        .expect("restore");
                    vm.resume(crate::Value::Unit).expect("resume");
                    restored = true;
                }
                Ok(Some(
                    crate::LinkedVmEvent::Statement(_) | crate::LinkedVmEvent::BudgetExhausted,
                )) => {}
                Err(crate::LinkedVmError::Vm(error @ crate::VmError::Panic { .. })) => {
                    assert!(restored);
                    let crate::VmError::Panic { frames, span, .. } = &error else {
                        unreachable!()
                    };
                    assert_eq!(&provider[span.range()], "value!");
                    assert_eq!(
                        frames
                            .iter()
                            .map(|frame| frame.function.as_str())
                            .collect::<Vec<_>>(),
                        ["require", "second", "outer", "entry"]
                    );
                    let report = error
                        .render_diagnostic(crate::RenderOptions::plain())
                        .expect("pretty diagnostic");
                    assert!(report.contains("non-null assertion failed"), "{report}");
                    assert!(report.contains("at require(provider.hks 3:5)"), "{report}");
                    assert_eq!(report.matches("[HKS-PANIC]").count(), 3, "{report}");
                    return;
                }
                result => panic!("expected assertion panic, got {result:?}"),
            }
        }
        panic!("assertion did not fail");
    }

    #[test]
    fn restored_cross_module_panic_pretty_prints_the_bottom_three_frames() {
        let natives = BuiltinManifest::new([("checkpoint", crate::BuiltinId(1))]);
        let project = compile_project(vec![
            source("entry.hks", "fn outer() -> Never { second() }\nouter()"),
            source("provider.hks", "fn third() -> Never {\n checkpoint()\n panic(\"failure\")\n}\nglobal fn second() -> Never { third() }"),
        ], &natives).expect("project compiles");
        let mut vm = crate::LinkedVm::new(project.program.clone(), project.paths["entry.hks"])
            .expect("entry starts");
        assert!(matches!(vm.step(), Ok(Some(crate::LinkedVmEvent::Call(_)))));
        let mut vm =
            crate::LinkedVm::restore(vm.snapshot(), project.program).expect("frames restore");
        vm.resume(crate::Value::Unit).expect("checkpoint resumes");
        loop {
            match vm.step() {
                Ok(Some(crate::LinkedVmEvent::Statement(_))) => continue,
                Err(crate::LinkedVmError::Vm(error @ crate::VmError::Panic { .. })) => {
                    let crate::VmError::Panic { frames, .. } = &error else {
                        unreachable!()
                    };
                    assert_eq!(
                        frames
                            .iter()
                            .map(|frame| frame.function.as_str())
                            .collect::<Vec<_>>(),
                        ["third", "second", "outer", "entry"]
                    );
                    assert_eq!(
                        frames[0].source.as_ref().expect("source").path,
                        "provider.hks"
                    );
                    let report = error
                        .render_diagnostic(crate::RenderOptions::plain())
                        .expect("report renders");
                    assert_eq!(report.matches("[HKS-PANIC]").count(), 3, "{report}");
                    assert!(report.contains("  at third(provider.hks 3:2)"), "{report}");
                    for name in ["second", "outer", "entry"] {
                        assert!(
                            report.contains(&format!("[HKS-PANIC] Error: at {name}")),
                            "{report}"
                        );
                    }
                    assert!(!report.contains("[HKS-PANIC] Error: at third"), "{report}");
                    assert!(
                        !report.contains("checkpoint()"),
                        "top frame must be compact: {report}"
                    );
                    break;
                }
                event => panic!("expected panic, got {event:?}"),
            }
        }
    }
}
