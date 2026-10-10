use super::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};

type PreparedLoad = Result<(crate::state::SaveGameData, ScriptRuntimeState), String>;

#[derive(Default)]
pub struct PendingSaveLoad {
    request: Option<SaveLoadRequest>,
    task: Option<Task<PreparedLoad>>,
    slot: String,
}

struct SaveLoadRequest {
    slot: String,
    history: Option<Result<crate::state::SaveGameData, String>>,
    vfs: VfsResource,
    settings: UserSettings,
    program: Option<crate::script::StoryProgram>,
}

fn prepare_load(request: SaveLoadRequest) -> PreparedLoad {
    let mut save = match request.history {
        Some(result) => result?,
        None => load_save_data(&request.slot).map_err(|e| e.to_string())?,
    };
    let bootstrap = ScriptBootstrap::take_from_save(&mut save)?;
    let program = request
        .program
        .and_then(|p| p.for_path(&bootstrap.startup_script));
    let mut runtime = ScriptRuntimeState::default();
    crate::script::start_story_runtime_with_program(
        &request.vfs,
        &mut runtime,
        bootstrap,
        &request.settings,
        program,
    )?;
    Ok((save, runtime))
}

/// Marker for an application-owned modal layered above Hiraku's script UI.
#[derive(Component)]
pub struct PauseMenuRoot;

#[derive(Component)]
pub struct RuntimeMenuButton {
    pub callback: crate::ui::UiCallback,
    /// Modal screen which owns this action.
    pub screen_root: Option<Entity>,
}

#[derive(SystemParam)]
pub struct RuntimeMenuContext<'w, 's> {
    pub fades: Query<'w, 's, &'static super::ui_visuals::ScreenFade>,
    pub preview: Res<'w, super::save_preview::SavePreview>,
    pub local_states: Query<'w, 's, &'static mut super::widgets::UiLocalState>,
    pub commands: Commands<'w, 's>,
    pub asset_server: Res<'w, AssetServer>,
    pub textures: Res<'w, TextureCatalog>,
    pub terms: Res<'w, TermCatalog>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: ResMut<'w, SceneSharedState>,
    pub script_runtime: ResMut<'w, ScriptRuntimeState>,
    pub frontend: ResMut<'w, FrontendState>,
    pub user_settings: Res<'w, UserSettings>,
    pub models: Res<'w, crate::ui::UiModels>,
    pub effects: MessageWriter<'w, super::screen_ui::UiEffectMessage>,
    pub ui_style: Res<'w, UiStyle>,
    pub dialogue_history: ResMut<'w, DialogueHistoryState>,
    pub stage: ResMut<'w, StageState>,
    pub waits: ResMut<'w, PendingWaits>,
    pub pending_script_commands: ResMut<'w, PendingScriptCommands>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub animations: ResMut<'w, AnimationState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub dialogue_chars: Query<'w, 's, &'static mut DialogueCharSpan>,
    pub responses: MessageWriter<'w, ScriptResponseMessage>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root:
        Query<'w, 's, &'static mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub clicks: MessageReader<'w, 's, PointerClick>,
    pub widget_callbacks: MessageReader<'w, 's, super::widgets::UiCallbackRequest>,
    pub entities: Query<'w, 's, Entity>,
    pub action_query: Query<
        'w,
        's,
        (
            &'static RuntimeMenuButton,
            Option<&'static ScreenUiButton>,
            Option<&'static ScreenUiImageButton>,
        ),
    >,
    pub parents: Query<'w, 's, &'static ChildOf>,
    pub images: Query<'w, 's, &'static mut ImageNode>,
}

pub fn update_runtime_menu_button_visuals(
    ui_style: Res<UiStyle>,
    screen_state: Res<ScreenUiState>,
    overlay_state: Res<OverlayUiState>,
    mut buttons: Query<
        (
            &PickingInteraction,
            &mut BackgroundColor,
            &RuntimeMenuButton,
            Option<&ScreenUiButton>,
            Option<&ScreenUiImageButton>,
        ),
        Changed<PickingInteraction>,
    >,
) {
    for (interaction, mut color, button, screen_button, image_button) in &mut buttons {
        if let Some(root) = button.screen_root
            && !screen_state.accepts_input(root, &overlay_state)
        {
            continue;
        }
        if screen_button.is_some_and(|button| !button.enabled) || screen_button.is_some() {
            continue;
        }
        if image_button.is_some() {
            *color = Color::NONE.into();
            continue;
        }
        *color = match *interaction {
            PickingInteraction::Pressed => ui_style.choice_button_pressed.into(),
            PickingInteraction::Hovered => ui_style.choice_button_hovered.into(),
            PickingInteraction::None => ui_style.choice_button_bg.into(),
        };
    }
}

