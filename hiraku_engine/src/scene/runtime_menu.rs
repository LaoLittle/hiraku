use super::*;

#[derive(Component)]
pub struct PauseMenuRoot;

#[derive(Component)]
pub struct RuntimeMenuButton {
    pub action: RuntimeMenuButtonAction,
    /// Modal screen which owns this action, or `None` for the built-in pause UI.
    pub screen_root: Option<Entity>,
}

#[derive(Clone)]
pub enum RuntimeMenuButtonAction {
    Save(String),
    Load(String),
    OpenPauseMenu,
    OpenUi(String),
    CloseUi,
    SetHistoryVisible(bool),
    Resume,
    ReturnToTitle,
    AdvanceDialogue,
}

#[derive(Resource, Default)]
pub struct RuntimeMenuState {
    pub pause_root: Option<Entity>,
    pub pause_open: bool,
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
    pub ui_fonts: Res<'w, UiFonts>,
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
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
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

pub(super) fn parse_ui_action_route(route: &str) -> Option<RuntimeMenuButtonAction> {
    let segments = route.split('.').collect::<Vec<_>>();
    let action = match segments.as_slice() {
        ["ui", "open", "menu"] => RuntimeMenuButtonAction::OpenPauseMenu,
        ["ui", "open", "history"] => RuntimeMenuButtonAction::SetHistoryVisible(true),
        ["ui", "close", "history"] => RuntimeMenuButtonAction::SetHistoryVisible(false),
        ["ui", "open", role] if !role.is_empty() => {
            RuntimeMenuButtonAction::OpenUi((*role).to_string())
        }
        ["ui", "close"] => RuntimeMenuButtonAction::CloseUi,
        ["ui", "close", "menu"] => RuntimeMenuButtonAction::Resume,
        ["storage", "save", slot] if !slot.is_empty() => {
            RuntimeMenuButtonAction::Save((*slot).to_string())
        }
        ["storage", "load", slot] if !slot.is_empty() => {
            RuntimeMenuButtonAction::Load((*slot).to_string())
        }
        ["story", "next"] => RuntimeMenuButtonAction::AdvanceDialogue,
        ["app", "returnToTitle"] => RuntimeMenuButtonAction::ReturnToTitle,
        _ => {
            warn!("unknown UI action route `{route}`");
            return None;
        }
    };
    Some(action)
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
                    &mut ctx.active_batches,
                    &mut ctx.pending_characters,
                    &mut ctx.animations,
                    &mut ctx.voice_state,
                    &ctx.choice_ui_roots,
                );
                close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
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
            RuntimeMenuButtonAction::OpenPauseMenu => {
                if ctx.runtime_menu.pause_root.is_none() {
                    ctx.runtime_menu.pause_root = Some(spawn_pause_menu(
                        &mut ctx.commands,
                        &ctx.ui_fonts,
                        &ctx.ui_style,
                    ));
                }
                ctx.runtime_menu.pause_open = true;
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
                        ctx.pending_script_commands
                            .enqueue(ScriptCommand::ShowScreen { screen, done: None });
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
            RuntimeMenuButtonAction::Resume => {
                close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
            }
            RuntimeMenuButtonAction::ReturnToTitle => {
                let startup_script = ctx.frontend.startup_script.clone();
                abort_runtime_waiters(
                    &mut ctx.commands,
                    &mut ctx.waits,
                    &mut ctx.dialogue_state,
                    &mut ctx.choice_state,
                    &mut ctx.screen_state,
                    &mut ctx.pending_script_commands,
                    &mut ctx.active_batches,
                    &mut ctx.pending_characters,
                    &mut ctx.animations,
                    &mut ctx.voice_state,
                    &ctx.choice_ui_roots,
                );
                close_pause_menu(&mut ctx.commands, &mut ctx.runtime_menu);
                ctx.dialogue_history.entries.clear();
                ctx.dialogue_history.visible = false;
                clear_screen_ui(&mut ctx.commands, &mut ctx.screen_state);
                clear_overlay_ui(&mut ctx.commands, &mut ctx.overlay_state);
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
                    ScriptBootstrap::new(startup_script),
                    SceneSnapshot::default(),
                );
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

fn spawn_pause_menu(commands: &mut Commands, ui_fonts: &UiFonts, ui_style: &UiStyle) -> Entity {
    let root = commands
        .spawn((
            PauseMenuRoot,
            GlobalZIndex(SCREEN_MODAL_ACTIVE_Z + 10),
            Node {
                position_type: PositionType::Absolute,
                left: px(0.0),
                right: px(0.0),
                top: px(0.0),
                bottom: px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.35)),
            Visibility::Inherited,
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Node {
                    width: percent(72.0),
                    max_width: px(640.0),
                    padding: UiRect::all(px(32.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18.0),
                    border: UiRect::all(px(2.0)),
                    border_radius: BorderRadius::all(px(24.0)),
                    ..default()
                },
                BackgroundColor(ui_style.choice_panel_bg),
                BorderColor::all(ui_style.choice_button_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Game Menu"),
                    ui_text_font(ui_fonts, 42.0),
                    TextColor(ui_style.speaker_color),
                ));
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Return",
                    RuntimeMenuButtonAction::Resume,
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Quick Save",
                    RuntimeMenuButtonAction::Save("quick".into()),
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Quick Load",
                    RuntimeMenuButtonAction::Load("quick".into()),
                );
                spawn_runtime_menu_button(
                    panel,
                    ui_fonts,
                    ui_style,
                    "Main Menu",
                    RuntimeMenuButtonAction::ReturnToTitle,
                );
            });
    });

    root
}

fn spawn_runtime_menu_button(
    parent: &mut bevy::ecs::hierarchy::ChildSpawnerCommands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    action: RuntimeMenuButtonAction,
) {
    parent
        .spawn((
            RuntimeMenuButton {
                action,
                screen_root: None,
            },
            Button,
            Node {
                width: percent(100.0),
                min_height: px(68.0),
                border: UiRect::all(px(2.0)),
                padding: UiRect::axes(px(24.0), px(14.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(px(14.0)),
                ..default()
            },
            BackgroundColor(ui_style.choice_button_bg),
            BorderColor::all(ui_style.choice_button_border),
        ))
        .with_children(|button| {
            button.spawn((
                Pickable::IGNORE,
                Text::new(text),
                ui_text_font(ui_fonts, ui_style.quick_button_size.max(30.0)),
                TextColor(ui_style.choice_text_color),
            ));
        });
}

fn close_pause_menu(commands: &mut Commands, runtime_menu: &mut RuntimeMenuState) {
    runtime_menu.pause_open = false;
    if let Some(root) = runtime_menu.pause_root.take() {
        commands.entity(root).try_despawn();
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
    active_batches: &mut ActiveScriptBatches,
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
    active_batches.items.clear();
    pending_characters.items.clear();
    animations.waits.clear();
    finish_all_voices(commands, animations, voice_state);
}
