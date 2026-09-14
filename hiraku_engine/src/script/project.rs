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
            Self::Single(code) => {
                let mut policy = hiraku_script::LinkPolicy::default();
                policy.grant(ModuleId(1), super::stdlib::DIALOGUE_CAPABILITY);
                policy.grant(ModuleId(1), super::stdlib::ACTOR_CAPABILITY);
                let library = super::stdlib::dialogue_bytecode(&code.symbols);
                Ok((
                    hiraku_script::link_named_modules_with_policy(
                        vec![(None, code), (None, library)],
                        &story_manifest(),
                        &policy,
                    )
                    .map_err(ExecutionRuntimeError::Link)?,
                    ModuleId(0),
                ))
            }
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
    let project = compile_library_project(sources, hiraku_script::RenderOptions::terminal())?;
    let entry = *project
        .paths
        .get(entry)
        .ok_or_else(|| format!("story entry `{entry}` is missing"))?;
    Ok(StoryProgram::Project {
        program: project.program,
        entry,
    })
}

pub(super) fn compile_library_project(
    mut sources: Vec<ScriptSource>,
    options: hiraku_script::RenderOptions,
) -> Result<hiraku_script::CompiledProject, String> {
    if sources
        .iter()
        .any(|source| source.path == super::stdlib::DIALOGUE_PATH)
    {
        return Err("user source cannot replace the embedded dialogue library".into());
    }
    sources.push(super::stdlib::dialogue_source());
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
    let mut policy = hiraku_script::ProjectLinkPolicy::default();
    policy.grant(
        super::stdlib::DIALOGUE_PATH,
        super::stdlib::DIALOGUE_CAPABILITY,
    );
    policy.grant(
        super::stdlib::DIALOGUE_PATH,
        super::stdlib::ACTOR_CAPABILITY,
    );
    let project = hiraku_script::compile_project_with_policy(sources, &story_manifest(), &policy)
        .map_err(|errors| {
        let diagnostics = errors
            .into_iter()
            .map(|error| error.error.diagnostic(ids[&error.path].clone()))
            .collect::<Vec<_>>();
        hiraku_script::render_diagnostics(&diagnostics, &source_map, options)
    })?;
    Ok(project)
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
    fn script_colon_wrappers_preserve_speaker_and_template_scope() {
        for receiver in ["char(\"alice\")", "\"alice\""] {
            let code = compile_sources(
                vec![source(
                    "entry.hks",
                    &format!("let name = \"bob\"\n{receiver}: \"Hello ${{name}}\""),
                )],
                "entry.hks",
            )
            .expect("colon compiles");
            let mut runtime =
                crate::script::story_runtime::StoryRuntime::new(code).expect("runtime");
            let mut said = false;
            for _ in 0..30 {
                if let Some(crate::script::story_runtime::StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::Say { speaker, text },
                )) = runtime.step().expect("colon preserves lexical scope")
                {
                    assert_eq!((speaker.as_str(), text.as_str()), ("alice", "Hello bob"));
                    said = true;
                    break;
                }
            }
            assert!(said);
        }
    }

    #[test]
    fn user_story_cannot_call_dialogue_primitives_directly() {
        let error = compile_sources(
            vec![source(
                "entry.hks",
                "intrinsics.engine.say(\"alice\", \"Hello\")",
            )],
            "entry.hks",
        )
        .err()
        .expect("user has no dialogue capability");
        assert!(error.contains("dialogue.write"), "{error}");
    }

    #[test]
    fn actor_primitives_require_library_capability_and_public_api_is_script_owned() {
        let manifest = super::super::capabilities::story_manifest();
        assert!(manifest.resolve("char").is_none());
        assert!(manifest.resolve_selector("Actor", "e").is_none());
        assert!(manifest.resolve_selector("Actor", "at").is_none());
        let error = compile_sources(
            vec![source(
                "entry.hks",
                "intrinsics.engine.actorIdentity(\"alice\")",
            )],
            "entry.hks",
        )
        .err()
        .expect("user has no actor capability");
        assert!(error.contains("scene.actor"), "{error}");
        compile_sources(
            vec![source(
                "entry.hks",
                r#"
            let alice = char("alice")
            alice.at(.left).scale(0.5).e("happy").focus().clip().show()
            alice.hide()
            alice.alias("middle").show()
            alice.clone("copy").show()
        "#,
            )],
            "entry.hks",
        )
        .expect("public script Actor methods and omitted optional arguments compile");
    }

    #[test]
    fn standalone_library_frame_restores_and_ellipsis_appends() {
        use crate::script::{
            capabilities::{StoryEffect, StoryWait},
            story_runtime::{StoryRuntime, StoryRuntimeEvent},
        };
        let code = super::super::capabilities::compile_story_bytecode(
            "entry.hks",
            r#"
            let name = "bob"
            char("alice"): "Hello"
            ...: " ${name}"
        "#,
        )
        .expect("standalone story compiles with library");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let mut restored = false;
        let mut appended = false;
        for _ in 0..60 {
            match runtime.step().expect("dialogue wrapper executes") {
                Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance)) if !restored => {
                    let bytes = hiraku_script::hson::to_vec(
                        &runtime.snapshot().expect("save library call frame"),
                    )
                    .expect("serialize snapshot");
                    runtime = StoryRuntime::restore(
                        code.clone(),
                        hiraku_script::hson::from_slice(&bytes).expect("decode"),
                    )
                    .expect("restore linked library frame");
                    runtime
                        .resume(Value::Unit)
                        .expect("advance restored dialogue");
                    restored = true;
                }
                Some(StoryRuntimeEvent::Effect(StoryEffect::ContinueDialogue { text })) => {
                    assert_eq!(text, " bob");
                    appended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(restored && appended);
    }

    #[test]
    fn stage_handles_cross_module_boundaries_through_typed_parameters() {
        compile_sources(vec![
            source("entry.hks", "let room = stage.open(\"room.stage.hson\")\nshot(room)"),
            source("shot.hks", "global fn shot(room: Stage) { room.camera(\"alice\").await()\nroom.clipView(\"main\", .left(25, 10)) }"),
        ], "entry.hks").expect("native handles retain selector types across script modules");
    }

    #[test]
    fn provider_resolves_existing_actor_handles_in_its_local_scope() {
        let code = compile_sources(vec![
            source("entry.hks", "global let alice = char(\"alice\")\ntell()"),
            source("provider.hks", "global fn tell() { let alice = char(\"alice\")\nalice.e(\"happy\")\nalice: \"Hello\" }"),
        ], "entry.hks").expect("typed local actor binding");
        let mut runtime = crate::script::story_runtime::StoryRuntime::new(code).expect("runtime");
        let mut said = false;
        for _ in 0..20 {
            if let Some(crate::script::story_runtime::StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::Say { speaker, text },
            )) = runtime.step().expect("native actor argument")
            {
                assert_eq!((speaker.as_str(), text.as_str()), ("alice", "Hello"));
                said = true;
                break;
            }
        }
        assert!(said);
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
