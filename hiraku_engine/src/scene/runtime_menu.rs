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
