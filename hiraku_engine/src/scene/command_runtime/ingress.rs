use super::*;
use crate::script::StoryRuntimeEvent;
use crate::script::capabilities::StoryEffect;
use crate::script::replay::{InputKind, ReplayJournal, ReplayPoint};
use std::time::Duration;

fn replay_signature(kind: &str, value: &impl serde::Serialize) -> Option<String> {
    match hiraku_script::hson::to_string(value) {
        Ok(value) => Some(format!("{kind}:{value}")),
        Err(error) => {
            warn!("could not encode replay boundary: {error}");
            None
        }
    }
}

fn observe_replay_boundary(runtime: &mut ScriptRuntimeState, event: &StoryRuntimeEvent) {
    let script = runtime.current_script.clone().unwrap_or_default();
    let journal = runtime.replay.get_or_insert_with(|| {
        use std::hash::{BuildHasher, Hasher};
        let seed = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        ReplayJournal::new(script.clone(), seed)
    });
    use crate::script::capabilities::{StoryEffect, StoryWait};
    let signature = match event {
        StoryRuntimeEvent::Effect(StoryEffect::Say { speaker, text }) => {
            runtime.replay_dialogue = replay_signature("say", &(speaker, text)).unwrap_or_default();
            None
        }
        StoryRuntimeEvent::Effect(StoryEffect::ContinueDialogue { text }) => {
            runtime.replay_dialogue = replay_signature("append", text).unwrap_or_default();
            None
        }
        StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance) => {
            Some(format!("dialogue:{}", runtime.replay_dialogue))
        }
        StoryRuntimeEvent::Choice {
            prompt,
            options,
            enabled,
            parameters,
        } => replay_signature("choice", &(prompt, options, enabled, parameters)),
        StoryRuntimeEvent::OpenUi { path, arguments } => replay_signature("ui", &(path, arguments)),
        StoryRuntimeEvent::RandomInt { min, max } => replay_signature("random", &(min, max)),
        _ => None,
    };
    if journal.destination.is_none() {
        if let Some(signature) = signature {
            journal.destination = Some(ReplayPoint { script, signature });
        }
    }
}

fn record_replay_response(runtime: &mut ScriptRuntimeState, response: &ScriptResponse) {
    let Some(journal) = runtime.replay.as_mut() else {
        return;
    };
    let Some(point) = journal.destination.clone() else {
        return;
    };
    match response {
        ScriptResponse::Continue if point.signature.starts_with("dialogue:") => {
            if let Err(error) = journal.dialogue(&point) {
                journal.complete = false;
                warn!("could not record dialogue continuation: {error}");
            }
        }
        ScriptResponse::Choice(value) => {
            let kind = if point.signature.starts_with("choice:") {
                InputKind::Choice
            } else {
                InputKind::UiResult
            };
            journal.input(kind, point, value.clone());
        }
        ScriptResponse::UiResult(value) => {
            if let Some(value) = hks_to_stored(value) {
                journal.input(InputKind::UiResult, point, value);
            } else {
                journal.destination = None;
                journal.complete = false;
            }
        }
        _ => {
            journal.destination = None;
        }
    }
}

fn stored_to_hks(value: StoredValue) -> hiraku_script::Value {
    match value {
        StoredValue::Bool(value) => hiraku_script::Value::Bool(value),
        StoredValue::Int(value) => hiraku_script::Value::Int(value),
        StoredValue::UInt(value) => hiraku_script::Value::UInt(value),
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
        hiraku_script::Value::Int(value) => Some(StoredValue::Int(*value)),
        hiraku_script::Value::UInt(value) => Some(StoredValue::UInt(*value)),
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
pub(crate) fn evaluate_ui_at_with_arguments(
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
    values.insert("dialogue".to_string(), default_dialogue_model());
    values.insert("history".to_string(), default_history_model());
    values.extend(extra_values);
    let source = vfs.0.read_text(target).map_err(|error| error.to_string())?;
    let textures = textures.ok_or_else(|| "texture catalog is unavailable".to_string())?;
    let terms = terms.ok_or_else(|| "term catalog is unavailable".to_string())?;
    evaluate_ui_component_named_with_args(
        target,
        &source,
        UiContext::new(values).with_preferences(user_settings.clone()),
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
        ("autoEnabled".to_string(), StoredValue::Bool(false)),
        ("fastForwardEnabled".to_string(), StoredValue::Bool(false)),
    ]))
}

