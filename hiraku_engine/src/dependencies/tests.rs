use super::*;
use hiraku_hdp::dependencies::{ResourceGraph, ResourceNode};

fn node(start: usize, image: &str, next: &[usize]) -> ResourceNode {
    ResourceNode {
        span: [start, start + 10],
        images: BTreeSet::from([image.into()]),
        next: next.to_vec(),
        calls: BTreeSet::new(),
    }
}
fn manifest() -> DependencyManifest {
    DependencyManifest {
        version: DEPENDENCY_VERSION,
        image_bytes: BTreeMap::from([("alice.png".into(), 16), ("bob.png".into(), 16)]),
        exports: BTreeMap::new(),
        resident: BTreeSet::from(["ui.png".into()]),
        conservative: BTreeMap::new(),
        scripts: BTreeMap::new(),
        windows: BTreeMap::from([(
            "scene.hks".into(),
            ResourceGraph {
                entry: Some(0),
                functions: BTreeMap::new(),
                nodes: vec![node(0, "alice.png", &[1]), node(10, "bob.png", &[])],
            },
        )]),
    }
}

#[test]
fn window_moves_by_execution_and_releases_only_its_own_handles() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_asset::<Image>();
    let assets = app.world().resource::<AssetServer>();
    let mut state = ScriptDependencies {
        lookahead_steps: 0,
        retain_steps: 1,
        preload_budget_bytes: 16,
        ..default()
    };
    state.manifests.insert(String::new(), Some(manifest()));
    state.move_window(BTreeSet::from([("scene.hks".into(), 0)]), assets);
    let live = state.handles["alice.png"].clone();
    for _ in 0..100 {
        state.move_window(BTreeSet::from([("scene.hks".into(), 0)]), assets);
    }
    assert_eq!(state.revision, 1);
    assert_eq!(state.handles.len(), 1);
    state.move_window(BTreeSet::from([("scene.hks".into(), 1)]), assets);
    assert!(
        state.protects("alice.png"),
        "backward window retains recent artwork"
    );
    assert!(
        state.handles.contains_key("bob.png"),
        "near future wins the upload budget"
    );
    assert!(!state.handles.contains_key("alice.png"));
    assert!(live.is_strong(), "visible scene owner is never invalidated");
    assert!(
        !state.protects("ui.png"),
        "unopened UI is not globally pinned"
    );
    state.move_window(BTreeSet::new(), assets);
    assert!(!state.protects("alice.png"));
    state.finish();
    assert!(state.closed);
    assert_eq!(state.retained_images(), 0);
    assert!(state.desired.is_empty());
    assert!(
        live.is_strong(),
        "story completion does not invalidate visible artwork"
    );
}

#[test]
fn branch_loop_and_function_lookahead_are_bounded_and_include_both_sides() {
    let mut manifest = manifest();
    let graph = manifest.windows.get_mut("scene.hks").expect("graph");
    graph.nodes[0].next = vec![1, 2];
    graph.nodes.push(node(20, "other.png", &[0]));
    graph.nodes[1].calls.insert("helper".into());
    manifest
        .exports
        .insert("helper".into(), "helper.hks".into());
    manifest.windows.insert(
        "helper.hks".into(),
        ResourceGraph {
            entry: None,
            functions: BTreeMap::from([("helper".into(), 0)]),
            nodes: vec![node(0, "function.png", &[0])],
        },
    );
    let query = |depth, limit| window::collect(&manifest, [("scene.hks".into(), 0)], depth, limit);
    assert_eq!(query(0, 256), ["alice.png"]);
    assert_eq!(query(1, 256), ["alice.png", "bob.png", "other.png"]);
    assert_eq!(
        query(1000, 256),
        ["alice.png", "bob.png", "other.png", "function.png"]
    );
    assert_eq!(query(1000, 2).len(), 2);
}

#[test]
fn innermost_source_span_selects_the_branch_not_its_parent() {
    let mut graph = ResourceGraph::default();
    graph.nodes = vec![node(0, "parent.png", &[]), node(2, "child.png", &[])];
    graph.nodes[0].span = [0, 50];
    assert_eq!(window::locate(&graph, 3), Some(1));
    assert_eq!(window::locate(&graph, 49), Some(0));
    assert_eq!(window::locate(&graph, 50), None);
}

#[test]
fn navigation_preloads_only_entry_and_loose_content_remains_lazy() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_asset::<Image>();
    let mut state = ScriptDependencies {
        lookahead_steps: 0,
        ..default()
    };
    state
        .manifests
        .insert("hdp://test.hdp/".into(), Some(manifest()));
    state.manifests.insert(String::new(), None);
    let vfs = HdpVfs::new("unused-fixture-root");
    let assets = app.world().resource::<AssetServer>();
    state
        .prepare(&vfs, assets, &["hdp://test.hdp/scene.hks".into()])
        .expect("entry window");
    assert_eq!(state.handles.len(), 1);
    assert!(state.protects("hdp://test.hdp/alice.png"));
    assert!(!state.protects("hdp://test.hdp/bob.png"));
    assert!(
        state
            .prepare(&vfs, assets, &["hdp://test.hdp/missing.hks".into()])
            .expect_err("stale manifest")
            .contains("rebuild")
    );
    let mut loose = ScriptDependencies::default();
    loose.manifests.insert(String::new(), None);
    assert!(
        loose
            .prepare(&vfs, assets, &["scene.hks".into()])
            .expect("no packaging required")
    );
}

#[test]
fn real_vm_waits_and_restores_report_the_current_source_statement() {
    use crate::script::{StoryRuntime, StoryRuntimeEvent, compile_story_bytecode};
    let source = "\"first\"\n\"second\"";
    let code = compile_story_bytecode("scene.hks", source).expect("compile fixture");
    let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
    fn wait(runtime: &mut StoryRuntime) {
        for _ in 0..30 {
            if matches!(
                runtime.step().expect("step"),
                Some(StoryRuntimeEvent::Wait(_))
            ) {
                return;
            }
        }
        panic!("dialogue did not wait");
    }
    wait(&mut runtime);
    assert!(
        runtime
            .resource_positions()
            .contains(&("scene.hks".into(), 0)),
        "positions: {:?}",
        runtime.resource_positions()
    );
    let snapshot = runtime.snapshot().expect("wait boundary");
    let mut restored = StoryRuntime::restore(code, snapshot).expect("restore");
    assert_eq!(runtime.resource_positions(), restored.resource_positions());
    restored
        .resume(hiraku_script::Value::Unit)
        .expect("next dialogue");
    wait(&mut restored);
    assert!(
        restored
            .resource_positions()
            .contains(&("scene.hks".into(), 8))
    );
}