fn restore_frontend_scene(
    commands: &mut Commands,
    asset_server: &AssetServer,
    shared_state: &mut SceneSharedState,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    dialogue_root: &mut Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    user_settings: &UserSettings,
    frontend: &mut FrontendState,
    snapshot: SceneSnapshot,
) {
    clear_choice_ui(commands, choice_ui);
    shared_state.0 = snapshot.clone();
    restore_scene_snapshot(
        commands,
        asset_server,
        stage,
        dialogue_state,
        choice_state,
        dialogue_root,
        speaker_text,
        line_text,
        user_settings,
        snapshot,
    );

    frontend.notice = None;
    frontend.runtime_started = true;
}

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(
    mut redraw: crate::redraw::Redraw,
    mut ctx: RuntimeMenuContext,
    mut deferred: Local<Vec<crate::ui::UiEffect>>,
    mut pending_load: Local<PendingSaveLoad>,
    dependencies: Option<Res<crate::dependencies::ScriptDependencies>>,
) {
    // Give earlier effects a complete frame to submit their audio players,
    // including on executors which run tasks on the browser's main thread.
    if let Some(request) = pending_load.request.take() {
        redraw.request();
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        pending_load.task =
            Some(AsyncComputeTaskPool::get().spawn(async move { prepare_load(request) }));
        return;
    }
    if let Some(task) = pending_load.task.as_mut() {
        redraw.request();
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        if let Some(result) = check_ready(task) {
            pending_load.task = None;
            ctx.script_runtime.restore_pending = false;
            match result {
                Ok((save, runtime)) => {
                    ctx.script_runtime.apply_restored(runtime);
                    finish_load(&mut ctx, save);
                    info!("loaded save slot `{}`", pending_load.slot);
                }
                Err(error) => {
                    crate::script::emit_script_diagnostic(
                        &format!("failed to restore slot `{}`", pending_load.slot),
                        &error,
                    );
                    ctx.frontend.notice = Some(format!(
                        "Failed to load slot {}: {error}",
                        pending_load.slot
                    ));
                }
            }
        }
        return;
    }
    if dependencies.is_some_and(|d| d.loading) {
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        return;
    }
    if !crate::storage::storage_ready() {
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        if matches!(
            hiraku_storage::runtime_status(),
            hiraku_storage::RuntimeStorageStatus::Failed(_)
        ) {
            deferred.clear();
        }
        return;
    }
    if !deferred.is_empty() {
        redraw.request();
        let effects = std::mem::take(&mut *deferred);
        let root = ctx.screen_state.active_root;
        dispatch_ui_effects(&mut ctx, effects, &mut deferred, &mut pending_load, root);
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        return;
    }
    if ctx.shared_state.0.story_input_blocked
        || ctx
            .script_runtime
            .story
            .as_ref()
            .is_some_and(|story| story.blocks_ui_input())
    {
        // Consume, rather than defer, clicks made behind an unskippable movie.
        ctx.clicks.clear();
        ctx.widget_callbacks.clear();
        return;
    }
    let mut invocations = Vec::new();
    for click in ctx.clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let Some(button_entity) =
            find_component_ancestor(click.entity, &ctx.action_query, &ctx.parents)
        else {
            continue;
        };
        let Ok((button, screen_button, image_button)) = ctx.action_query.get(button_entity) else {
            continue;
        };
        if let Some(root) = button.screen_root
            && !ctx.screen_state.accepts_input(root, &ctx.overlay_state)
        {
            continue;
        }
        if screen_button.is_some_and(|button| !button.enabled) {
            continue;
        }
        if image_button.is_some_and(|button| !button.enabled) {
            continue;
        }
        // The action may replace or cover this node before picking emits a
        // later interaction transition. Restore its release visual now.
        if let Some(image_button) = image_button {
            if let Ok(mut image) = ctx.images.get_mut(button_entity) {
                restore_image_source(
                    &mut image,
                    &image_button.normal_texture,
                    &image_button.normal_atlas,
                    image_button.normal_rect,
                );
            }
            ctx.commands.entity(button_entity).insert((
                BackgroundColor(Color::NONE),
                UiTransform::IDENTITY,
                image_button.normal_node.clone(),
            ));
        } else if let Some(screen_button) = screen_button {
            ctx.commands.entity(button_entity).insert((
                BackgroundColor(screen_button.normal_background),
                UiTransform::IDENTITY,
            ));
            ctx.commands
                .entity(screen_button.text_entity)
                .insert(TextColor(screen_button.normal_text_color));
            if let Some(texture) = screen_button.normal_texture.as_ref() {
                if let Ok(mut image) = ctx.images.get_mut(button_entity) {
                    restore_image_source(
                        &mut image,
                        texture,
                        &screen_button.normal_atlas,
                        screen_button.normal_rect,
                    );
                }
            }
        } else {
            ctx.commands
                .entity(button_entity)
                .insert(BackgroundColor(ctx.ui_style.choice_button_bg));
        }
        invocations.push((button.screen_root, button.callback.clone(), Vec::new()));
    }
    for request in ctx.widget_callbacks.read() {
        if ctx.entities.contains(request.entity)
            && ctx
                .screen_state
                .accepts_input(request.root, &ctx.overlay_state)
        {
            invocations.push((
                Some(request.root),
                request.callback.clone(),
                request.arguments.clone(),
            ));
        }
    }
    for (root, callback, arguments) in invocations {
        if let Some(root) = root
            && !ctx.screen_state.accepts_input(root, &ctx.overlay_state)
        {
            continue;
        }
        // A callback can change local UI state without emitting a story
        // command. Allow the dependent layout/visual systems to finish too.
        redraw.request();
        let story_globals = ctx.script_runtime.story.as_ref().map(|story| story.globals());
        let local_globals = root.and_then(|root| ctx.local_states.get(root).ok());
        let mut globals = std::collections::BTreeMap::new();
        for name in callback.required_globals.iter() {
            if let Some(value) = local_globals.and_then(|local| local.0.get(name))
                .or_else(|| story_globals.and_then(|story| story.get(name)))
            {
                globals.insert(name.clone(), value.clone());
            }
        }
        let (effects, updated) = match crate::script::evaluate_ui_callback_with_args(
            &callback,
            &globals,
            &ctx.models,
            arguments,
        ) {
            Ok(result) => result,
            Err(error) => {
                crate::script::emit_script_diagnostic("UI callback failed", &error.to_string());
                continue;
            }
        };
        // Accept only this screen's explicitly declared state. No story writeback.
        if let Some(root) = root
            && let Ok(mut local) = ctx.local_states.get_mut(root)
        {
            for (name, value) in updated {
                if callback.owned_globals.contains(&name) {
                    local.0.insert(name, value);
                }
            }
        }
        dispatch_ui_effects(&mut ctx, effects, &mut deferred, &mut pending_load, root);
        if pending_load.request.is_some() || pending_load.task.is_some() {
            redraw.request();
            break;
        }
        if !crate::storage::storage_ready() {
            return;
        }
    }
}

