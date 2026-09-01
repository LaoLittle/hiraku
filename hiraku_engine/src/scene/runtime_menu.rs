use super::*;

/// Marker for an application-owned modal layered above Hiraku's script UI.
#[derive(Component)]
pub struct PauseMenuRoot;

#[derive(Component)]
pub struct RuntimeMenuButton {
    pub action: RuntimeMenuButtonAction,
    /// Modal screen which owns this action.
    pub screen_root: Option<Entity>,
}

#[derive(Clone)]
pub enum RuntimeMenuButtonAction {
    Save(String),
    Load(String),
    OpenUi(String),
    CloseUi,
    SetHistoryVisible(bool),
    Navigate(crate::script::navigation::NavigationRequest),
    AdvanceDialogue,
}

#[derive(Resource, Default)]
pub struct RuntimeMenuState {
    /// Pointer clicks claimed by UI actions during this update. Keeping the
    /// pointer identity makes consumption survive deferred modal despawning.
    pub consumed_pointer_clicks: HashMap<PointerId, usize>,
}

#[derive(SystemParam)]
pub struct RuntimeMenuContext<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub asset_server: Res<'w, AssetServer>,
    pub textures: Res<'w, TextureCatalog>,
    pub terms: Res<'w, TermCatalog>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: ResMut<'w, SceneSharedState>,
    pub script_runtime: ResMut<'w, ScriptRuntimeState>,
    pub frontend: ResMut<'w, FrontendState>,
    pub user_settings: Res<'w, UserSettings>,
    pub ui_style: Res<'w, UiStyle>,
    pub runtime_menu: ResMut<'w, RuntimeMenuState>,
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

    if let Err(error) = start_hks_runtime(vfs, script_runtime, bootstrap, user_settings) {
        frontend.notice = Some(format!("Failed to start HKS runtime: {error}"));
        frontend.runtime_started = false;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_runtime_menu_buttons(mut ctx: RuntimeMenuContext) {
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
        *ctx.runtime_menu
            .consumed_pointer_clicks
            .entry(click.pointer_id)
            .or_default() += 1;
        // The action may replace or cover this node before picking emits a
        // later interaction transition. Restore its release visual now.
        if let Some(image_button) = image_button {
            let mut image = ImageNode::new(image_button.normal_texture.clone());
            image.texture_atlas = image_button.normal_atlas.clone();
            image.rect = image_button.normal_rect;
            ctx.commands.entity(button_entity).insert((
                BackgroundColor(Color::NONE),
                UiTransform::IDENTITY,
                image,
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
                let mut image = ImageNode::new(texture.clone());
                image.texture_atlas = screen_button.normal_atlas.clone();
                image.rect = screen_button.normal_rect;
                ctx.commands.entity(button_entity).insert(image);
            }
        } else {
            ctx.commands
                .entity(button_entity)
                .insert(BackgroundColor(ctx.ui_style.choice_button_bg));
        }
        let action = button.action.clone();
        match &action {
            RuntimeMenuButtonAction::Save(slot) => {
                if let Err(error) = save_runtime_slot(slot, &ctx.script_runtime, &ctx.shared_state)
                {
                    warn!("failed to save slot `{slot}`: {error}");
                }
            }
            RuntimeMenuButtonAction::Load(slot) => {
                let save_data = match load_save_data(slot) {
                    Ok(save_data) => save_data,
                    Err(error) => {
                        warn!("failed to load slot `{slot}`: {error}");
                        ctx.frontend.notice = Some(format!("Failed to load slot {slot}: {error}"));
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
                ctx.dialogue_history.visible = false;
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
                    warn!("failed to restore slot `{slot}`: {error}");
                } else {
                    info!("loaded save slot `{slot}`");
                }
            }
            RuntimeMenuButtonAction::OpenUi(role) => {
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
                    Err(error) => warn!("failed to open UI role `{role}`: {error}"),
                }
            }
            RuntimeMenuButtonAction::CloseUi => {
                clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
            }
            RuntimeMenuButtonAction::SetHistoryVisible(visible) => {
                ctx.dialogue_history.visible = *visible;
            }
            RuntimeMenuButtonAction::Navigate(navigation) => {
                ctx.pending_script_commands.enqueue(ScriptCommand::Runtime(
                    RuntimeCommand::Navigate(navigation.clone()),
                ));
            }
            RuntimeMenuButtonAction::AdvanceDialogue => {
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