fn default_history_model() -> StoredValue {
    StoredValue::Map(BTreeMap::from([
        ("entries".to_string(), StoredValue::Array(Vec::new())),
        ("text".to_string(), StoredValue::String(String::new())),
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
    mut redraw: crate::redraw::Redraw,
    dependencies: Res<crate::dependencies::ScriptDependencies>,
    mut runtime: ResMut<ScriptRuntimeState>,
    mut response_messages: MessageReader<ScriptResponseMessage>,
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    textures: Option<Res<TextureCatalog>>,
    terms: Option<Res<TermCatalog>>,
    audio: Option<Res<AudioCatalog>>,
    movies: Option<Res<crate::movie::MovieCatalog>>,
    vfs: Res<VfsResource>,
    user_settings: Res<UserSettings>,
    models: Res<crate::ui::UiModels>,
) {
    for message in response_messages.read() {
        if let Some((task, effect)) = runtime.task_requests.remove(&message.request) {
            if let Some(story) = runtime.story.as_mut()
                && let Err(error) = story.complete_task_effect(task, &effect)
            {
                crate::script::emit_script_diagnostic(
                    &format!("failed to resume HKS task {task}"),
                    &error.to_string(),
                );
                runtime.story = None;
            }
        } else {
            runtime.accept_response(message.clone());
        }
    }

    if !crate::storage::storage_ready() || dependencies.loading {
        return;
    }

    if let Some(request) = runtime.wait_request
        && let Some(response) = runtime.take_response(request)
    {
        let direct_value = match &response {
            ScriptResponse::Choice(value) => stored_to_hks(value.clone()),
            ScriptResponse::Continue => hiraku_script::Value::Unit,
            ScriptResponse::UiResult(value) => value.clone(),
        };
        if runtime
            .story
            .as_ref()
            .is_some_and(|story| !story.accepts_choice_response(&direct_value))
        {
            return;
        }
        runtime.pending_ui_screen = None;
        runtime.pending_ui_arguments.clear();
        runtime.wait_request = None;
        let mut accepted = false;
        if let Some(story) = runtime.story.as_mut() {
            if !story.is_waiting_for_host_response() {
                // Host completions are asynchronous. Navigation, load, or a
                // competing completion may have invalidated this request after
                // it was emitted; the token has already been consumed above.
                debug!("discarded stale script response for request {}", request.0);
            } else if let Err(error) = story.resume(direct_value) {
                crate::script::emit_script_diagnostic(
                    "failed to resume script runtime",
                    &error.to_string(),
                );
                runtime.story = None;
            } else {
                accepted = true;
            }
        }
        if accepted {
            record_replay_response(&mut runtime, &response);
        }
    }

    // Submit consecutive non-blocking effects as one presentation batch. A
    // frame per effect exposes intermediate states (clear -> hide -> show).
    // Navigation and host waits remain barriers: later code must observe ECS
    // dispatch/results, not run ahead of them. Bound work for responsive input.
    for _ in 0..256 {
        let event = match runtime.story.as_mut() {
            Some(story) => match story.step() {
                Ok(event) => event,
                Err(error) => {
                    crate::script::emit_script_diagnostic("HKS runtime failed", &error.to_string());
                    runtime.story = None;
                    None
                }
            },
            None => None,
        };

        if let Some(event) = event {
            let continue_batch = matches!(&event,
                StoryRuntimeEvent::Effect(effect)
                    if !matches!(effect, StoryEffect::Navigate(_) | StoryEffect::Exit | StoryEffect::SaveSlot(_))
            );
            // Effects are dispatched in later systems; their completion/next story
            // step must not depend on a subsequent physical input event.
            redraw.request();
            observe_replay_boundary(&mut runtime, &event);
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
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlayBgm {
                                path: definition.path.clone(),
                                prelude: definition.prelude.clone(),
                                volume,
                                fade_in: fade_in_ms.map(std::time::Duration::from_millis),
                                animation_id: None,
                            },
                        ));
                    }
                    None => warn!("music `{path}` is not defined"),
                },
                StoryRuntimeEvent::Effect(crate::script::capabilities::StoryEffect::PlaySfx {
                    path,
                    volume,
                    fade_in_ms,
                }) => {
                    if let Some(definition) = audio
                        .as_deref()
                        .and_then(|catalog| catalog.resolve_sfx(&path))
                    {
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlaySfx {
                                channel: None,
                                looped: false,
                                path: definition.path.clone(),
                                volume,
                                fade_in: fade_in_ms.map(Duration::from_millis),
                                animation_id: None,
                            },
                        ));
                    } else {
                        warn!("sound effect `{path}` is not defined");
                    }
                }
                StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::PlaySfxChannel {
                        channel,
                        path,
                        volume,
                        fade_in_ms,
                        looped,
                    },
                ) => {
                    if let Some(definition) = audio
                        .as_deref()
                        .and_then(|catalog| catalog.resolve_sfx(&path))
                    {
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlaySfx {
                                channel: Some(channel),
                                looped,
                                path: definition.path.clone(),
                                volume,
                                fade_in: fade_in_ms.map(Duration::from_millis),
                                animation_id: None,
                            },
                        ));
                    } else {
                        warn!("sound effect `{path}` is not defined");
                    }
                }
                StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::PlayVoice { path, volume },
                ) => match audio
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
                StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::SetUiRole { role, component },
                ) => {
                    let target = resolve_ui_component_path(
                        &vfs,
                        runtime.current_script.as_deref(),
                        &component,
                    );
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
                                        lifetime: None,
                                    },
                                ));
                            }
                            Err(error) => {
                                crate::script::emit_script_diagnostic(
                                    &format!("failed to enable dialogue UI `{component}`"),
                                    &error.to_string(),
                                );
                                runtime.story = None;
                            }
                        }
                    }
                }
                StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::MountUiOverlay {
                        name,
                        component,
                        lifetime,
                    },
                ) => {
                    let target =
                        runtime
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
                            // Timed notifications are presentation-only, not
                            // persistent UI roles restored from a save.
                            if lifetime.is_none() {
                                runtime
                                    .mounted_ui_overlays
                                    .insert(name.clone(), target.clone());
                            } else {
                                runtime.mounted_ui_overlays.remove(&name);
                            }
                            pending_script_commands.enqueue(ScriptCommand::Ui(
                                UiCommand::ShowOverlay {
                                    name,
                                    screen,
                                    lifetime,
                                },
                            ));
                        }
                        Err(error) => crate::script::emit_script_diagnostic(
                            &format!("failed to mount UI overlay `{name}` from `{target}`"),
                            &error.to_string(),
                        ),
                    }
                }
                StoryRuntimeEvent::Effect(
                    crate::script::capabilities::StoryEffect::UnmountUiOverlay { name },
                ) => {
                    runtime.mounted_ui_overlays.remove(&name);
                    pending_script_commands
                        .enqueue(ScriptCommand::Ui(UiCommand::HideOverlay { name }));
                }
                StoryRuntimeEvent::Effect(
                    effect @ (crate::script::capabilities::StoryEffect::Say { .. }
                    | crate::script::capabilities::StoryEffect::ContinueDialogue { .. }),
                ) => {
                    let Some(dialogue_component) = runtime.ui_registry.get("dialogue").cloned()
                    else {
                        warn!(
                            "dialogue UI is not configured; call ui.set(\"dialogue\", \"path/to/dialogue.ui.hks\") before executing dialogue"
                        );
                        runtime.story = None;
                        return;
                    };

                    let dialogue_overlay = "__role.dialogue";
                    let dialogue_ready =
                        if runtime.mounted_ui_overlays.contains_key(dialogue_overlay) {
                            true
                        } else {
                            match evaluate_ui_at(
                                &dialogue_component,
                                &runtime,
                                &vfs,
                                &user_settings,
                                textures.as_deref(),
                                terms.as_deref(),
                            ) {
                                Ok(screen) => {
                                    runtime.mounted_ui_overlays.insert(
                                        dialogue_overlay.into(),
                                        dialogue_component.clone(),
                                    );
                                    pending_script_commands.enqueue(ScriptCommand::Ui(
                                        UiCommand::ShowOverlay {
                                            name: dialogue_overlay.into(),
                                            screen,
                                            lifetime: None,
                                        },
                                    ));
                                    true
                                }
                                Err(error) => {
                                    crate::script::emit_script_diagnostic(
                                        &format!(
                                            "failed to restore dialogue UI `{dialogue_component}`"
                                        ),
                                        &error.to_string(),
                                    );
                                    runtime.story = None;
                                    false
                                }
                            }
                        };

                    if dialogue_ready {
                        match script_command_from_effect(effect, textures.as_deref()) {
                            Ok(command) => {
                                pending_script_commands.enqueue(command);
                            }
                            Err(error) => crate::script::emit_script_diagnostic(
                                "HKS dialogue command rejected",
                                &error.to_string(),
                            ),
                        }
                    }
                }
                StoryRuntimeEvent::Wait(
                    crate::script::capabilities::StoryWait::DialogueAdvance,
                ) => {
                    let request = runtime.allocate_request();
                    runtime.wait_request = Some(request);
                    pending_script_commands.enqueue(ScriptCommand::Dialogue(
                        DialogueCommand::AwaitAdvance { done: request },
                    ));
                }
                StoryRuntimeEvent::Wait(crate::script::capabilities::StoryWait::Delay {
                    duration_ms,
                }) => {
                    let request = runtime.allocate_request();
                    runtime.wait_request = Some(request);
                    pending_script_commands.enqueue(ScriptCommand::Animation(
                        AnimationCommand::Delay {
                            duration: Duration::from_millis(duration_ms),
                            done: request,
                        },
                    ));
                }
                event @ (StoryRuntimeEvent::Wait(
                    crate::script::capabilities::StoryWait::Movie { .. },
                )
                | StoryRuntimeEvent::Effect(StoryEffect::MovieBackground { .. })) => {
                    let (path, blocking, fade_out_ms) = match event {
                        StoryRuntimeEvent::Wait(
                            crate::script::capabilities::StoryWait::Movie { path, fade_out_ms },
                        ) => (path, true, fade_out_ms),
                        StoryRuntimeEvent::Effect(StoryEffect::MovieBackground {
                            path,
                            fade_out_ms,
                        }) => (path, false, fade_out_ms),
                        _ => unreachable!("matched movie events"),
                    };
                    let target = movies
                        .as_deref()
                        .and_then(|catalog| catalog.resolve(&path))
                        .map(|definition| (definition.path.clone(), definition.layout))
                        .or_else(|| {
                            let lower = path.to_ascii_lowercase();
                            [".mkv", ".webm", ".mkva", ".webma"]
                                .iter()
                                .any(|ext| lower.ends_with(ext))
                                .then(|| {
                                    (
                                        vfs.0
                                            .resolve_path(runtime.current_script.as_deref(), &path),
                                        hiraku_video::AlphaLayout::default(),
                                    )
                                })
                        });
                    let Some((target, layout)) = target else {
                        warn!("movie `{path}` is not defined");
                        runtime.story = None;
                        return;
                    };
                    let done = if blocking {
                        let request = runtime.allocate_request();
                        runtime.wait_request = Some(request);
                        Some(request)
                    } else {
                        None
                    };
                    pending_script_commands.enqueue(ScriptCommand::Video(VideoCommand::Play {
                        path: target,
                        layout,
                        fade_out: Duration::from_millis(fade_out_ms),
                        done,
                    }));
                }
                StoryRuntimeEvent::Effect(effect) => {
                    match script_command_from_effect(effect, textures.as_deref()) {
                        Ok(command) => {
                            pending_script_commands.enqueue(command);
                        }
                        Err(error) => crate::script::emit_script_diagnostic(
                            "HKS native command rejected",
                            &error.to_string(),
                        ),
                    }
                }
                StoryRuntimeEvent::Choice {
                    prompt,
                    options,
                    enabled,
                    parameters,
                } => {
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
                        (
                            "parameters".into(),
                            StoredValue::Map(
                                parameters
                                    .into_iter()
                                    .enumerate()
                                    .filter_map(|(index, value)| {
                                        value.map(|value| (index.to_string(), value))
                                    })
                                    .collect(),
                            ),
                        ),
                        (
                            "enabled".into(),
                            StoredValue::Array(
                                enabled.into_iter().map(StoredValue::Bool).collect(),
                            ),
                        ),
                        ("prompt".into(), StoredValue::String(prompt)),
                        (
                            "options".into(),
                            StoredValue::Array(
                                options.into_iter().map(StoredValue::String).collect(),
                            ),
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
                            pending_script_commands.enqueue(ScriptCommand::Ui(
                                UiCommand::ShowScreen {
                                    screen,
                                    done: Some(request),
                                    push: false,
                                },
                            ));
                        }
                        Err(error) => {
                            crate::script::emit_script_diagnostic(
                                &format!("failed to render choice UI `{target}`"),
                                &error.to_string(),
                            );
                            runtime.story = None;
                        }
                    }
                }
                StoryRuntimeEvent::RandomInt { min, max } => {
                    let journal = runtime
                        .replay
                        .as_mut()
                        .expect("observed story event has a journal");
                    let value = journal.random_int(min, max);
                    if let Some(story) = runtime.story.as_mut() {
                        if let Err(error) = story.resume(hiraku_script::Value::Int(value as i64)) {
                            warn!("failed to resume randomInt: {error}");
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
                        .map(crate::script::ui_argument_to_stored)
                        .collect::<Result<Vec<_>, _>>();
                    let arguments = match arguments {
                        Ok(arguments) => arguments,
                        Err(error) => {
                            warn!("invalid ui.open arguments: {error}");
                            runtime.story = None;
                            return;
                        }
                    };
                    let screen = evaluate_ui_at_with_arguments(
                        &target,
                        &runtime,
                        &vfs,
                        &user_settings,
                        textures.as_deref(),
                        terms.as_deref(),
                        models
                            .roots()
                            .map(|(name, value)| (name.to_owned(), value.clone()))
                            .collect(),
                        &arguments,
                    );
                    let request = runtime.allocate_request();
                    runtime.pending_ui_screen = Some(target.clone());
                    runtime.pending_ui_arguments = arguments;
                    runtime.wait_request = Some(request);
                    match screen {
                        Ok(screen) => {
                            pending_script_commands.enqueue(ScriptCommand::Ui(
                                UiCommand::ShowScreen {
                                    screen,
                                    done: Some(request),
                                    push: false,
                                },
                            ));
                        }
                        Err(error) => {
                            crate::script::emit_script_diagnostic(
                                &format!("failed to render UI script `{target}`"),
                                &error.to_string(),
                            );
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
                        runtime.task_requests.insert(
                            request,
                            (
                                task,
                                crate::script::capabilities::StoryEffect::PlayVoice {
                                    path: path.clone(),
                                    volume,
                                },
                            ),
                        );
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlayVoice {
                                path: definition.path.clone(),
                                volume,
                                mode: runtime
                                    .story
                                    .as_ref()
                                    .map_or(VoicePlaybackMode::Exclusive, |story| {
                                        story.voice_playback_mode(task)
                                    }),
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
                            && let Err(error) = story.complete_task_effect(
                                task,
                                &crate::script::capabilities::StoryEffect::PlayVoice {
                                    path: path.clone(),
                                    volume,
                                },
                            )
                        {
                            crate::script::emit_script_diagnostic(
                                "failed to skip missing HKS task voice",
                                &error.to_string(),
                            );
                        }
                    }
                },
                StoryRuntimeEvent::TaskEffect {
                    task,
                    effect: crate::script::capabilities::StoryEffect::Delay { duration_ms },
                } => {
                    let request = runtime.allocate_request();
                    runtime.task_requests.insert(
                        request,
                        (
                            task,
                            crate::script::capabilities::StoryEffect::Delay { duration_ms },
                        ),
                    );
                    pending_script_commands.enqueue(ScriptCommand::Animation(
                        AnimationCommand::Delay {
                            duration: Duration::from_millis(duration_ms),
                            done: request,
                        },
                    ));
                }
                StoryRuntimeEvent::TaskEffect {
                    task,
                    effect:
                        effect @ (crate::script::capabilities::StoryEffect::PlaySfx { .. }
                        | crate::script::capabilities::StoryEffect::PlaySfxChannel { .. }),
                } => {
                    let (path, volume, fade_in_ms, channel, looped) = match &effect {
                        crate::script::capabilities::StoryEffect::PlaySfx {
                            path,
                            volume,
                            fade_in_ms,
                        } => (path, *volume, *fade_in_ms, None, false),
                        crate::script::capabilities::StoryEffect::PlaySfxChannel {
                            path,
                            volume,
                            fade_in_ms,
                            channel,
                            looped,
                        } => (path, *volume, *fade_in_ms, Some(channel.clone()), *looped),
                        _ => unreachable!("matched audio task effect"),
                    };
                    if let Some(definition) = audio
                        .as_deref()
                        .and_then(|catalog| catalog.resolve_sfx(&path))
                    {
                        let request = runtime.allocate_request();
                        let animation_id = format!("hks-task-sfx-{}", request.0);
                        runtime
                            .task_requests
                            .insert(request, (task, effect.clone()));
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlaySfx {
                                channel,
                                looped,
                                path: definition.path.clone(),
                                volume,
                                fade_in: fade_in_ms.map(Duration::from_millis),
                                animation_id: Some(animation_id.clone()),
                            },
                        ));
                        pending_script_commands.enqueue(ScriptCommand::Animation(
                            AnimationCommand::Wait {
                                ids: vec![animation_id],
                                done: request,
                            },
                        ));
                    } else {
                        warn!("sound effect `{path}` is not defined");
                        if let Some(story) = runtime.story.as_mut()
                            && let Err(error) = story.complete_task_effect(task, &effect)
                        {
                            crate::script::emit_script_diagnostic(
                                "failed to complete missing task sound",
                                &error.to_string(),
                            );
                        }
                    }
                }
                StoryRuntimeEvent::TaskEffect {
                    task,
                    effect: effect @ crate::script::capabilities::StoryEffect::PlayBgm { .. },
                } => {
                    let crate::script::capabilities::StoryEffect::PlayBgm {
                        path,
                        volume,
                        fade_in_ms,
                    } = &effect
                    else {
                        unreachable!()
                    };
                    if let Some(definition) = audio
                        .as_deref()
                        .and_then(|catalog| catalog.resolve_music(path))
                    {
                        let request = runtime.allocate_request();
                        let id = format!("hks-bgm-{}", request.0);
                        pending_script_commands.enqueue(ScriptCommand::Audio(
                            AudioCommand::PlayBgm {
                                path: definition.path.clone(),
                                prelude: definition.prelude.clone(),
                                volume: *volume,
                                fade_in: fade_in_ms.map(Duration::from_millis),
                                animation_id: Some(id.clone()),
                            },
                        ));
                        runtime.task_requests.insert(request, (task, effect));
                        pending_script_commands.enqueue(ScriptCommand::Animation(
                            AnimationCommand::Wait {
                                ids: vec![id],
                                done: request,
                            },
                        ));
                    } else {
                        warn!("music `{path}` is not defined");
                        if let Some(story) = runtime.story.as_mut() {
                            if let Err(error) = story.complete_task_effect(task, &effect) {
                                crate::script::emit_script_diagnostic(
                                    "failed to complete missing music",
                                    &error.to_string(),
                                );
                            }
                        }
                    }
                }
                StoryRuntimeEvent::TaskEffect { task, effect } => {
                    let mut command = match crate::script::script_command_from_effect(
                        effect.clone(),
                        textures.as_deref(),
                    ) {
                        Ok(command) => command,
                        Err(error) => {
                            crate::script::emit_script_diagnostic(
                                "failed to dispatch task effect",
                                &error,
                            );
                            runtime.story = None;
                            return;
                        }
                    };
                    let request = runtime.allocate_request();
                    let id = format!("hks-effect-{}", request.0);
                    let wait = match &mut command {
                        ScriptCommand::Camera(CameraCommand::Set { animation_id, .. })
                        | ScriptCommand::Camera(CameraCommand::Shake { animation_id, .. })
                        | ScriptCommand::Audio(AudioCommand::StopBgm { animation_id, .. })
                        | ScriptCommand::Stage(StageCommand::SetBackground {
                            animation_id, ..
                        })
                        | ScriptCommand::Character(CharacterCommand::Motion {
                            animation_id, ..
                        })
                        | ScriptCommand::Dialogue(DialogueCommand::Say { animation_id, .. })
                        | ScriptCommand::Dialogue(DialogueCommand::Continue {
                            animation_id, ..
                        }) => {
                            *animation_id = Some(id.clone());
                            ScriptCommand::Animation(AnimationCommand::Wait {
                                ids: vec![id],
                                done: request,
                            })
                        }
                        ScriptCommand::Character(CharacterCommand::Show {
                            animation_id,
                            placement_animation_id,
                            placement_animation,
                            ..
                        }) => {
                            *animation_id = Some(id.clone());
                            let mut ids = vec![id.clone()];
                            if placement_animation.is_some() {
                                let placement_id = format!("{id}-placement");
                                *placement_animation_id = Some(placement_id.clone());
                                ids.push(placement_id);
                            }
                            ScriptCommand::Animation(AnimationCommand::Wait { ids, done: request })
                        }
                        ScriptCommand::Stage(StageCommand::SetCurtain { .. }) => {
                            ScriptCommand::Stage(StageCommand::AwaitCurtain { done: request })
                        }
                        ScriptCommand::Stage(StageCommand::Picture(picture)) => {
                            ScriptCommand::Animation(AnimationCommand::Scene {
                                effect: super::super::effect_wait::SceneEffect::Picture(
                                    picture.clone(),
                                ),
                                done: request,
                            })
                        }
                        ScriptCommand::Stage(StageCommand::Spatial(command)) => {
                            ScriptCommand::Animation(AnimationCommand::Scene {
                                effect: super::super::effect_wait::SceneEffect::Spatial(
                                    command.clone(),
                                ),
                                done: request,
                            })
                        }
                        ScriptCommand::Character(CharacterCommand::Hide { actor_id, .. }) => {
                            ScriptCommand::Animation(AnimationCommand::Scene {
                                effect: super::super::effect_wait::SceneEffect::HideCharacter(
                                    actor_id.clone(),
                                ),
                                done: request,
                            })
                        }
                        _ => {
                            crate::script::emit_script_diagnostic(
                                "failed to dispatch task effect",
                                "effect has no completion protocol",
                            );
                            runtime.story = None;
                            return;
                        }
                    };
                    runtime.task_requests.insert(request, (task, effect));
                    pending_script_commands.enqueue(command);
                    pending_script_commands.enqueue(wait);
                }
                StoryRuntimeEvent::Completed(_) => {
                    if let Some(frame) = runtime.call_stack.pop() {
                        let globals = runtime
                            .story
                            .as_ref()
                            .map(|story| story.globals().clone())
                            .unwrap_or_default();
                        let mut caller = frame.story;
                        let mut merged = caller.globals().clone();
                        merged.extend(globals);
                        caller.set_globals(merged);
                        if let Some(callee) = runtime.story.as_ref() {
                            caller.inherit_native_state(callee, false);
                        }
                        runtime.story = Some(caller);
                        runtime.current_script = Some(frame.script);
                        runtime.task_requests.clear();
                    }
                }
            }
            if continue_batch {
                continue;
            }
            return;
        }
        return;
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn consecutive_presentation_commands_are_queued_in_one_frame() {
        let code = crate::script::compile_story_bytecode(
            "scene.hks",
            r#"
            scene.clearPictures()
            scene.hideCharacters(0)
            scene.curtain(0).fade(0)
            story.goto("next.hks", .{ preload: false })
        "#,
        )
        .expect("compile synthetic scene");
        let mut runtime = ScriptRuntimeState::default();
        runtime.story = Some(StoryRuntime::new(code).expect("story"));
        runtime.current_script = Some("scene.hks".into());
        let mut app = App::new();
        app.init_resource::<crate::dependencies::ScriptDependencies>()
            .insert_resource(runtime)
            .add_message::<ScriptResponseMessage>()
            .init_resource::<PendingScriptCommands>()
            .insert_resource(VfsResource(std::sync::Arc::new(crate::vfs::HdpVfs::new(
                "unused-fixture-root",
            ))))
            .init_resource::<UserSettings>()
            .init_resource::<crate::ui::UiModels>()
            .add_systems(Update, drive_story_runtime);
        app.update();
        let queue = app.world().resource::<PendingScriptCommands>();
        assert_eq!(
            queue.items.len(),
            4,
            "clear, hide, curtain and navigation form one batch"
        );
        assert!(matches!(
            queue.items.front().expect("first").command,
            ScriptCommand::Stage(StageCommand::Picture(
                crate::scene::pictures::PictureCommand::Clear
            ))
        ));
        assert!(matches!(
            queue.items.back().expect("last").command,
            ScriptCommand::Runtime(RuntimeCommand::Navigate(_))
        ));
    }
}