fn finish_load(ctx: &mut RuntimeMenuContext, mut save: crate::state::SaveGameData) {
    info!(
        "restoring save presentation using {}",
        if ctx.script_runtime.recovered_presentation.is_some() {
            "deterministic replay"
        } else {
            "matching checkpoint"
        }
    );
    ctx.script_runtime.restore_presentation(&mut save.scene);
    abort_runtime_waiters(
        &mut ctx.commands,
        &mut ctx.waits,
        &mut ctx.dialogue_state,
        &mut ctx.choice_state,
        &mut ctx.screen_state,
        &mut ctx.pending_script_commands,
        &mut ctx.pending_characters,
        &mut ctx.animations,
        &mut ctx.voice_state,
        &ctx.choice_ui_roots,
    );
    ctx.dialogue_history.restore(save.dialogue_history);
    ctx.dialogue_history.restore_records(save.history_records);
    clear_overlay_ui(&mut ctx.commands, &mut ctx.overlay_state);
    clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
    restore_frontend_scene(
        &mut ctx.commands,
        &ctx.asset_server,
        &mut ctx.shared_state,
        &mut ctx.stage,
        &mut ctx.dialogue_state,
        &mut ctx.choice_state,
        &ctx.choice_ui_roots,
        &mut ctx.dialogue_root,
        &mut ctx.speaker_text,
        &mut ctx.line_text,
        &ctx.user_settings,
        &mut ctx.frontend,
        save.scene,
    );
}

