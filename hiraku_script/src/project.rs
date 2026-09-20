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
    pub(crate) types: Vec<crate::Stmt>,
    pub(crate) statement_hooks: std::collections::BTreeSet<SymbolId>,
    pub(crate) type_parameters: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub(crate) symbols: SymbolManifest,
    pub(crate) functions: BTreeMap<SymbolId, FunctionSignature>,
    pub(crate) globals: BTreeMap<String, (crate::ScriptType, bool)>,
}

#[derive(Clone, Debug)]
pub struct CompiledProject {
    pub paths: BTreeMap<String, ModuleId>,
    pub program: LinkedProgram,
    /// Nominal declaration provenance, scoped to each compiled module.
    /// These paths are compiler inputs, not runtime symbol or heap identities.
    pub type_origins: BTreeMap<ModuleId, BTreeMap<String, String>>,
}

impl CompiledProject {
    /// Resolve a script definition's contract without exporting native
    /// capabilities or assuming another program uses the same SymbolIds.
    pub fn function_contract(
        &self,
        module: ModuleId,
        symbol: SymbolId,
    ) -> Result<crate::contract::FunctionContract, crate::contract::ContractError> {
        use crate::contract::{ContractError, FunctionContract};
        let bytecode = &self
            .program
            .modules
            .get(module.0 as usize)
            .ok_or(ContractError::MissingModule(module))?
            .bytecode;
        let signature = &bytecode
            .functions
            .iter()
            .find(|function| function.name == symbol)
            .ok_or(ContractError::MissingFunction { module, symbol })?
            .signature;
        let origins = self
            .type_origins
            .get(&module)
            .ok_or(ContractError::MissingModule(module))?;
        FunctionContract::new(signature, &bytecode.symbols, origins)
    }
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
        types: Vec::new(),
        statement_hooks: Default::default(),
        type_parameters: BTreeMap::new(),
        symbols: natives.symbols().clone(),
        functions: BTreeMap::new(),
        globals: BTreeMap::new(),
    };
    let mut type_owners = BTreeMap::new();
    for (source, program) in sources.iter().zip(&parsed) {
        for declaration in &program.statements {
            let (crate::Stmt::Struct {
                exported: true,
                name,
                span,
                ..
            }
            | crate::Stmt::Enum {
                exported: true,
                name,
                span,
                ..
            }
            | crate::Stmt::TypeAlias {
                exported: true,
                name,
                span,
                ..
            }) = declaration
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
                interface.types.push(declaration.clone());
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
    let mut type_origins = BTreeMap::new();
    for (source, parsed) in sources.iter().zip(&parsed) {
        let mut origins = type_owners.clone();
        for declaration in &parsed.statements {
            match declaration {
                crate::Stmt::Struct { name, .. } | crate::Stmt::Enum { name, .. } => {
                    origins.insert(name.clone(), source.path.clone());
                }
                _ => {}
            }
        }
        type_origins.insert(paths[&source.path], origins);
    }
    Ok(CompiledProject {
        paths,
        program,
        type_origins,
    })
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
    fn typed_project_globals_preserve_binding_rules_across_modules() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let common = "global var score: Int = 0\nglobal fn initialize() { score = 1 }\nglobal fn increase() { score += 1 }";
        let project = compile_project(
            vec![
                source("common.hks", common),
                source(
                    "story.hks",
                    "initialize()\nincrease()\nscore += 3\nglobal let result = score",
                ),
            ],
            &manifest,
        )
        .expect("shared globals compile");
        let mut vm =
            crate::LinkedVm::new(project.program.clone(), project.paths["story.hks"]).expect("VM");
        for _ in 0..1000 {
            let event = vm.step_with_budget(&mut 1).expect("run shared state");
            vm = crate::LinkedVm::restore(vm.snapshot(), project.program.clone())
                .expect("restore shared state");
            if matches!(event, Some(crate::LinkedVmEvent::Completed(_))) {
                break;
            }
        }
        assert_eq!(
            vm.current_globals().expect("globals")["result"],
            crate::Value::Int(5)
        );
        let errors = compile_project(
            vec![
                source("common.hks", "global let name: String = \"Alice\""),
                source("story.hks", "name = \"Bob\""),
            ],
            &manifest,
        )
        .expect_err("shared let is immutable");
        assert!(
            errors
                .iter()
                .any(|error| error.error.message.contains("immutable")),
            "{errors:?}"
        );
    }

    #[test]
    fn shared_enums_and_aliases_link_without_copying_declarations() {
        let project = compile_project(vec![
            source("contracts.hks", "global enum Reply<T> { accepted(T), cancelled }\nglobal type Name = String\nglobal struct Input { name: Name }"),
            source("form.hks", "global fn accept(input: Input) -> Reply<Name> { .accepted(input.name) }"),
            source("story.hks", "global let answer = when accept(Input.{ name: \"Alice\" }) { .accepted(name) -> name, .cancelled -> \"Bob\" }"),
        ], &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new())).expect("shared data types compile");
        let mut vm =
            crate::LinkedVm::new(project.program.clone(), project.paths["story.hks"]).expect("VM");
        for _ in 0..1000 {
            if matches!(
                vm.step().expect("execute shared enum"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                break;
            }
        }
        assert_eq!(
            vm.current_globals().expect("globals").get("answer"),
            Some(&crate::Value::String("Alice".into()))
        );
        let module = project.paths["form.hks"];
        let symbol = project.program.modules[module.0 as usize]
            .bytecode
            .symbols
            .find("accept")
            .expect("export");
        project
            .function_contract(module, symbol)
            .expect("enum provenance is available");
    }

    #[test]
    fn exported_types_have_one_owner_across_all_declaration_kinds() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for conflicting in [
            "global enum Input { cancelled }",
            "global type Input = String",
            "struct Input {}",
            "enum Input { cancelled }",
            "type Input = String",
        ] {
            let errors = compile_project(
                vec![
                    source("contracts.hks", "global struct Input {}"),
                    source("form.hks", conflicting),
                ],
                &manifest,
            )
            .expect_err("shared type cannot be redefined or shadowed");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains("type `Input`")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn explicit_static_and_instance_properties_execute_and_restore() {
        let project = compile_project(
            vec![source(
                "main.hks",
                r#"
            struct MyFoo { a: Int }
            global var recorded = 0
            extend MyFoo {
                let instance: MyFoo = .{ a: 1 }
                let base = 2
                let limit = base + 3
                var staticCalc: Int {
                    get() { 12 }
                    set(val) { recorded = val }
                }
                var nonStaticCalc: Int {
                    get(self) { self.a }
                    set(self, newVal) { self.a = newVal }
                }
                var readOnly: Int { get(self) { self.a } }
            }
            let foo: MyFoo = .instance
            let alias = foo
            if foo.nonStaticCalc != 1 { panic("constant initializer") }
            alias.nonStaticCalc = 7
            alias.nonStaticCalc += 2
            if foo.readOnly != 9 { panic("shared instance setter") }
            let again: MyFoo = .instance
            if again.a != 9 { panic("static object was reconstructed") }
            MyFoo.staticCalc = 6
            if recorded != 6 { panic("static setter") }
            MyFoo.staticCalc += 1
            if recorded != 13 { panic("static compound assignment") }
            if MyFoo.limit != 5 { panic("constant dependency") }
        "#,
            )],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect("explicit accessors compile");
        let program = project.program;
        let mut vm =
            crate::LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
        for _ in 0..2_000 {
            if matches!(
                vm.step_with_budget(&mut 1).expect("accessors execute"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
            vm =
                crate::LinkedVm::restore(vm.snapshot(), program.clone()).expect("restore accessor");
        }
        panic!("script did not complete");
    }

    #[test]
    fn static_objects_survive_linked_calls_collection_and_restore() {
        let project = compile_project(
            vec![
                source(
                    "main.hks",
                    r#"
                    let first = shared()
                    first.a = 42
                    let second = shared()
                    if second.a != 42 { panic("linked static lost identity") }
                    second.a = 7
                    if first.a != 7 { panic("linked static was copied") }
                "#,
                ),
                source(
                    "library.hks",
                    r#"
                    struct MyFoo { a: Int }
                    extend MyFoo { let instance: MyFoo = .{ a: 1 } }
                    global fn shared() -> MyFoo { MyFoo.instance }
                "#,
                ),
            ],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect("linked static compiles");
        let program = project.program;
        let mut vm =
            crate::LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
        for _ in 0..2_000 {
            if matches!(
                vm.step_with_budget(&mut 1).expect("static executes"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
            vm.collect_objects(&[]).expect("collect linked roots");
            vm = crate::LinkedVm::restore(vm.snapshot(), program.clone()).expect("restore static");
        }
        panic!("script did not complete");
    }

    #[test]
    fn accessor_receivers_and_removed_declarations_are_checked() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for members in [
            "var value: Int { get(self) { self.a } set(value) {} }",
            "var value: Int { get() { 1 } set(self, value) {} }",
            "var value: Int { get() { 1 } set() {} }",
            "var value: Int { get(self) { 1 } set(self) {} }",
            "var value: Int { get(self) { 1 } set(self, self) {} }",
            "var value: Int { get(other) { 1 } }",
            "var value: Int = 1",
            "var value: Int { get { 1 } }",
            "var value: Int { 1 }",
            "const value = 1",
            "@getter fn value() -> Int { 1 }",
        ] {
            let source = format!("struct MyFoo {{ a: Int }}\nextend MyFoo {{ {members} }}");
            assert!(
                compile_project(vec![self::source("main.hks", &source)], &manifest).is_err(),
                "accepted {members}"
            );
        }
        for source in [
            "const value = 1",
            "global const value = 1",
            "struct MyFoo {}\nextend MyFoo { let value = 1 }\nMyFoo.value = 2",
            "struct MyFoo {}\nextend MyFoo { var value: Int { get() { 1 } set(value) {} } }\nMyFoo.value = \"bad\"",
        ] {
            assert!(
                compile_project(vec![self::source("main.hks", source)], &manifest).is_err(),
                "accepted {source}"
            );
        }
    }

    #[test]
    fn associated_types_are_qualified_by_protocol_in_bodies_and_signatures() {
        let mut registry = crate::native::NativeRegistry::<Vec<String>>::new();
        registry
            .register_fn("print", |output: &mut Vec<String>, value: String| {
                output.push(value);
                Ok(())
            })
            .expect("register print");
        let project = compile_project(
            vec![source(
                "main.hks",
                r#"
            enum Item { a(Int) }
            protocol Read { type Output
                fn read(self) -> Self.Output
            }
            protocol Notify { type Output
                fn notify(self) -> Output
            }
            extend Item: Read {
                type Output = Int
                fn read(self) -> Self.Output {
                    let copy: Self = self
                    let callback: (Self.Output) -> Output = { value: Output -> value }
                    let values: List<Self.Output> = [12]
                    when copy { .a(value) -> callback(value) }
                }
            }
            extend Item: Notify {
                type Output = Unit
                fn notify(self) -> Self.Output {
                    let value: Self.Output = ()
                    let callback: (Output) -> Self.Output = { value: Self.Output -> value }
                    print("notified")
                    callback(value)
                }
            }
            extend Item {
                fn make() -> Self { .a(12) }
                fn identity(value: Self) -> Self { value }
            }
            let item: Item = .make()
            let value: Int = item.read()
            if value != 12 { panic("wrong Read.Output") }
            let unit: Unit = item.notify()
        "#,
            )],
            &registry.manifest(),
        )
        .expect("qualified associated types compile");
        let program = project.program;
        let mut vm =
            crate::LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
        let mut output = Vec::new();
        let mut completed = false;
        for _ in 0..2_000 {
            match vm
                .step_with_budget(&mut 1)
                .expect("qualified types execute")
            {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    let value = registry.call(&mut output, &call).expect("native call");
                    vm.resume(value).expect("resume");
                }
                Some(crate::LinkedVmEvent::Completed(_)) => {
                    completed = true;
                    break;
                }
                _ => {}
            }
            vm = crate::LinkedVm::restore(vm.snapshot(), program.clone())
                .expect("restore projection-free state");
        }
        assert!(completed);
        assert_eq!(output, ["notified"]);
    }

    #[test]
    fn qualified_associated_signatures_export_as_concrete_types() {
        let project = compile_project(
            vec![
                source(
                    "library.hks",
                    r#"
                global struct Item { value: Int }
                protocol Read { type Output; fn read(self) -> Self.Output }
                extend Item: Read {
                    type Output = Int
                    global fn read(self) -> Output {
                        let result: Self.Output = self.value
                        result
                    }
                }
                global fn fetch(item: Item) -> Int { item.read() }
            "#,
                ),
                source(
                    "main.hks",
                    r#"
                let item: Item = .{ value: 12 }
                let value: Int = fetch(item)
                if value != 12 { panic("exported projection") }
            "#,
                ),
            ],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect("concrete exported associated signature");
        let read = project
            .program
            .modules
            .iter()
            .flat_map(|module| {
                module.bytecode.functions.iter().filter(|function| {
                    module
                        .bytecode
                        .symbols
                        .resolve(function.name)
                        .is_some_and(|name| name.ends_with("::protocol#Read#read"))
                })
            })
            .next()
            .expect("exported protocol method");
        assert_eq!(read.signature.result, crate::ScriptType::Int);
        let mut vm =
            crate::LinkedVm::new(project.program, project.paths["main.hks"]).expect("entry");
        for _ in 0..1_000 {
            if matches!(
                vm.step().expect("exported projection executes"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
        }
        panic!("script did not complete");
    }

    #[test]
    fn associated_type_cycles_and_unqualified_projections_are_errors() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for (script, message) in [
            (
                "struct Item {}\nprotocol Read { type Output; fn read(self) -> Self.Output }\nextend Item: Read { type Output = Self.Output; fn read(self) -> Self.Output { () } }",
                "cyclic associated type",
            ),
            (
                "struct Item {}\nextend Item { fn read(self) -> Self.Output { () } }",
                "projection requires",
            ),
            (
                "struct Item {}\nprotocol Read { type Output; fn read(self) -> Self.Output }\nextend Item: Read { type Output = Int; fn read(self) -> Self.Output { let bad: Self.Missing = 1; 1 } }",
                "no associated type `Missing`",
            ),
        ] {
            let errors = compile_project(vec![source("main.hks", script)], &manifest)
                .expect_err("invalid projection");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains(message)),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn contextual_static_members_and_self_alias_execute() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        let project = compile_project(
            vec![source(
                "main.hks",
                r#"
            enum TestEnum { a(Int), b }
            extend TestEnum {
                fn someStatic() -> Self { .a(12) }
                var defaultValue: Self { get() { Self.someStatic() } }
                fn identity(value: Self) -> Self { value }
                fn test(self) -> Int { when self { .a(value) -> value, .b -> 0 } }
            }
            let value: TestEnum = .someStatic()
            if value.test() != 12 { panic("static method") }
            let defaultValue: TestEnum = .defaultValue
            if defaultValue.test() != 12 { panic("static getter") }
            let same: TestEnum = .identity(.someStatic())
            if same.test() != 12 { panic("Self parameter") }
            let optional: TestEnum? = .someStatic()
            if optional!.test() != 12 { panic("optional context") }
            if TestEnum.someStatic().test() != 12 { panic("qualified method") }
            if TestEnum.defaultValue.test() != 12 { panic("qualified getter") }
            struct Point { x: Int }
            extend Point {
                fn make(value: Int) -> Self { .{ x: value } }
                var origin: Self { get() { .make(0) } }
                fn copy(value: Self?) -> Self? { value }
            }
            let point: Point = .make(42)
            let origin: Point = .origin
            if point.x != 42 || origin.x != 0 { panic("struct static members") }
            let copied: Point? = Point.copy(point)
            if copied!.x != 42 { panic("nested Self type") }
        "#,
            )],
            &manifest,
        )
        .expect("contextual static members compile");
        let mut vm =
            crate::LinkedVm::new(project.program, project.paths["main.hks"]).expect("entry");
        for _ in 0..1_000 {
            if matches!(
                vm.step().expect("static members execute"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
        }
        panic!("script did not complete");
    }

    #[test]
    fn imported_static_members_use_exported_self_signatures() {
        let project = compile_project(
            vec![
                source(
                    "library.hks",
                    r#"
                global struct Point { x: Int }
                extend Point {
                    global fn make(x: Int) -> Self { .{ x: x } }
                    global var origin: Self { get() { .make(0) } }
                }
            "#,
                ),
                source(
                    "main.hks",
                    r#"
                let point: Point = .make(12)
                let origin: Point = .origin
                if point.x != 12 || origin.x != 0 { panic("imported static member") }
            "#,
                ),
            ],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect("exported static signatures");
        let mut vm =
            crate::LinkedVm::new(project.program, project.paths["main.hks"]).expect("entry");
        for _ in 0..1_000 {
            if matches!(
                vm.step().expect("imported member executes"),
                Some(crate::LinkedVmEvent::Completed(_))
            ) {
                return;
            }
        }
        panic!("script did not complete");
    }

    #[test]
    fn leading_dot_requires_context_and_getters_reject_parameters() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for (script, message) in [
            (
                "enum Item { a }\nextend Item { fn make() -> Self { .a } }\nlet value = .make()",
                "cannot infer",
            ),
            (
                "struct Item {}\nextend Item { @getter fn value(a: Int) -> Self { .{} } }",
                "@getter has been removed",
            ),
            ("let value: Self = 1", "unknown type `Self`"),
        ] {
            let errors = compile_project(vec![source("main.hks", script)], &manifest)
                .expect_err("invalid static member use");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains(message)),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn unit_callables_reject_explicit_non_unit_returns() {
        let manifest = BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new());
        for script in [
            "fn invalid() -> Unit { return 7 }",
            "let invalid: () -> Unit = { return 7 }",
        ] {
            let errors = compile_project(vec![source("main.hks", script)], &manifest)
                .expect_err("only implicit tail values may be discarded");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains("expected Unit, got Int")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn unit_callable_results_discard_tail_values_but_keep_effects() {
        let mut registry = crate::native::NativeRegistry::<Vec<String>>::new();
        registry
            .register_fn("print", |output: &mut Vec<String>, value: String| {
                output.push(value);
                Ok(())
            })
            .expect("register print");
        let project = compile_project(
            vec![source(
                "main.hks",
                r#"
            type Ccc = (Int) -> () -> ()
            let cc: Ccc = { a: Int -> { a } }
            fn printu(u: Unit) { print("Unit") }
            let value: Unit = cc(1)()
            printu(value)
            fn effect() -> Int { print("effect"); 42 }
            fn discard() -> Unit { effect() }
            @inline
            fn inlineDiscard() -> Unit { effect() }
            let callback: () -> Unit = { effect() }
            printu(discard())
            printu(inlineDiscard())
            printu(callback())
            let inferred = { 7 }
            print(inferred().toString())
        "#,
            )],
            &registry.manifest(),
        )
        .expect("unit callables compile");
        let program = project.program;
        let mut vm =
            crate::LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
        let mut output = Vec::new();
        let mut completed = false;
        for _ in 0..2_000 {
            match vm.step_with_budget(&mut 1).expect("typed unit result") {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    vm = crate::LinkedVm::restore(vm.snapshot(), program.clone())
                        .expect("restore host wait");
                    let value = registry.call(&mut output, &call).expect("native call");
                    vm.resume(value).expect("resume");
                }
                Some(crate::LinkedVmEvent::Completed(_)) => {
                    completed = true;
                    break;
                }
                _ => {}
            }
            vm = crate::LinkedVm::restore(vm.snapshot(), program.clone())
                .expect("restore instruction");
        }
        assert!(completed);
        assert_eq!(
            output,
            [
                "Unit", "effect", "Unit", "effect", "Unit", "effect", "Unit", "7"
            ]
        );
    }

    #[test]
    fn imported_function_values_link_without_a_direct_call() {
        for entry in [
            "let callback: () -> Int = value; if callback() != 7 { panic(\"wrong callback\") }",
            "let callback = value; let forwarded = callback; if forwarded() != 7 { panic(\"wrong callback\") }",
        ] {
            let project = compile_project(
                vec![
                    source("main.hks", entry),
                    source("library.hks", "global fn value() -> Int { 7 }"),
                ],
                &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
            )
            .expect("function value links");
            let program = project.program;
            let mut vm =
                crate::LinkedVm::new(program.clone(), project.paths["main.hks"]).expect("entry");
            let mut completed = false;
            for _ in 0..1_000 {
                if matches!(
                    vm.step_with_budget(&mut 1)
                        .expect("invoke imported function"),
                    Some(crate::LinkedVmEvent::Completed(_))
                ) {
                    completed = true;
                    break;
                }
                vm = crate::LinkedVm::restore(vm.snapshot(), program.clone())
                    .expect("restore callable state");
            }
            assert!(completed);
        }
    }

    #[test]
    fn missing_enum_method_is_not_reported_as_an_any_call() {
        let errors = compile_project(
            vec![source(
                "main.hks",
                r#"
            let result: Result<Int, String> = .success(67)
            result.toString()
        "#,
            )],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect_err("Result does not implement toString");
        assert!(
            errors.iter().any(|error| error
                .error
                .message
                .contains("unknown method `toString` for `Result`")),
            "{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|error| error.error.message.contains("cannot call Any"))
        );
    }

    #[test]
    fn global_declaration_reports_local_name_collision() {
        let errors = compile_project(
            vec![source(
                "main.hks",
                r#"
            let result: Result<Int, String> = .success(67)
            global let result: Int = 7
        "#,
            )],
            &BuiltinManifest::new(Vec::<(String, crate::BuiltinId)>::new()),
        )
        .expect_err("ambiguous binding");
        assert!(
            errors.iter().any(|error| error
                .error
                .message
                .contains("global `result` conflicts with an existing local binding")),
            "{errors:?}"
        );
    }

    #[test]
    fn global_callable_result_retains_primitive_methods() {
        let mut registry = crate::native::NativeRegistry::<Vec<String>>::new();
        registry
            .register_fn("print", |output: &mut Vec<String>, value: String| {
                output.push(value);
                Ok(())
            })
            .expect("register print");
        let project = compile_project(
            vec![source(
                "main.hks",
                r#"
            global fn apply(callback: () -> Int) -> Int { callback() }
            let unused = 0
            let callback: () -> Int = { return 7 }
            global let result: Int = apply(callback)
            print(result.toString())
        "#,
            )],
            &registry.manifest(),
        )
        .expect("global result compiles");
        let mut vm =
            crate::LinkedVm::new(project.program, project.paths["main.hks"]).expect("entry");
        let mut output = Vec::new();
        loop {
            match vm.step().expect("execution") {
                Some(crate::LinkedVmEvent::Call(call)) => {
                    let value = registry.call(&mut output, &call).expect("native call");
                    vm.resume(value).expect("resume");
                }
                Some(crate::LinkedVmEvent::Completed(_)) | None => break,
                _ => {}
            }
        }
        assert_eq!(output, ["7"]);
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
                    assert_eq!(call.arguments[0].value, crate::Value::Int(4));
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
