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

pub fn compile_project(
    sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
) -> Result<CompiledProject, Vec<ProjectError>> {
    compile_project_impl(sources, natives, None)
}

pub fn compile_project_with_hir_pass(
    sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
    pass: &mut dyn crate::hir::HirPass,
) -> Result<CompiledProject, Vec<ProjectError>> {
    compile_project_impl(sources, natives, Some(pass))
}

fn compile_project_impl(
    mut sources: Vec<ScriptSource>,
    natives: &BuiltinManifest,
    mut pass: Option<&mut dyn crate::hir::HirPass>,
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
        type_parameters: BTreeMap::new(),
        symbols: natives.symbols().clone(),
        functions: BTreeMap::new(),
    };
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
    let program = crate::link_named_modules(modules, natives).map_err(|link_errors| {
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
    })?;
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