fn dispatch_ui_effects(
    ctx: &mut RuntimeMenuContext,
    effects: Vec<crate::ui::UiEffect>,
    deferred: &mut Vec<crate::ui::UiEffect>,
    pending_load: &mut PendingSaveLoad,
    root: Option<Entity>,
) {
    if !crate::storage::storage_ready() {
        deferred.extend(effects);
        return;
    }
    let mut effects = effects.into_iter();
    while let Some(effect) = effects.next() {
        match &effect {
            crate::ui::UiEffect::StopVoice => {
                finish_all_voices(&mut ctx.commands, &mut ctx.animations, &mut ctx.voice_state);
            }
            crate::ui::UiEffect::SetPreference(change) => {
                ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                    SettingsCommand::Preference(change.clone()),
                ));
            }
            crate::ui::UiEffect::SetAutoDialogue(enabled) => {
                if *enabled
                    && (!ctx.shared_state.0.input_permissions.auto
                        || ctx.shared_state.0.story_input_blocked)
                {
                    continue;
                }
                ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                    SettingsCommand::AutoDialogue(*enabled),
                ));
            }
            crate::ui::UiEffect::SetFastForward(enabled) => {
                if *enabled
                    && (!ctx.shared_state.0.input_permissions.fast_forward
                        || ctx.shared_state.0.story_input_blocked)
                {
                    continue;
                }
                ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                    SettingsCommand::FastForward(*enabled),
                ));
            }
            crate::ui::UiEffect::SetVolume { channel, value } => {
                ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                    SettingsCommand::Set {
                        name: channel.clone(),
                        value: *value,
                    },
                ));
            }
            crate::ui::UiEffect::PlaySfx { .. }
            | crate::ui::UiEffect::PlaySfxChannel { .. }
            | crate::ui::UiEffect::StopSfx { .. } => {
                ctx.effects
                    .write(super::screen_ui::UiEffectMessage(effect.clone(), root));
            }
            crate::ui::UiEffect::Save { slot } => {
                if let Err(error) = save_runtime_slot(
                    slot,
                    &ctx.script_runtime,
                    &ctx.shared_state,
                    if ctx.screen_state.active_root.is_some() {
                        &ctx.preview.png
                    } else {
                        &[]
                    },
                    &ctx.dialogue_history,
                ) {
                    warn!("failed to save slot `{slot}`: {error}");
                    ctx.frontend.notice = Some(format!("Failed to save slot {slot}: {error}"));
                    break;
                }
            }
            effect @ (crate::ui::UiEffect::Load { .. }
            | crate::ui::UiEffect::HistoryRollback { .. }) => {
                if matches!(effect, crate::ui::UiEffect::HistoryRollback { .. })
                    && (!ctx.shared_state.0.input_permissions.rollback
                        || ctx.shared_state.0.story_input_blocked)
                {
                    continue;
                }
                let (slot, history) = match effect {
                    crate::ui::UiEffect::Load { slot } => (slot.clone(), None),
                    crate::ui::UiEffect::HistoryRollback { id } => (
                        format!("history:{id}"),
                        Some(
                            ctx.dialogue_history
                                .rollback_save(*id)
                                .map_err(|e| e.to_string()),
                        ),
                    ),
                    _ => unreachable!(),
                };
                let program = ctx.script_runtime.story.as_ref().and_then(|story| {
                    ctx.script_runtime
                        .current_script
                        .as_deref()
                        .and_then(|path| story.program_for_path(path))
                });
                pending_load.slot = slot.clone();
                ctx.script_runtime.restore_pending = true;
                pending_load.request = Some(SaveLoadRequest {
                    slot,
                    history,
                    program,
                    vfs: ctx.vfs.clone(),
                    settings: ctx.user_settings.clone(),
                });
                // Earlier audio messages can now reach Bevy's playback systems
                // without waiting for disk I/O, checkpoint decoding or replay.
                break;
            }
            crate::ui::UiEffect::OpenUi {
                role,
                origin,
                arguments,
            } => {
                let target = ctx
                    .script_runtime
                    .ui_registry
                    .get(role)
                    .cloned()
                    .unwrap_or_else(|| ctx.vfs.0.resolve_path(origin.as_deref(), role));
                match super::command_runtime::evaluate_ui_at_with_arguments(
                    &target,
                    &ctx.script_runtime,
                    &ctx.vfs,
                    &ctx.user_settings,
                    Some(&ctx.textures),
                    Some(&ctx.terms),
                    ctx.models
                        .roots()
                        .map(|(name, value)| (name.to_owned(), value.clone()))
                        .collect(),
                    arguments,
                ) {
                    Ok(screen) => {
                        ctx.pending_script_commands.enqueue(ScriptCommand::Ui(
                            UiCommand::ShowScreen {
                                screen,
                                done: None,
                                push: true,
                            },
                        ));
                    }
                    Err(error) => crate::script::emit_script_diagnostic(
                        &format!("failed to open UI role `{role}`"),
                        &error.to_string(),
                    ),
                }
            }
            crate::ui::UiEffect::CloseUi { value } | crate::ui::UiEffect::CompleteUi { value } => {
                let complete = matches!(&effect, crate::ui::UiEffect::CompleteUi { .. });
                if let Some(root) = ctx.screen_state.active_root {
                    if ctx.fades.contains(root) {
                        if ctx.screen_state.closing_root.is_none() {
                            ctx.commands
                                .entity(root)
                                .try_insert(super::ui_visuals::FadeResult {
                                    value: value.clone(),
                                    complete,
                                });
                            ctx.screen_state.closing_root = Some(root);
                        }
                        continue;
                    }
                }
                super::ui_visuals::finish(
                    &mut ctx.commands,
                    &mut ctx.screen_state,
                    &mut ctx.responses,
                    value.clone(),
                    complete,
                );
            }
            crate::ui::UiEffect::Navigate(navigation) => {
                if navigation.reset != crate::script::navigation::NavigationReset::Session
                    && let Some(journal) = ctx.script_runtime.replay.as_mut()
                    && journal.destination.is_none()
                {
                    journal.complete = false;
                }
                ctx.pending_script_commands.enqueue(ScriptCommand::Runtime(
                    RuntimeCommand::Navigate {
                        request: navigation.clone(),
                        program: None,
                    },
                ));
            }
            crate::ui::UiEffect::NextDialogue => {
                if ctx.shared_state.0.story_input_blocked
                    || !ctx.shared_state.0.input_permissions.advance
                {
                    continue;
                }
                advance_dialogue(
                    &mut ctx.dialogue_state,
                    &mut ctx.animations,
                    &mut ctx.dialogue_chars,
                    &mut ctx.responses,
                );
            }
        }
        if !crate::storage::storage_ready() {
            deferred.extend(effects);
            break;
        }
    }
}

