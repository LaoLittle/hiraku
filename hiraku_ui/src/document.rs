use std::collections::BTreeSet;

use hiraku_script::{
    BuiltinManifest, LinkedProgram, LinkedVm, LinkedVmError, ModuleId, RenderOptions, ScriptSource,
    SourceMap, SymbolId, Value, parse_program, render_diagnostics,
};

use crate::{CompositionPlan, UiCompiler};

/// A reusable compiled UI document. Widget schemas and library sources belong
/// to the embedding application; this crate does not prescribe a widget set.
#[derive(Clone, Debug)]
pub struct UiDocument {
    pub program: LinkedProgram,
    pub entry: ModuleId,
    pub entry_symbol: Option<SymbolId>,
    pub owned_globals: BTreeSet<String>,
    pub plan: CompositionPlan,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UiCompileError(pub String);

impl UiDocument {
    pub fn compile(
        path: &str,
        sources: Vec<ScriptSource>,
        manifest: &BuiltinManifest,
        options: RenderOptions,
    ) -> Result<Self, UiCompileError> {
        let source = sources
            .iter()
            .find(|source| source.path == path)
            .ok_or_else(|| {
                UiCompileError(format!("UI document `{path}` is missing from the project"))
            })?;
        let mut source_map = SourceMap::new();
        for source in &sources {
            source_map.insert(&source.path, &source.source);
        }
        let id = source_map.insert(path, &source.source);
        let parsed = parse_program(&source.source).map_err(|errors| {
            UiCompileError(render_diagnostics(
                &errors
                    .into_iter()
                    .map(|error| error.diagnostic(id.clone()))
                    .collect::<Vec<_>>(),
                &source_map,
                options,
            ))
        })?;
        let entries = parsed
            .statements
            .iter()
            .filter_map(|statement| {
                let hiraku_script::Stmt::Function {
                    attributes,
                    exported,
                    name,
                    ..
                } = statement
                else {
                    return None;
                };
                attributes
                    .iter()
                    .any(|attribute| attribute.name == "ui")
                    .then_some((*exported, name.clone()))
            })
            .collect::<Vec<_>>();
        if entries.len() > 1 {
            return Err(UiCompileError(
                "a UI module may declare only one `@ui` entrypoint".into(),
            ));
        }
        if entries.first().is_some_and(|(exported, _)| !exported) {
            return Err(UiCompileError(
                "the `@ui` entrypoint must be declared with `global fn`".into(),
            ));
        }
        let mut compiler = UiCompiler::default();
        let project = hiraku_script::project::compile_project_with_hir_pass(
            sources.clone(),
            manifest,
            &mut compiler,
        )
        .map_err(|errors| {
            let diagnostics = errors
                .into_iter()
                .map(|error| {
                    let source = sources.iter().find(|source| source.path == error.path);
                    let id = source_map.insert(
                        &error.path,
                        source.map(|source| source.source.as_str()).unwrap_or(""),
                    );
                    error.error.diagnostic(id)
                })
                .collect::<Vec<_>>();
            UiCompileError(render_diagnostics(&diagnostics, &source_map, options))
        })?;
        let entry = project.paths[path];
        let bytecode = &project.program.modules[entry.0 as usize].bytecode;
        let owned_globals = bytecode
            .globals
            .iter()
            .filter_map(|symbol| bytecode.symbols.resolve(*symbol))
            .filter(|name| !manifest.globals().contains_key(*name))
            .map(str::to_owned)
            .collect();
        let entry_symbol = entries
            .first()
            .map(|(_, name)| {
                bytecode.symbols.find(name).ok_or_else(|| {
                    UiCompileError(format!("UI entrypoint `{name}` was not interned"))
                })
            })
            .transpose()?;
        // Imported helpers may read structural state too. Until call-graph
        // dependency tracking exists, conservatively include their reads.
        let mut plan = compiler.plans.remove(path).unwrap_or_default();
        for dependency in compiler.plans.values() {
            plan.structural_globals
                .extend(dependency.structural_globals.iter().cloned());
        }
        Ok(Self {
            program: project.program,
            entry,
            entry_symbol,
            owned_globals,
            plan,
        })
    }

    pub fn initializer(&self) -> Result<Option<LinkedVm>, LinkedVmError> {
        self.entry_symbol
            .map(|_| LinkedVm::new(self.program.clone(), self.entry))
            .transpose()
    }

    pub fn invocation(&self, arguments: Vec<Value>) -> Result<LinkedVm, UiInvocationError> {
        let mut vm = if let Some(symbol) = self.entry_symbol {
            LinkedVm::from_callable(
                self.program.clone(),
                &Value::Function {
                    module: Some(self.entry.0),
                    symbol,
                },
                arguments,
            )
        } else if arguments.is_empty() {
            LinkedVm::new(self.program.clone(), self.entry)
        } else {
            return Err(UiInvocationError::MissingEntry);
        }
        .map_err(UiInvocationError::Vm)?;
        vm.freeze_invocation_inputs()
            .map_err(UiInvocationError::Vm)?;
        Ok(vm)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UiInvocationError {
    #[error("parameterized UI modules require an `@ui global fn` entrypoint")]
    MissingEntry,
    #[error("{0}")]
    Vm(LinkedVmError),
}
