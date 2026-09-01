use super::*;
use crate::script::StoryRuntimeEvent;

fn stored_to_hks(value: StoredValue) -> hiraku_script::Value {
    match value {
        StoredValue::Bool(value) => hiraku_script::Value::Bool(value),
        StoredValue::Int(value) => hiraku_script::Value::Number(value as f64),
        StoredValue::Float(value) => hiraku_script::Value::Number(value),
        StoredValue::String(value) => hiraku_script::Value::String(value),
        StoredValue::Array(values) => {
            hiraku_script::Value::List(values.into_iter().map(stored_to_hks).collect())
        }
        StoredValue::Map(values) => hiraku_script::Value::Map(
            values
                .into_iter()
                .map(|(name, value)| (name, stored_to_hks(value)))
                .collect(),
        ),
    }
}

fn hks_globals_to_stored(
    globals: &BTreeMap<String, hiraku_script::Value>,
) -> BTreeMap<String, StoredValue> {
    globals
        .iter()
        .filter_map(|(name, value)| hks_to_stored(value).map(|value| (name.clone(), value)))
        .collect()
}

fn hks_to_stored(value: &hiraku_script::Value) -> Option<StoredValue> {
    match value {
        hiraku_script::Value::Bool(value) => Some(StoredValue::Bool(*value)),
        hiraku_script::Value::Number(value) => Some(StoredValue::Float(*value)),
        hiraku_script::Value::String(value) | hiraku_script::Value::Symbol(value) => {
            Some(StoredValue::String(value.clone()))
        }
        hiraku_script::Value::List(values) | hiraku_script::Value::Tuple(values) => Some(
            StoredValue::Array(values.iter().filter_map(hks_to_stored).collect()),
        ),
        hiraku_script::Value::Map(values) => Some(StoredValue::Map(
            values
                .iter()
                .filter_map(|(name, value)| hks_to_stored(value).map(|value| (name.clone(), value)))
                .collect(),
        )),
        hiraku_script::Value::Typed { value, .. } => hks_to_stored(value),
        _ => None,
    }
}

pub(crate) fn evaluate_ui_at(
    target: &str,
    runtime: &ScriptRuntimeState,
    vfs: &VfsResource,
    user_settings: &UserSettings,
    textures: Option<&TextureCatalog>,
    terms: Option<&TermCatalog>,
) -> Result<ScreenSpec, String> {
    evaluate_ui_at_with(
        target,
        runtime,
        vfs,
        user_settings,
        textures,
        terms,
        BTreeMap::new(),
    )
}

