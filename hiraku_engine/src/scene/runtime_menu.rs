use super::*;

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
    pub clicks: MessageReader<'w, 's, Pointer<Click>>,
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
            && Some(root) != screen_state.active_root
            && !overlay_state.roots.values().any(|overlay| *overlay == root)
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

fn start_frontend_session(
    commands: &mut Commands,
    asset_server: &AssetServer,
    vfs: &VfsResource,
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
    script_runtime: &mut ScriptRuntimeState,
    bootstrap: ScriptBootstrap,
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

    if let Err(error) = start_story_runtime(vfs, script_runtime, bootstrap, user_settings) {
        frontend.notice = Some(format!("Failed to start HKS runtime: {error}"));
        frontend.runtime_started = false;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(mut ctx: RuntimeMenuContext) {
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
            && Some(root) != ctx.screen_state.active_root
            && !ctx
                .overlay_state
                .roots
                .values()
                .any(|overlay| *overlay == root)
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
            && ctx.screen_state.active_root.map_or_else(
                || {
                    ctx.overlay_state
                        .roots
                        .values()
                        .any(|root| *root == request.root)
                },
                |root| root == request.root,
            )
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
            && Some(root) != ctx.screen_state.active_root
            && !ctx
                .overlay_state
                .roots
                .values()
                .any(|overlay| *overlay == root)
        {
            continue;
        }
        let mut globals = ctx
            .script_runtime
            .story
            .as_ref()
            .map(|story| story.globals().clone())
            .unwrap_or_default();
        if let Some(root) = root
            && let Ok(local) = ctx.local_states.get(root)
        {
            globals.extend(local.0.clone());
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
        for effect in effects {
            match &effect {
                crate::ui::UiEffect::SetPreference(change) => {
                    ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                        SettingsCommand::Preference(change.clone()),
                    ));
                }
                crate::ui::UiEffect::SetAutoDialogue(enabled) => {
                    ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                        SettingsCommand::AutoDialogue(*enabled),
                    ));
                }
                crate::ui::UiEffect::SetVolume { channel, value } => {
                    ctx.pending_script_commands.enqueue(ScriptCommand::Settings(
                        SettingsCommand::Set { name: channel.clone(), value: *value },
                    ));
                }
                crate::ui::UiEffect::PlaySfx { .. } => {
                    ctx.effects
                        .write(super::screen_ui::UiEffectMessage(effect.clone()));
                }
                crate::ui::UiEffect::Save { slot } => {
                    if let Err(error) =
                        save_runtime_slot(slot, &ctx.script_runtime, &ctx.shared_state)
                    {
                        warn!("failed to save slot `{slot}`: {error}");
                    }
                }
                crate::ui::UiEffect::Load { slot } => {
                    let save_data = match load_save_data(slot) {
                        Ok(save_data) => save_data,
                        Err(error) => {
                            warn!("failed to load slot `{slot}`: {error}");
                            ctx.frontend.notice =
                                Some(format!("Failed to load slot {slot}: {error}"));
                            continue;
                        }
                    };
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
                    ctx.dialogue_history.entries.clear();
                    clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
                    start_frontend_session(
                        &mut ctx.commands,
                        &ctx.asset_server,
                        &ctx.vfs,
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
                        &mut ctx.script_runtime,
                        ScriptBootstrap::from_save(&save_data),
                        save_data.scene.clone(),
                    );
                    if let Some(error) = ctx.frontend.notice.as_deref() {
                        crate::script::emit_script_diagnostic(
                            &format!("failed to restore slot `{slot}`"),
                            &error.to_string(),
                        );
                    } else {
                        info!("loaded save slot `{slot}`");
                    }
                }
                crate::ui::UiEffect::OpenUi { role } => {
                    let Some(target) = ctx.script_runtime.ui_registry.get(role).cloned() else {
                        warn!("UI action route references unregistered role `{role}`");
                        continue;
                    };
                    match evaluate_ui_at(
                        &target,
                        &ctx.script_runtime,
                        &ctx.vfs,
                        &ctx.user_settings,
                        Some(&ctx.textures),
                        Some(&ctx.terms),
                    ) {
                        Ok(screen) => {
                            ctx.pending_script_commands.enqueue(ScriptCommand::Ui(
                                UiCommand::ShowScreen { screen, done: None },
                            ));
                        }
                        Err(error) => crate::script::emit_script_diagnostic(
                            &format!("failed to open UI role `{role}`"),
                            &error.to_string(),
                        ),
                    }
                }
                crate::ui::UiEffect::CloseUi { value } => {
                    if let Some(request) = ctx.screen_state.waiting.take() {
                        ctx.responses.write(ScriptResponseMessage {
                            request,
                            response: ScriptResponse::UiResult(value.clone()),
                        });
                    }
                    clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
                }
                crate::ui::UiEffect::Navigate(navigation) => {
                    ctx.pending_script_commands.enqueue(ScriptCommand::Runtime(
                        RuntimeCommand::Navigate(navigation.clone()),
                    ));
                }
                crate::ui::UiEffect::NextDialogue => {
                    advance_dialogue(
                        &mut ctx.dialogue_state,
                        &mut ctx.animations,
                        &mut ctx.dialogue_chars,
                        &mut ctx.responses,
                    );
                }
            }
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
