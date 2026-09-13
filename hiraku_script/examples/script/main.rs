//! A standalone host: edit the adjacent main.hks and run this example again.
use std::{io::Write, path::Path, process::ExitCode};

use hiraku_script::native::{NativeError, NativeRegistry};
use hiraku_script::{
    LinkedVm, LinkedVmEvent, RenderOptions, ScriptSource, SourceMap, compile_project,
    render_diagnostics,
};

fn print(output: &mut std::io::Stdout, message: String) -> Result<(), NativeError> {
    writeln!(output.lock(), "{message}")
        .map_err(|error| NativeError::message(format!("failed to write stdout: {error}")))
}

fn run() -> Result<(), String> {
    // Resolve relative to the crate, not the shell's current working directory.
    // Read at runtime so editing main.hks does not require rebuilding the host.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/script/main.hks");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let source_path = path.to_string_lossy().into_owned();
    let mut natives = NativeRegistry::new();
    natives
        .register_fn("print", print)
        .map_err(|error| error.to_string())?;
    let mut sources = SourceMap::new();
    let source_id = sources.insert(source_path.clone(), source.clone());
    let project = compile_project(
        vec![ScriptSource {
            path: source_path.clone(),
            namespace: None,
            source,
        }],
        &natives.manifest(),
    )
    .map_err(|errors| {
        let diagnostics = errors
            .iter()
            .map(|error| error.error.diagnostic(source_id.clone()))
            .collect::<Vec<_>>();
        render_diagnostics(&diagnostics, &sources, RenderOptions::terminal())
    })?;
    let entry = *project
        .paths
        .get(&source_path)
        .ok_or("compiled entry module is missing")?;
    let mut vm = LinkedVm::new(project.program, entry).map_err(|error| error.to_string())?;
    let mut output = std::io::stdout();
    loop {
        match vm.step().map_err(|error| error.to_string())? {
            Some(LinkedVmEvent::Call(call)) => {
                let result = natives
                    .call(&mut output, &call)
                    .map_err(|error| error.to_string())?;
                vm.resume(result).map_err(|error| error.to_string())?;
            }
            // This generic host has no story/UI statement handlers.
            Some(LinkedVmEvent::Statement(_) | LinkedVmEvent::BudgetExhausted) => {}
            Some(LinkedVmEvent::Completed(_)) | None => return Ok(()),
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Pretty diagnostics go directly to stderr, never through a logger.
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_print_arguments_fail_during_compilation() {
        let mut natives = NativeRegistry::new();
        natives.register_fn("print", print).expect("register print");
        for source in ["print(1)", "print()", "let output = print; output(1)"] {
            let errors = compile_project(
                vec![ScriptSource {
                    path: "test.hks".into(),
                    namespace: None,
                    source: source.into(),
                }],
                &natives.manifest(),
            )
            .expect_err("invalid native calls must not reach execution");
            assert!(
                errors
                    .iter()
                    .any(|error| error.error.message.contains("expects")
                        || error.error.message.contains("expected")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn registered_print_accepts_interpolated_strings() {
        let mut natives = NativeRegistry::new();
        natives.register_fn("print", print).expect("register print");
        compile_project(
            vec![ScriptSource {
                path: "main.hks".into(),
                namespace: None,
                source: r#"let index = 2; print("Iteration ${index}")"#.into(),
            }],
            &natives.manifest(),
        )
        .expect("the native String parameter must accept interpolation");
    }
}