fn evaluate_ui_at_with(
    target: &str,
    runtime: &ScriptRuntimeState,
    vfs: &VfsResource,
    user_settings: &UserSettings,
    textures: Option<&TextureCatalog>,
    terms: Option<&TermCatalog>,
    extra_values: BTreeMap<String, StoredValue>,
) -> Result<ScreenSpec, String> {
    evaluate_ui_at_with_arguments(
        target,
        runtime,
        vfs,
        user_settings,
        textures,
        terms,
        extra_values,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_ui_at_with_arguments(
    target: &str,
    runtime: &ScriptRuntimeState,
    vfs: &VfsResource,
    user_settings: &UserSettings,
    textures: Option<&TextureCatalog>,
    terms: Option<&TermCatalog>,
    extra_values: BTreeMap<String, StoredValue>,
    arguments: &[StoredValue],
) -> Result<ScreenSpec, String> {
    let mut values = runtime
        .story
        .as_ref()
        .map(|story| hks_globals_to_stored(story.globals()))
        .unwrap_or_default();
    values.insert(
        "bgmVolume".to_string(),
        StoredValue::Float(user_settings.bgm_volume as f64),
    );
    values.insert(
        "voiceVolume".to_string(),
        StoredValue::Float(user_settings.voice_volume as f64),
    );
    values.insert(
        "sfxVolume".to_string(),
        StoredValue::Float(user_settings.sfx_volume as f64),
    );
    values.insert("dialogue".to_string(), default_dialogue_model());
    values.insert("history".to_string(), default_history_model());
    values.extend(extra_values);
    let source = vfs.0.read_text(target).map_err(|error| error.to_string())?;
    let textures = textures.ok_or_else(|| "texture catalog is unavailable".to_string())?;
    let terms = terms.ok_or_else(|| "term catalog is unavailable".to_string())?;
    evaluate_ui_component_named_with_args(
        target,
        &source,
        UiContext::new(values),
        textures,
        terms,
        arguments,
    )
    .map_err(|error| error.to_string())
}

fn default_dialogue_model() -> StoredValue {
    StoredValue::Map(BTreeMap::from([
        ("speaker".to_string(), StoredValue::String(String::new())),
        ("text".to_string(), StoredValue::String(String::new())),
        ("visible".to_string(), StoredValue::Bool(false)),
        ("revealedCharacters".to_string(), StoredValue::Int(0)),
        ("canAdvance".to_string(), StoredValue::Bool(false)),
    ]))
}

fn default_history_model() -> StoredValue {
    StoredValue::Map(BTreeMap::from([
        ("entries".to_string(), StoredValue::Array(Vec::new())),
        ("text".to_string(), StoredValue::String(String::new())),
        ("visible".to_string(), StoredValue::Bool(false)),
    ]))
}

/// UI components under the conventional `ui/` directory are package-rooted.
/// Other relative paths remain relative to the declaring script. Persisted
/// canonical paths pass through unchanged, which also repairs older saves that
/// retained `ui/...` and were restored from a script subdirectory.
pub(crate) fn resolve_ui_component_path(
    vfs: &VfsResource,
    current_script: Option<&str>,
    component: &str,
) -> String {
    if component.starts_with("ui/") {
        if let Some((archive, _)) = current_script
            .and_then(|path| path.strip_prefix("hdp://"))
            .and_then(|path| path.split_once('/'))
        {
            return vfs
                .0
                .resolve_path(None, &format!("hdp://{archive}/{component}"));
        }
        return vfs.0.resolve_path(None, component);
    }
    vfs.0.resolve_path(current_script, component)
}

pub fn drive_story_runtime(
    mut runtime: ResMut<ScriptRuntimeState>,
    mut response_messages: MessageReader<ScriptResponseMessage>,
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    textures: Option<Res<TextureCatalog>>,
    terms: Option<Res<TermCatalog>>,
    audio: Option<Res<AudioCatalog>>,
    vfs: Res<VfsResource>,
    user_settings: Res<UserSettings>,
) {
    for message in response_messages.read() {
        if let Some(task) = runtime.task_requests.remove(&message.request) {
            if let Some(story) = runtime.story.as_mut()
                && let Err(error) = story.resume_task(task)
            {
                warn!("failed to resume HKS task {task}: {error}");
                runtime.story = None;
            }
        } else {
            runtime.accept_response(message.clone());
        }
    }

    if let Some(request) = runtime.wait_request
        && let Some(response) = runtime.take_response(request)
    {
        let direct_value = match &response {
            ScriptResponse::Choice(value) => stored_to_hks(value.clone()),
            ScriptResponse::Continue => hiraku_script::Value::Unit,
        };
        runtime.pending_ui_screen = None;
        runtime.pending_ui_arguments.clear();
        runtime.wait_request = None;
        if let Some(story) = runtime.story.as_mut()
            && let Err(error) = story.resume(direct_value)
        {
            warn!("failed to resume script runtime: {error}");
            runtime.story = None;
        }
    }

    let event = match runtime.story.as_mut() {
        Some(story) => match story.step() {
            Ok(event) => event,
            Err(error) => {
                warn!("HKS runtime failed: {error}");
                runtime.story = None;
                None
            }
        },
        None => None,
    };

    if let Some(event) = event {
        match event {
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::PlayBgm {
                path,
                volume,
                fade_in_ms,
            }) => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_music(&path))
            {
                Some(definition) => {
                    pending_script_commands.enqueue(ScriptCommand::Audio(AudioCommand::PlayBgm {
                        path: definition.path.clone(),
                        prelude: definition.prelude.clone(),
                        volume,
                        fade_in: fade_in_ms.map(std::time::Duration::from_millis),
                        animation_id: None,
                    }));
                }
                None => warn!("music `{path}` is not defined"),
            },
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::PlayVoice {
                path,
                volume,
            }) => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_voice(&path))
            {
                Some(definition) => {
                    pending_script_commands.enqueue(ScriptCommand::Audio(
                        AudioCommand::PlayVoice {
                            path: definition.path.clone(),
                            volume,
                            mode: VoicePlaybackMode::Exclusive,
                            animation_id: None,
                        },
                    ));
                }
                None => warn!("voice `{path}` is not defined"),
            },
            StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::SetUiRole {
                role,
                component,
            }) => {
                let target =
                    resolve_ui_component_path(&vfs, runtime.current_script.as_deref(), &component);
                runtime.ui_registry.insert(role.clone(), target.clone());
                if role == "dialogue" {
                    match evaluate_ui_at(
                        &target,
                        &runtime,
                        &vfs,
                        &user_settings,
                        textures.as_deref(),
                        terms.as_deref(),
                    ) {
                        Ok(screen) => {
                            runtime
                                .mounted_ui_overlays
                                .insert("__role.dialogue".into(), target);
                            pending_script_commands.enqueue(ScriptCommand::Ui(
                                UiCommand::ShowOverlay {
                                    name: "__role.dialogue".into(),
                                    screen,
                                },
                            ));
                        }
                        Err(error) => {
                            warn!("failed to enable dialogue UI `{component}`: {error}");
                            runtime.story = None;
                        }
                    }
                }
            }
            StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::MountUiOverlay { name, component },
            ) => {
                let target = runtime
                    .ui_registry
                    .get(&component)
                    .cloned()
                    .unwrap_or_else(|| {
                        resolve_ui_component_path(
                            &vfs,
                            runtime.current_script.as_deref(),
                            &component,
                        )
                    });
                let overlay = evaluate_ui_at(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                );
                match overlay {
                    Ok(screen) => {
                        runtime
                            .mounted_ui_overlays
                            .insert(name.clone(), target.clone());
                        pending_script_commands
                            .enqueue(ScriptCommand::Ui(UiCommand::ShowOverlay { name, screen }));
                    }
                    Err(error) => {
                        warn!("failed to mount UI overlay `{name}` from `{target}`: {error}")
                    }
                }
            }
            StoryRuntimeEvent::Effect(
                crate::script::capabilities::StoryEffect::UnmountUiOverlay { name },
            ) => {
                runtime.mounted_ui_overlays.remove(&name);
                pending_script_commands.enqueue(ScriptCommand::Ui(UiCommand::HideOverlay { name }));
            }
            StoryRuntimeEvent::Effect(
                effect @ (crate::script::capabilities::StoryEffect::Say { .. }
                | crate::script::capabilities::StoryEffect::ContinueDialogue { .. }),
            ) => {
                if !runtime.ui_registry.contains_key("dialogue") {
                    warn!(
                        "dialogue UI is not configured; call ui.set(\"dialogue\", \"path/to/dialogue.ui.hks\") before executing dialogue"
                    );
                    runtime.story = None;
                } else {
                    match script_command_from_effect(effect, textures.as_deref()) {
                        Ok(command) => {
                            pending_script_commands.enqueue(command);
                        }
                        Err(error) => warn!("HKS dialogue command rejected: {error}"),
                    }
                }
            }
            StoryRuntimeEvent::Effect(effect) => {
                match script_command_from_effect(effect, textures.as_deref()) {
                    Ok(command) => {
                        pending_script_commands.enqueue(command);
                    }
                    Err(error) => warn!("HKS native command rejected: {error}"),
                }
            }
            StoryRuntimeEvent::Wait(crate::script::capabilities::StoryWait::DialogueAdvance) => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                pending_script_commands.enqueue(ScriptCommand::Dialogue(
                    DialogueCommand::AwaitAdvance { done: request },
                ));
            }
            StoryRuntimeEvent::Choice { prompt, options } => {
                let request = runtime.allocate_request();
                runtime.wait_request = Some(request);
                let Some(target) = runtime.ui_registry.get("choice").cloned() else {
                    warn!(
                        "choice UI is not configured; call ui.set(\"choice\", \"path/to/choice.ui.hks\") before executing choice"
                    );
                    runtime.story = None;
                    return;
                };
                let choice_model = StoredValue::Map(BTreeMap::from([
                    ("prompt".into(), StoredValue::String(prompt)),
                    (
                        "options".into(),
                        StoredValue::Array(options.into_iter().map(StoredValue::String).collect()),
                    ),
                ]));
                match evaluate_ui_at_with(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                    BTreeMap::from([("choice".into(), choice_model)]),
                ) {
                    Ok(screen) => {
                        pending_script_commands.enqueue(ScriptCommand::Ui(UiCommand::ShowScreen {
                            screen,
                            done: Some(request),
                        }));
                    }
                    Err(error) => {
                        warn!("failed to render choice UI `{target}`: {error}");
                        runtime.story = None;
                    }
                }
            }
            StoryRuntimeEvent::OpenUi { path, arguments } => {
                let target = runtime.ui_registry.get(&path).cloned().unwrap_or_else(|| {
                    vfs.0.resolve_path(runtime.current_script.as_deref(), &path)
                });
                let arguments = arguments
                    .iter()
                    .map(hks_to_stored)
                    .collect::<Option<Vec<_>>>();
                let Some(arguments) = arguments else {
                    warn!("ui.open arguments must contain only persistable values");
                    runtime.story = None;
                    return;
                };
                let screen = evaluate_ui_at_with_arguments(
                    &target,
                    &runtime,
                    &vfs,
                    &user_settings,
                    textures.as_deref(),
                    terms.as_deref(),
                    BTreeMap::new(),
                    &arguments,
                );
                let request = runtime.allocate_request();
                runtime.pending_ui_screen = Some(target.clone());
                runtime.pending_ui_arguments = arguments;
                runtime.wait_request = Some(request);
                match screen {
                    Ok(screen) => {
                        pending_script_commands.enqueue(ScriptCommand::Ui(UiCommand::ShowScreen {
                            screen,
                            done: Some(request),
                        }));
                    }
                    Err(error) => {
                        warn!("failed to render UI script `{target}`: {error}");
                        runtime.story = None;
                        runtime.wait_request = None;
                    }
                }
            }
            StoryRuntimeEvent::TaskEffect {
                task,
                effect: crate::script::capabilities::StoryEffect::PlayVoice { path, volume },
            } => match audio
                .as_deref()
                .and_then(|catalog| catalog.resolve_voice(&path))
            {
                Some(definition) => {
                    let request = runtime.allocate_request();
                    let animation_id = format!("hks-task-voice-{}", request.0);
                    runtime.task_requests.insert(request, task);
                    pending_script_commands.enqueue(ScriptCommand::Audio(
                        AudioCommand::PlayVoice {
                            path: definition.path.clone(),
                            volume,
                            mode: VoicePlaybackMode::Concurrent,
                            animation_id: Some(animation_id.clone()),
                        },
                    ));
                    pending_script_commands.enqueue(ScriptCommand::Animation(
                        AnimationCommand::Wait {
                            ids: vec![animation_id],
                            done: request,
                        },
                    ));
                }
                None => {
                    warn!("voice `{path}` is not defined");
                    if let Some(story) = runtime.story.as_mut()
                        && let Err(error) = story.resume_task(task)
                    {
                        warn!("failed to skip missing HKS task voice: {error}");
                    }
                }
            },
            StoryRuntimeEvent::TaskEffect { task, effect } => {
                warn!("unsupported HKS task effect for task {task}: {effect:?}");
                if let Some(story) = runtime.story.as_mut()
                    && let Err(error) = story.resume_task(task)
                {
                    warn!("failed to resume unsupported HKS task effect: {error}");
                }
            }
            StoryRuntimeEvent::Completed(_) => {
                if let Some(frame) = runtime.call_stack.pop() {
                    let globals = runtime
                        .story
                        .as_ref()
                        .map(|story| story.globals().clone())
                        .unwrap_or_default();
                    let mut caller = frame.story;
                    caller.set_globals(globals);
                    runtime.story = Some(caller);
                    runtime.current_script = Some(frame.script);
                    runtime.task_requests.clear();
                }
            }
        }
        return;
    }
}