/// Swapping artwork must not discard stretching, tint, flips or visual-box
/// settings. Reconstructing ImageNode here made buttons shrink after a click.
fn restore_image_source(
    image: &mut ImageNode,
    texture: &Handle<Image>,
    atlas: &Option<TextureAtlas>,
    rect: Option<Rect>,
) {
    image.image = texture.clone();
    image.texture_atlas = atlas.clone();
    image.rect = rect;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_callback_emits_sound_before_requesting_session_restore() {
        let screen = crate::script::evaluate_ui_component_named_with_args(
            "memory://slots.ui.hks",
            r#"
                import ui.widgets.*
                canvas {
                    button { text("Load") }.onClick { sfx("ui/load"); storage.load("alice") }
                }
            "#,
            crate::script::ui_runtime::UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
            &[],
        )
        .expect("compile load button");
        let crate::ui::ScreenNode::Button(button) = &screen.children[0] else {
            panic!("expected button");
        };
        let (effects, _) = crate::script::evaluate_ui_callback_with_args(
            button.on_click.as_ref().expect("click callback"),
            &Default::default(),
            &crate::ui::UiModels::default(),
            Vec::new(),
        )
        .expect("evaluate without performing file I/O");
        assert!(matches!(effects.as_slice(), [
            crate::ui::UiEffect::PlaySfx { name, .. },
            crate::ui::UiEffect::Load { slot },
        ] if name == "ui/load" && slot == "alice"));
    }

    #[test]
    fn restoring_button_artwork_preserves_rendering_configuration() {
        let mut image = ImageNode::default().with_mode(bevy::ui::widget::NodeImageMode::Stretch);
        image.visual_box = bevy::ui::VisualBox::BorderBox;
        image.flip_x = true;
        image.color = Color::srgb(0.3, 0.5, 0.7);
        let rect = Some(Rect::new(0.0, 0.0, 64.0, 64.0));
        for _ in 0..3 {
            restore_image_source(&mut image, &Handle::default(), &None, rect);
            assert!(matches!(
                image.image_mode,
                bevy::ui::widget::NodeImageMode::Stretch
            ));
            assert_eq!(image.visual_box, bevy::ui::VisualBox::BorderBox);
            assert!(image.flip_x);
            assert_eq!(image.color, Color::srgb(0.3, 0.5, 0.7));
            assert_eq!(image.rect, rect);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn abort_runtime_waiters(
    commands: &mut Commands,
    waits: &mut PendingWaits,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    screen_state: &mut ScreenUiState,
    pending_script_commands: &mut PendingScriptCommands,
    pending_characters: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
    choice_ui_roots: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    clear_choice_ui(commands, choice_ui_roots);
    choice_state.options.clear();
    choice_state.waiting.take();
    screen_state.waiting.take();
    dialogue_state.waiting.take();
    waits.items.clear();
    pending_script_commands.clear();
    pending_characters.items.clear();
    animations.waits.clear();
    finish_all_voices(commands, animations, voice_state);
}
