//! Embedded libraries are separate compilation units, never privileged user source.
use hiraku_script::{Bytecode, ScriptSource};

pub(super) const DIALOGUE_PATH: &str = "embedded://hiraku_engine/std/dialogue.hks";
pub(super) const DIALOGUE_CAPABILITY: &str = "dialogue.write";
pub(super) const ACTOR_CAPABILITY: &str = "scene.actor";
const DIALOGUE_SOURCE: &str = include_str!("std/dialogue.hks");

pub(super) fn dialogue_source() -> ScriptSource {
    ScriptSource {
        path: DIALOGUE_PATH.into(),
        source: DIALOGUE_SOURCE.into(),
        namespace: None,
    }
}

pub(super) fn dialogue_bytecode(symbols: &hiraku_script::SymbolManifest) -> Bytecode {
    static CODE: std::sync::OnceLock<Bytecode> = std::sync::OnceLock::new();
    let code = CODE.get_or_init(|| {
        let project = super::project::compile_library_project(
            Vec::new(),
            hiraku_script::RenderOptions::plain(),
        )
        .expect("embedded dialogue library must compile and link");
        (*project.program.modules[project.paths[DIALOGUE_PATH].0 as usize].bytecode).clone()
    });
    let program =
        hiraku_script::parse_program(DIALOGUE_SOURCE).expect("embedded actor library parses");
    let mut aligned = hiraku_script::vm::compile_with_project_interface(
        &program,
        code.source_hash,
        &super::capabilities::story_manifest(),
        &hiraku_script::project::ProjectInterface::with_symbols(symbols.clone()),
    )
    .expect("embedded library compiles with caller symbols");
    aligned.debug.source = code.debug.source.clone();
    aligned
}
