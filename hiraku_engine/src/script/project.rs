//! Story entry points share a statically checked set of script modules.
use super::{capabilities::story_manifest, execution_runtime::ExecutionRuntimeError};
use hiraku_script::{Bytecode, LinkedProgram, ModuleId, ScriptSource};

#[derive(Clone)]
pub(crate) enum StoryProgram {
    Single(Bytecode),
    Project {
        program: LinkedProgram,
        entry: ModuleId,
    },
}

impl From<Bytecode> for StoryProgram {
    fn from(code: Bytecode) -> Self {
        Self::Single(code)
    }
}

impl StoryProgram {
    pub(super) fn link(self) -> Result<(LinkedProgram, ModuleId), ExecutionRuntimeError> {
        match self {
            Self::Single(code) => Ok((
                hiraku_script::link_register_modules(vec![code], &story_manifest())
                    .map_err(ExecutionRuntimeError::Link)?,
                ModuleId(0),
            )),
            Self::Project { program, entry } => Ok((program, entry)),
        }
    }
}

pub(crate) fn compile_story_program(
    vfs: &crate::vfs::HdpVfs,
    path: &str,
    source: &str,
) -> Result<StoryProgram, String> {
    let root = path
        .strip_prefix("hdp://")
        .and_then(|path| path.split_once('/'))
        .map(|(archive, _)| format!("hdp://{archive}/"))
        .unwrap_or_default();
    let mut sources = Vec::new();
    for file in vfs
        .list_files_recursive(&root)
        .map_err(|error| error.to_string())?
    {
        // UI files have their own capability manifest and module domain.
        if !file.ends_with(".hks")
            || file.ends_with(".ui.hks")
            || file.contains("/ui/")
            || file.starts_with("ui/")
        {
            continue;
        }
        let text = if file == path {
            source.to_owned()
        } else {
            vfs.read_text(&file).map_err(|error| error.to_string())?
        };
        sources.push(ScriptSource {
            path: file,
            source: text,
            namespace: None,
        });
    }
    if !sources.iter().any(|source| source.path == path) {
        sources.push(ScriptSource {
            path: path.into(),
            source: source.into(),
            namespace: None,
        });
    }
    compile_sources(sources, path)
}

fn compile_sources(sources: Vec<ScriptSource>, entry: &str) -> Result<StoryProgram, String> {
    let mut source_map = hiraku_script::SourceMap::new();
    let ids = sources
        .iter()
        .map(|source| {
            (
                source.path.clone(),
                source_map.insert(&source.path, &source.source),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let project = hiraku_script::compile_project(sources, &story_manifest()).map_err(|errors| {
        let diagnostics = errors
            .into_iter()
            .map(|error| error.error.diagnostic(ids[&error.path].clone()))
            .collect::<Vec<_>>();
        hiraku_script::render_diagnostics(
            &diagnostics,
            &source_map,
            hiraku_script::RenderOptions::terminal(),
        )
    })?;
    let entry = *project
        .paths
        .get(entry)
        .ok_or_else(|| format!("story entry `{entry}` is missing"))?;
    Ok(StoryProgram::Project {
        program: project.program,
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::execution_runtime::{ExecutionEvent, ExecutionId, ExecutionRuntime};
    use hiraku_script::Value;

    fn source(path: &str, text: &str) -> ScriptSource {
        ScriptSource {
            path: path.into(),
            source: text.into(),
            namespace: None,
        }
    }

    #[test]
    fn exported_function_links_without_executing_provider_and_restores_its_call_frame() {
        let compile = || {
            compile_sources(
                vec![
                    source(
                        "chapter.hks",
                        "global var score = 1\nrecord(\"alice\")\nlog(\"bob\")",
                    ),
                    source(
                        "common.hks",
                        "global fn record(id: String) { log(id) }\nunreachable()",
                    ),
                ],
                "chapter.hks",
            )
            .expect("project compiles")
        };
        let mut runtime = ExecutionRuntime::new(compile()).expect("entry links");
        let mut calls = Vec::new();
        for _ in 0..100 {
            match runtime.step().expect("project advances") {
                Some(ExecutionEvent::Call { execution, call }) => {
                    calls.push(call.arguments[0].value.clone());
                    let bytes = hiraku_script::hson::to_vec(&runtime.snapshot()).expect("snapshot");
                    runtime = ExecutionRuntime::restore(
                        compile(),
                        hiraku_script::hson::from_slice(&bytes).expect("decode state"),
                    )
                    .expect("restore external frame");
                    runtime
                        .resume(execution, Value::Unit)
                        .expect("resume native call");
                }
                Some(ExecutionEvent::Completed {
                    execution: ExecutionId::MAIN,
                    ..
                }) => break,
                _ => {}
            }
        }
        assert_eq!(
            calls,
            [Value::String("alice".into()), Value::String("bob".into())]
        );
        assert!(runtime.globals().contains_key("score"));
    }

    #[test]
    fn shared_function_signatures_are_checked_before_entry_execution() {
        let result = compile_sources(
            vec![
                source("entry.hks", "record(false)"),
                source("common.hks", "global fn record(id: String) { log(id) }"),
            ],
            "entry.hks",
        );
        assert!(result.err().expect("type mismatch").contains("String"));
    }

    #[test]
    fn provider_closure_uses_its_own_module_and_shared_globals() {
        use crate::script::execution_runtime::ExecutionMode;
        let code = compile_sources(vec![
            source("entry.hks", "global var score = 1\nstart()"),
            source("common.hks", "global var score: Int\nglobal fn start() { par { score += 1\nlog(\"alice\") } }"),
        ], "entry.hks").expect("project compiles");
        let mut runtime = ExecutionRuntime::new(code.clone()).expect("runtime");
        let call = loop {
            if let Some(ExecutionEvent::Call { call, .. }) = runtime.step().expect("start") {
                break call;
            }
        };
        let child = runtime
            .spawn(&call.arguments[0].value, ExecutionMode::Parallel)
            .expect("provider closure");
        runtime
            .resume(ExecutionId::MAIN, Value::Unit)
            .expect("resume builder");
        runtime =
            ExecutionRuntime::restore(code, runtime.snapshot()).expect("restore both executions");
        let mut logged = false;
        for _ in 0..100 {
            match runtime.step().expect("run closure") {
                Some(ExecutionEvent::Call { execution, call }) => {
                    assert_eq!(execution, child);
                    assert_eq!(call.arguments[0].value, Value::String("alice".into()));
                    runtime.resume(execution, Value::Unit).expect("resume log");
                    logged = true;
                }
                Some(ExecutionEvent::Completed { execution, .. }) if execution == child => break,
                _ => {}
            }
        }
        assert!(logged);
        assert_eq!(runtime.globals()["score"], Value::Number(2.0));
    }
}
