use super::*;

pub(crate) fn capture_checkpoint(
    runtime: Res<ScriptRuntimeState>,
    scene: Res<SceneSharedState>,
    dialogue: Res<DialogueState>,
    mut history: ResMut<DialogueHistoryState>,
) {
    let Some(request) = dialogue.waiting.as_ref().and_then(|wait| wait.request) else { return; };
    if runtime.wait_request != Some(request) || history.captured_request == Some(request) || history.records.is_empty() {
        return;
    }
    history.captured_request = Some(request);
    match crate::script::capture_runtime_save(&runtime, &scene) {
        Ok(checkpoint) => {
            if let Some(record) = history.records.last_mut() {
                record.checkpoint = Some(std::sync::Arc::new(checkpoint));
            }
        }
        Err(error) => warn!("could not capture dialogue history checkpoint: {error}"),
    }
}
