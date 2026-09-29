//! Host lifecycle requests use the same session transition as story navigation.
//!
//! Restart reads saved project sources; it never owns or modifies editor drafts.
//! Compilation happens off the main thread. A failed compile leaves the running
//! story, globals, scene, input waits and pending commands untouched.
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};

use crate::{
    scene::{PendingScriptCommands, drive_story_runtime, process_script_commands},
    script::navigation::{NavigationKind, NavigationRequest, NavigationReset},
    script::{RuntimeCommand, ScriptCommand, ScriptRuntimeState, StoryProgram},
    vfs::{HdpVfs, VfsResource},
};

#[derive(Clone, Copy, Debug, Message)]
pub enum RuntimeControl {
    /// Re-read the startup path from settings and compile the project on disk.
    /// Does not save source files, settings, or the currently running game.
    RestartSavedProject,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestartState {
    #[default]
    Idle,
    Compiling,
    Applying,
    Restarted,
    /// The diagnostic has been printed through the normal script diagnostic
    /// boundary. The previous runtime remains active after a compile failure.
    Failed,
}

#[derive(Resource, Default)]
pub struct RuntimeControlStatus {
    /// Lifecycle requests can be accepted after the engine frontend is ready.
    pub ready: bool,
    pub restart: RestartState,
    pub current_script: Option<String>,
}

type PreparedRestart = Result<(String, StoryProgram), String>;

#[derive(Resource, Default)]
struct RestartTask(Option<Task<PreparedRestart>>);

pub(crate) fn install(app: &mut App) {
    app.add_message::<RuntimeControl>()
        .init_resource::<RuntimeControlStatus>()
        .init_resource::<RestartTask>()
        .add_systems(
            Update,
            process_control
                .after(drive_story_runtime)
                .before(process_script_commands)
                .run_if(crate::runtime_initialized),
        )
        .add_systems(PostUpdate, report_current_script);
}

fn compile_restart(vfs: &HdpVfs) -> PreparedRestart {
    let path = vfs
        .load_startup_script_path()
        .map_err(|error| error.to_string())?;
    let source = vfs.read_text(&path).map_err(|error| error.to_string())?;
    let program = crate::script::compile_story_program(vfs, &path, &source)?;
    Ok((path, program))
}

fn process_control(
    mut requests: MessageReader<RuntimeControl>,
    vfs: Res<VfsResource>,
    mut task: ResMut<RestartTask>,
    mut status: ResMut<RuntimeControlStatus>,
    mut queue: ResMut<PendingScriptCommands>,
    mut redraw: crate::redraw::Redraw,
) {
    // Multiple clicks during the same compile coalesce. Never race two source
    // snapshots against each other or queue a second restart accidentally.
    if requests.read().count() > 0 && task.0.is_none() {
        let vfs = vfs.0.clone();
        task.0 = Some(AsyncComputeTaskPool::get().spawn(async move { compile_restart(&vfs) }));
        status.restart = RestartState::Compiling;
    }
    let Some(running) = task.0.as_mut() else {
        return;
    };
    redraw.request();
    let Some(result) = check_ready(running) else {
        return;
    };
    task.0 = None;
    apply_compilation(result, &mut queue, &mut status);
}

fn apply_compilation(
    result: PreparedRestart,
    queue: &mut PendingScriptCommands,
    status: &mut RuntimeControlStatus,
) {
    match result {
        Ok((path, program)) => {
            queue.clear();
            queue.enqueue(ScriptCommand::Runtime(RuntimeCommand::Navigate {
                request: NavigationRequest {
                    path,
                    kind: NavigationKind::Goto,
                    reset: NavigationReset::Session,
                    // A development restart uses normal on-demand asset
                    // loading; immutable archive manifests can be out of date.
                    preload: Some(false),
                    // Already root-relative, never relative to the story that
                    // happened to be playing when Restart was clicked.
                    origin: Some(String::new()),
                },
                program: Some(Box::new(program)),
            }));
            status.restart = RestartState::Applying;
        }
        Err(error) => {
            status.restart = RestartState::Failed;
            crate::script::emit_script_diagnostic("failed to restart saved project:", &error);
        }
    }
}

fn report_current_script(
    runtime: Res<ScriptRuntimeState>,
    frontend: Option<Res<crate::scene::FrontendState>>,
    mut status: ResMut<RuntimeControlStatus>,
) {
    if status.ready != frontend.is_some() {
        status.ready = frontend.is_some();
    }
    if status.current_script != runtime.current_script {
        status.current_script.clone_from(&runtime.current_script);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_restart_preserves_pending_game_commands() {
        let mut queue = PendingScriptCommands::default();
        queue.enqueue(ScriptCommand::Runtime(RuntimeCommand::Log("keep".into())));
        let before = format!("{queue:?}");
        let mut status = RuntimeControlStatus::default();
        apply_compilation(
            Err("synthetic compile error".into()),
            &mut queue,
            &mut status,
        );
        assert_eq!(status.restart, RestartState::Failed);
        assert_eq!(format!("{queue:?}"), before);
    }

    #[test]
    fn restart_target_is_root_relative_not_relative_to_the_current_scene() {
        let vfs = HdpVfs::new_with_config("unused", "settings.hson", "startup.hks");
        assert_eq!(vfs.resolve_path(Some(""), "startup.hks"), "startup.hks");
        assert_eq!(
            vfs.resolve_path(Some(""), "scripts/entry.hks"),
            "scripts/entry.hks"
        );
    }

    #[test]
    fn reported_path_only_marks_the_resource_changed_when_it_changes() {
        let mut app = App::new();
        app.init_resource::<ScriptRuntimeState>()
            .init_resource::<RuntimeControlStatus>()
            .add_systems(Update, report_current_script);
        app.world_mut()
            .resource_mut::<ScriptRuntimeState>()
            .current_script = Some("alice.hks".into());
        app.update();
        assert_eq!(
            app.world()
                .resource::<RuntimeControlStatus>()
                .current_script
                .as_deref(),
            Some("alice.hks")
        );
        app.update();
        assert!(!app.world().is_resource_changed::<RuntimeControlStatus>());
    }

    #[test]
    fn restart_compiles_updated_saved_sources_instead_of_reusing_a_linked_program() {
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root =
            Fixture(std::env::temp_dir().join(format!("hiraku-restart-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir(&root.0).expect("create isolated fixture");
        let source = root.0.join("startup.hks");
        let vfs = HdpVfs::new_with_config(&root.0, "settings.hson", "startup.hks");
        for message in ["alice", "bob"] {
            std::fs::write(&source, format!("log(\"{message}\")")).expect("write synthetic source");
            let (path, program) = compile_restart(&vfs).expect("compile saved source");
            assert_eq!(path, "startup.hks");
            let mut story = crate::script::StoryRuntime::new(program).expect("link source");
            assert!(matches!(story.step().expect("execute source"),
                Some(crate::script::StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::Log(value)))
                    if value == message));
        }
        std::fs::write(&source, "let broken =").expect("write incomplete source");
        assert!(
            compile_restart(&vfs).is_err(),
            "must not silently reuse the last valid program"
        );
    }
}
