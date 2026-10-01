use super::*;

pub(crate) fn capture_checkpoint(
    runtime: Res<ScriptRuntimeState>,
    scene: Res<SceneSharedState>,
    dialogue: Res<DialogueState>,
    mut history: ResMut<DialogueHistoryState>,
    mut redraw: crate::redraw::Redraw,
) {
    let Some(request) = dialogue.waiting.as_ref().and_then(|wait| wait.request) else {
        return;
    };
    if runtime.story.is_none()
        || runtime.wait_request != Some(request)
        || history.captured_request == Some(request)
        || history.records.is_empty()
    {
        return;
    }
    history.captured_request = Some(request);
    match crate::script::capture_runtime_save(&runtime, &scene) {
        Ok(checkpoint) => {
            if let Some(record) = history.records.last_mut() {
                record.checkpoint = Some(std::sync::Arc::new(checkpoint));
                redraw.request();
            }
        }
        Err(error) => warn!("could not capture dialogue history checkpoint: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_dialogue_captures_once_without_invalidating_ui_each_frame() {
        let code = crate::script::compile_story_bytecode("alice.hks", "\"Ready\"\n\"Next\"")
            .expect("compile story");
        let mut story = crate::script::StoryRuntime::new(code).expect("story");
        while let Some(event) = story.step().expect("step") {
            if matches!(event, crate::script::StoryRuntimeEvent::Wait(_)) {
                break;
            }
        }
        let request = ScriptRequestId(1);
        let mut runtime = ScriptRuntimeState::default();
        runtime.story = Some(story);
        runtime.current_script = Some("alice.hks".into());
        runtime.wait_request = Some(request);
        let mut history = DialogueHistoryState::default();
        history.push(DialogueSnapshot {
            speaker: "alice".into(),
            text: "Ready".into(),
        });
        let mut app = App::new();
        app.insert_resource(runtime)
            .insert_resource(history)
            .init_resource::<SceneSharedState>()
            .insert_resource(DialogueState {
                waiting: Some(PendingDialogueAdvance {
                    animation_id: None,
                    request: Some(request),
                }),
                ..default()
            })
            .add_systems(Update, capture_checkpoint);
        app.update();
        let checkpoint = app.world().resource::<DialogueHistoryState>().records[0]
            .checkpoint
            .clone()
            .expect("checkpoint");
        assert!(checkpoint.vm_snapshot.is_some());
        assert!(checkpoint.history_records.is_empty());
        for _ in 0..3 {
            app.update();
        }
        let retained = app.world().resource::<DialogueHistoryState>().records[0]
            .checkpoint
            .as_ref()
            .expect("retained checkpoint");
        assert!(std::sync::Arc::ptr_eq(&checkpoint, retained));
    }

    #[test]
    fn checkpoint_resumes_after_the_retained_line_with_its_original_variables() {
        let code = crate::script::compile_story_bytecode(
            "alice.hks",
            "global var score: Int = 1\n\"Ready\"\nscore += 1\n\"Next\"",
        )
        .expect("story");
        let mut story = crate::script::StoryRuntime::new(code.clone()).expect("runtime");
        while let Some(event) = story.step().expect("step") {
            if matches!(event, crate::script::StoryRuntimeEvent::Wait(_)) {
                break;
            }
        }
        let snapshot = story.snapshot().expect("dialogue boundary");
        let mut restored = crate::script::StoryRuntime::restore(code, snapshot).expect("restore");
        assert_eq!(
            restored.globals().get("score"),
            Some(&hiraku_script::Value::Int(1))
        );
        restored
            .resume(hiraku_script::Value::Unit)
            .expect("advance retained line");
        let mut next = None;
        while let Some(event) = restored.step().expect("next line") {
            if let crate::script::StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::Say { text, .. },
            ) = event
            {
                next = Some(text);
            } else if matches!(event, crate::script::StoryRuntimeEvent::Wait(_)) {
                break;
            }
        }
        assert_eq!(next.as_deref(), Some("Next"));
        assert_eq!(
            restored.globals().get("score"),
            Some(&hiraku_script::Value::Int(2))
        );
    }
}
