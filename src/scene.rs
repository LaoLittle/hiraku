use std::{collections::{HashMap, HashSet, VecDeque}, sync::mpsc};

use bevy::{
    audio::{AudioSink, AudioSinkPlayback, Volume},
    ecs::system::SystemParam,
    log::warn,
    prelude::*,
};

use crate::{
    character::{CharacterCatalog, CharacterDefinition, load_character_catalog},
    effect::{CustomScreenEffectMaterial, CustomScreenEffectPlayer},
    script::{
        BatchSubmissionItem, BatchSubmitMode, CharacterEase, InlineDialogueControlResource,
        ResolvedCharacterKeyframe, ScriptBootstrap, ScriptCommand, ScriptInbox, ScriptResponse,
        spawn_script_runtime,
    },
    storage::{
        SaveSlotSummary, StorageError, UserSettings, list_save_slots, load_save_data,
        read_user_settings, write_user_settings,
    },
    state::{
        AudioSnapshot, ChoiceOption, DialogueSnapshot, ImageLayerSnapshot, SceneSharedState,
        SceneSnapshot, SpriteSnapshot, StoredValue, TextEffectSnapshot, UiStylePatch,
    },
    transition::{RuleTransitionMaterial, RuleTransitionMesh, RuleTransitionPlayer},
    ui::{
        ButtonNode, ContainerNode, ScreenNode, ScreenSpec, ScreenUiButton, ScreenUiNode,
        ScreenUiRoot, ScreenUiState, SpacerNode, TextNode,
    },
    vfs::VfsResource,
};

const STAGE_Z_BACKGROUND: f32 = 0.0;
const STAGE_Z_SPRITE: f32 = 10.0;
const STAGE_Z_OVERLAY: f32 = 30.0;

const BUTTON_NORMAL: Color = Color::srgb(0.13, 0.15, 0.19);
const BUTTON_HOVERED: Color = Color::srgb(0.22, 0.26, 0.32);
const BUTTON_PRESSED: Color = Color::srgb(0.88, 0.74, 0.44);
const FRONTEND_BG: Color = Color::srgb(0.06, 0.08, 0.11);
const FRONTEND_PANEL: Color = Color::srgb(0.12, 0.09, 0.08);
const FRONTEND_PANEL_ALT: Color = Color::srgb(0.17, 0.13, 0.11);
const FRONTEND_BUTTON: Color = Color::srgb(0.24, 0.18, 0.13);
const FRONTEND_BUTTON_HOVERED: Color = Color::srgb(0.34, 0.24, 0.16);
const FRONTEND_BUTTON_PRESSED: Color = Color::srgb(0.84, 0.61, 0.30);

#[derive(Resource, Clone)]
pub struct UiFonts {
    pub regular: Handle<Font>,
}

#[derive(Resource, Clone)]
pub struct UiStyle {
    pub dialogue_bg: Color,
    pub dialogue_border: Color,
    pub speaker_color: Color,
    pub line_color: Color,
    pub hint_color: Color,
    pub choice_panel_bg: Color,
    pub choice_prompt_color: Color,
    pub choice_button_bg: Color,
    pub choice_button_hovered: Color,
    pub choice_button_pressed: Color,
    pub choice_button_border: Color,
    pub choice_text_color: Color,
}

impl Default for UiStyle {
    fn default() -> Self {
        Self {
            dialogue_bg: Color::BLACK.with_alpha(0.82),
            dialogue_border: Color::WHITE.with_alpha(0.14),
            speaker_color: Color::srgb(1.0, 0.9, 0.72),
            line_color: Color::WHITE,
            hint_color: Color::WHITE.with_alpha(0.55),
            choice_panel_bg: Color::BLACK.with_alpha(0.72),
            choice_prompt_color: Color::WHITE.with_alpha(0.82),
            choice_button_bg: BUTTON_NORMAL,
            choice_button_hovered: BUTTON_HOVERED,
            choice_button_pressed: BUTTON_PRESSED,
            choice_button_border: Color::WHITE.with_alpha(0.16),
            choice_text_color: Color::WHITE,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrontendScreen {
    #[default]
    Title,
    Load,
    Settings,
    InGame,
}

#[derive(Resource, Default)]
pub struct FrontendState {
    pub startup_script: String,
    pub gallery_script: String,
    pub root: Option<Entity>,
    pub screen: FrontendScreen,
    pub saves: Vec<SaveSlotSummary>,
    pub notice: Option<String>,
    pub runtime_started: bool,
    pub dirty: bool,
}

#[derive(Resource, Default)]
pub struct StageState {
    pub background: Option<Entity>,
    pub overlay: Option<Entity>,
    pub transition: Option<Entity>,
    pub screen_effect: Option<Entity>,
    pub sprites: HashMap<String, Entity>,
    pub character_positions: HashMap<String, Vec2>,
    pub bgm: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct DialogueState {
    pub waiting: Option<PendingDialogueAdvance>,
    pub span_entities: Vec<Entity>,
    pub reveal: Option<DialogueRevealState>,
    pub effect: DialogueTextEffect,
}

pub struct PendingDialogueAdvance {
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

pub struct DialogueRevealState {
    pub spans: Vec<Entity>,
    pub next_index: usize,
    pub accumulator: f32,
    pub interval: f32,
    pub fade_seconds: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Clone)]
pub struct DialogueTextEffect {
    pub mode: DialogueTextEffectMode,
    pub cps: f32,
    pub fade_seconds: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogueTextEffectMode {
    Instant,
    TypewriterFade,
}

impl Default for DialogueTextEffect {
    fn default() -> Self {
        Self {
            mode: DialogueTextEffectMode::TypewriterFade,
            cps: 30.0,
            fade_seconds: 0.12,
        }
    }
}

#[derive(Component)]
pub struct DialogueCharSpan {
    pub target_alpha: f32,
    pub age: f32,
    pub revealed: bool,
}

#[derive(Resource, Default)]
pub struct ChoiceState {
    pub waiting: Option<mpsc::Sender<ScriptResponse>>,
    pub options: Vec<ChoiceOption>,
}

#[derive(Resource, Default)]
pub struct PendingWaits {
    pub items: Vec<PendingWait>,
}

#[derive(Resource, Default)]
pub struct CameraShakeState {
    pub active: Option<CameraShake>,
}

#[derive(Resource, Default)]
pub struct AnimationState {
    pub completed: HashSet<String>,
    pub waits: Vec<PendingAnimationWait>,
}

#[derive(Resource, Default)]
pub struct PendingAnimationCancels {
    pub ids: Vec<String>,
}

#[derive(Resource, Default)]
pub struct VoiceState {
    pub active: Option<ActiveVoice>,
}

#[derive(Resource, Default)]
pub struct PendingCharacterShows {
    pub items: Vec<PendingCharacterShow>,
}

#[derive(Resource, Default)]
pub struct PendingScriptCommands {
    pub items: VecDeque<ScriptCommand>,
}

#[derive(Resource, Default)]
pub struct ActiveScriptBatches {
    pub items: Vec<ActiveScriptBatch>,
}

pub struct ActiveScriptBatch {
    pub remaining: VecDeque<BatchSubmissionItem>,
    pub current_handle: String,
}

pub struct PendingWait {
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: mpsc::Sender<ScriptResponse>,
}

pub struct PendingAnimationWait {
    pub ids: Vec<String>,
    pub done: mpsc::Sender<ScriptResponse>,
}

pub struct CameraShake {
    pub timer: Timer,
    pub amplitude: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

pub struct ActiveVoice {
    pub entity: Entity,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

pub struct PendingCharacterShow {
    pub actor_id: String,
    pub entity_ids: Vec<String>,
    pub entities: Vec<Entity>,
    pub handles: Vec<Handle<Image>>,
    pub fade: Option<std::time::Duration>,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
pub struct CharacterJumpEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub height: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
pub struct CharacterShakeEffect {
    pub origin: Vec3,
    pub timer: Timer,
    pub amplitude: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
pub struct CharacterTimelineEffect {
    pub origin: Vec3,
    pub actor_id: String,
    pub actor_origin: Vec2,
    pub keyframes: Vec<ResolvedCharacterKeyframe>,
    pub elapsed: f32,
    pub duration: f32,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Clone, Copy)]
pub enum CharacterMotionKind {
    Jump { height: f32 },
    Shake { amplitude: f32 },
}


#[derive(Component)]
pub struct OverlayMarker;

#[derive(Component)]
pub struct SpeakerText;

#[derive(Component)]
pub struct LineText;

#[derive(Component)]
pub struct HintText;

#[derive(Component)]
pub struct DialogueRoot;

#[derive(Component)]
pub struct ChoiceUi;

#[derive(Component)]
pub struct ChoiceButton {
    pub index: usize,
}

#[derive(Component)]
pub struct FrontendRoot;

#[derive(Component)]
pub struct FrontendButton {
    pub action: FrontendAction,
}

#[derive(Clone)]
pub enum FrontendAction {
    StartNewGame,
    StartCharacterGallery,
    OpenLoad,
    OpenSettings,
    BackToTitle,
    LoadSlot(String),
    AdjustBgm(f32),
    AdjustVoice(f32),
    AdjustSfx(f32),
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component, Clone)]
pub struct SpriteActor {
    pub id: String,
    pub path: String,
}

#[derive(Component, Clone)]
pub struct BackgroundLayer {
    pub path: String,
}

#[derive(Component, Clone)]
pub struct BgmChannel {
    pub path: String,
}

#[derive(Component)]
pub struct BgmFade {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
}

#[derive(Component)]
#[expect(dead_code, reason = "voice metadata is kept for future UI/debug hooks")]
pub struct VoiceChannel {
    pub path: String,
}

#[derive(Component)]
pub struct VisualTween {
    pub from_alpha: Option<f32>,
    pub to_alpha: Option<f32>,
    pub from_translation: Option<Vec3>,
    pub to_translation: Option<Vec3>,
    pub from_scale: Option<Vec3>,
    pub to_scale: Option<Vec3>,
    pub timer: Timer,
    pub animation_id: Option<String>,
    pub done: Option<mpsc::Sender<ScriptResponse>>,
    pub despawn_on_finish: bool,
}

#[derive(SystemParam)]
pub struct SceneCommandContext<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub asset_server: Res<'w, AssetServer>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: Res<'w, SceneSharedState>,
    pub characters: Res<'w, CharacterCatalog>,
    pub user_settings: ResMut<'w, UserSettings>,
    pub ui_fonts: Res<'w, UiFonts>,
    pub ui_style: ResMut<'w, UiStyle>,
    pub frontend: ResMut<'w, FrontendState>,
    pub inbox: Option<Res<'w, ScriptInbox>>,
    pub stage: ResMut<'w, StageState>,
    pub waits: ResMut<'w, PendingWaits>,
    pub pending_cancels: ResMut<'w, PendingAnimationCancels>,
    pub pending_script_commands: ResMut<'w, PendingScriptCommands>,
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub animations: ResMut<'w, AnimationState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub transition_mesh: Res<'w, RuleTransitionMesh>,
    pub custom_effect_materials: ResMut<'w, Assets<CustomScreenEffectMaterial>>,
    pub rule_materials: ResMut<'w, Assets<RuleTransitionMaterial>>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root: Query<'w, 's, &'static mut Visibility, With<DialogueRoot>>,
    pub dialogue_background: Query<'w, 's, &'static mut BackgroundColor, With<DialogueRoot>>,
    pub dialogue_border: Query<'w, 's, &'static mut BorderColor, With<DialogueRoot>>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub line_text_entity: Query<'w, 's, Entity, (With<LineText>, Without<SpeakerText>)>,
    pub speaker_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<SpeakerText>, Without<LineText>, Without<HintText>),
    >,
    pub line_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<LineText>, Without<SpeakerText>, Without<HintText>),
    >,
    pub hint_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
}

pub fn setup_stage(
    mut commands: Commands,
    ui_fonts: Res<UiFonts>,
    ui_style: Res<UiStyle>,
    mut meshes: ResMut<Assets<Mesh>>,
    shared_state: Res<SceneSharedState>,
) {
    commands.spawn((Camera2d, MainCamera));
    commands.insert_resource(RuleTransitionMesh(meshes.add(Rectangle::default())));

    let overlay = commands
        .spawn((
            OverlayMarker,
            Sprite::from_color(Color::BLACK.with_alpha(0.0), Vec2::new(6000.0, 6000.0)),
            Transform::from_xyz(0.0, 0.0, STAGE_Z_OVERLAY),
        ))
        .id();

    commands.insert_resource(StageState {
        overlay: Some(overlay),
        ..default()
    });
    commands.insert_resource(DialogueState::default());
    commands.insert_resource(ChoiceState::default());
    commands.insert_resource(PendingWaits::default());
    commands.insert_resource(CameraShakeState::default());
    commands.insert_resource(AnimationState::default());
    commands.insert_resource(PendingAnimationCancels::default());
    commands.insert_resource(PendingScriptCommands::default());
    commands.insert_resource(ActiveScriptBatches::default());
    commands.insert_resource(VoiceState::default());
    commands.insert_resource(PendingCharacterShows::default());
    commands.insert_resource(ScreenUiState::default());

    let dialogue_root = commands
        .spawn((
            DialogueRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(24.0),
                right: px(24.0),
                bottom: px(24.0),
                min_height: px(180.0),
                border: UiRect::all(px(1.0)),
                padding: UiRect::axes(px(28.0), px(22.0)),
                border_radius: BorderRadius::all(px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(12.0),
                ..default()
            },
            BackgroundColor(ui_style.dialogue_bg),
            BorderColor::all(ui_style.dialogue_border),
            Visibility::Hidden,
        ))
        .id();

    commands.entity(dialogue_root).with_children(|parent| {
        parent.spawn((
            SpeakerText,
            Text::new(""),
            ui_text_font(&ui_fonts, 28.0),
            TextColor(ui_style.speaker_color),
        ));
        parent.spawn((
            LineText,
            Text::new(""),
            ui_text_font(&ui_fonts, 34.0),
            TextColor(ui_style.line_color),
        ));
        parent.spawn((
            HintText,
            Text::new("click / enter / space"),
            ui_text_font(&ui_fonts, 18.0),
            TextColor(ui_style.hint_color),
        ));
    });

    let mut snapshot = SceneSnapshot::default();
    snapshot.text_effect = text_effect_snapshot(&DialogueTextEffect::default());
    *shared_state.0.lock().unwrap() = snapshot;
}

pub fn setup_frontend(mut commands: Commands, asset_server: Res<AssetServer>, vfs: Res<VfsResource>) {
    let startup_script = match vfs.0.load_startup_script_path() {
        Ok(startup_script) => startup_script,
        Err(err) => {
            warn!("failed to resolve startup script: {err}");
            String::new()
        }
    };

    let user_settings = match read_user_settings() {
        Ok(settings) => settings,
        Err(err) => {
            warn!("failed to read user settings: {err}");
            UserSettings::default()
        }
    };

    let ui_font_path = match vfs.0.load_ui_font_path() {
        Ok(path) => path,
        Err(err) => {
            warn!("failed to resolve ui font path: {err}");
            None
        }
    };

    let character_catalog = match load_character_catalog(&vfs.0) {
        Ok(catalog) => catalog,
        Err(err) => {
            warn!("failed to load character catalog: {err}");
            CharacterCatalog::default()
        }
    };

    let mut frontend = FrontendState {
        startup_script,
        gallery_script: vfs
            .0
            .resolve_path(Some(vfs.0.settings_path()), "scripts/character_gallery.rhai"),
        screen: FrontendScreen::InGame,
        runtime_started: true,
        dirty: false,
        ..default()
    };

    if frontend.startup_script.is_empty() {
        frontend.notice = Some("startup.rhai not found. Fix settings.toml before starting.".to_string());
    }

    commands.insert_resource(UiFonts {
        regular: ui_font_path
            .as_ref()
            .map(|path| asset_server.load(path.clone()))
            .unwrap_or_default(),
    });
    commands.insert_resource(UiStyle::default());
    commands.insert_resource(character_catalog);
    commands.insert_resource(user_settings);
    commands.insert_resource(frontend);
}

pub fn rebuild_frontend_ui(
    mut commands: Commands,
    mut frontend: ResMut<FrontendState>,
    ui_fonts: Res<UiFonts>,
    user_settings: Res<UserSettings>,
) {
    if !frontend.dirty {
        return;
    }

    if let Some(root) = frontend.root.take() {
        commands.entity(root).try_despawn();
    }

    frontend.dirty = false;
    if frontend.screen == FrontendScreen::InGame {
        return;
    }

    let root = commands
        .spawn((
            FrontendRoot,
            Node {
                width: percent(100.0),
                height: percent(100.0),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Stretch,
                padding: UiRect::axes(px(36.0), px(30.0)),
                column_gap: px(28.0),
                ..default()
            },
            BackgroundColor(FRONTEND_BG.with_alpha(0.96)),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Node {
                    width: percent(42.0),
                    height: percent(100.0),
                    padding: UiRect::all(px(28.0)),
                    border_radius: BorderRadius::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                },
                BackgroundColor(FRONTEND_PANEL.with_alpha(0.92)),
                BorderColor::all(Color::WHITE.with_alpha(0.08)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("hiraku"),
                    ui_text_font(&ui_fonts, 72.0),
                    TextColor(Color::srgb(0.95, 0.82, 0.67)),
                ));
                panel.spawn((
                    Text::new("Bevy + Rhai visual novel runtime"),
                    ui_text_font(&ui_fonts, 24.0),
                    TextColor(Color::WHITE.with_alpha(0.72)),
                ));
                panel.spawn((
                    Text::new("Title menu now owns startup, settings, and save selection so scripts can stay focused on scene flow."),
                    ui_text_font(&ui_fonts, 22.0),
                    TextColor(Color::WHITE.with_alpha(0.56)),
                ));
            });

        parent
            .spawn((
                Node {
                    width: percent(58.0),
                    height: percent(100.0),
                    padding: UiRect::all(px(28.0)),
                    border_radius: BorderRadius::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18.0),
                    ..default()
                },
                BackgroundColor(FRONTEND_PANEL_ALT.with_alpha(0.96)),
                BorderColor::all(Color::WHITE.with_alpha(0.10)),
            ))
            .with_children(|panel| {
                let title = match frontend.screen {
                    FrontendScreen::Title => "Start",
                    FrontendScreen::Load => "Load",
                    FrontendScreen::Settings => "Settings",
                    FrontendScreen::InGame => unreachable!(),
                };

                panel.spawn((
                    Text::new(title),
                    ui_text_font(&ui_fonts, 46.0),
                    TextColor(Color::WHITE),
                ));

                if let Some(notice) = frontend.notice.as_ref() {
                    panel.spawn((
                        Text::new(notice.clone()),
                        ui_text_font(&ui_fonts, 20.0),
                        TextColor(Color::srgb(0.98, 0.78, 0.58)),
                    ));
                }

                match frontend.screen {
                    FrontendScreen::Title => {
                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::StartNewGame,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("New Game"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::OpenLoad,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new(format!("Load Game ({})", frontend.saves.len())),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::StartCharacterGallery,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Character Gallery"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::OpenSettings,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Settings"),
                                    ui_text_font(&ui_fonts, 28.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::Load => {
                        if frontend.saves.is_empty() {
                            panel.spawn((
                                Text::new("No save files found in saves/*.toml"),
                                ui_text_font(&ui_fonts, 24.0),
                                TextColor(Color::WHITE.with_alpha(0.62)),
                            ));
                        }

                        for slot in &frontend.saves {
                            let route = slot.route.clone().unwrap_or_else(|| "unknown route".to_string());
                            let background = slot
                                .background
                                .as_ref()
                                .map(|path| shorten_path(path))
                                .unwrap_or_else(|| "no background snapshot".to_string());
                            panel
                                .spawn((
                                    FrontendButton {
                                        action: FrontendAction::LoadSlot(slot.slot.clone()),
                                    },
                                    Button,
                                    Node {
                                        width: percent(100.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(16.0)),
                                        justify_content: JustifyContent::FlexStart,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(16.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new(format!(
                                            "Slot {}\n{}\n{}\n{}",
                                            slot.slot,
                                            route,
                                            shorten_path(&slot.resume_script),
                                            background
                                        )),
                                        ui_text_font(&ui_fonts, 22.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                        }

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::BackToTitle,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Back"),
                                    ui_text_font(&ui_fonts, 26.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::Settings => {
                        panel.spawn((
                            Text::new(format!("BGM {:.0}%", user_settings.bgm_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustBgm(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("BGM -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustBgm(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("BGM +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel.spawn((
                            Text::new(format!("Voice {:.0}%", user_settings.voice_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustVoice(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("Voice -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustVoice(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("Voice +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel.spawn((
                            Text::new(format!("SFX {:.0}%", user_settings.sfx_volume * 100.0)),
                            ui_text_font(&ui_fonts, 28.0),
                            TextColor(Color::WHITE),
                        ));
                        panel
                            .spawn((
                                Node {
                                    width: percent(100.0),
                                    column_gap: px(12.0),
                                    ..default()
                                },
                            ))
                            .with_children(|row| {
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustSfx(-0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("SFX -"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                                row.spawn((
                                    FrontendButton {
                                        action: FrontendAction::AdjustSfx(0.1),
                                    },
                                    Button,
                                    Node {
                                        width: percent(50.0),
                                        border: UiRect::all(px(1.0)),
                                        padding: UiRect::axes(px(18.0), px(14.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        border_radius: BorderRadius::all(px(14.0)),
                                        ..default()
                                    },
                                    BackgroundColor(FRONTEND_BUTTON),
                                    BorderColor::all(Color::WHITE.with_alpha(0.14)),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        Text::new("SFX +"),
                                        ui_text_font(&ui_fonts, 24.0),
                                        TextColor(Color::WHITE),
                                    ));
                                });
                            });

                        panel
                            .spawn((
                                FrontendButton {
                                    action: FrontendAction::BackToTitle,
                                },
                                Button,
                                Node {
                                    width: percent(100.0),
                                    border: UiRect::all(px(1.0)),
                                    padding: UiRect::axes(px(20.0), px(18.0)),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border_radius: BorderRadius::all(px(18.0)),
                                    margin: UiRect::top(px(10.0)),
                                    ..default()
                                },
                                BackgroundColor(FRONTEND_BUTTON),
                                BorderColor::all(Color::WHITE.with_alpha(0.14)),
                            ))
                            .with_children(|button| {
                                button.spawn((
                                    Text::new("Back"),
                                    ui_text_font(&ui_fonts, 26.0),
                                    TextColor(Color::WHITE),
                                ));
                            });
                    }
                    FrontendScreen::InGame => unreachable!(),
                }
            });
    });

    frontend.root = Some(root);
}

#[allow(clippy::too_many_arguments)]
pub fn handle_frontend_buttons(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vfs: Res<VfsResource>,
    shared_state: Res<SceneSharedState>,
    mut frontend: ResMut<FrontendState>,
    mut user_settings: ResMut<UserSettings>,
    mut stage: ResMut<StageState>,
    mut dialogue_state: ResMut<DialogueState>,
    mut choice_state: ResMut<ChoiceState>,
    mut interaction_query: Query<
        (&Interaction, &mut BackgroundColor, &FrontendButton),
        Changed<Interaction>,
    >,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    mut dialogue_root: Query<&mut Visibility, With<DialogueRoot>>,
    mut speaker_text: Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    mut line_text: Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
) {
    if frontend.screen == FrontendScreen::InGame {
        return;
    }

    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                *color = FRONTEND_BUTTON_PRESSED.into();
                let action = button.action.clone();
                match action {
                    FrontendAction::StartNewGame => {
                        if frontend.runtime_started {
                            continue;
                        }
                        if frontend.startup_script.is_empty() {
                            frontend.notice = Some(
                                "No startup script is configured. Check main.hdp!settings.toml."
                                    .to_string(),
                            );
                            frontend.dirty = true;
                            continue;
                        }

                        let bootstrap = ScriptBootstrap::new(frontend.startup_script.clone());

                        start_frontend_session(
                            &mut commands,
                            &asset_server,
                            &vfs,
                            &shared_state,
                            &mut stage,
                            &mut dialogue_state,
                            &mut choice_state,
                            &choice_ui,
                            &mut dialogue_root,
                            &mut speaker_text,
                            &mut line_text,
                            &mut frontend,
                            bootstrap,
                            SceneSnapshot::default(),
                        );
                    }
                    FrontendAction::StartCharacterGallery => {
                        if frontend.runtime_started {
                            continue;
                        }
                        if !vfs.0.exists(&frontend.gallery_script) {
                            frontend.notice = Some(
                                "Character gallery script is missing from main.hdp.".to_string(),
                            );
                            frontend.dirty = true;
                            continue;
                        }
                        let mut bootstrap = ScriptBootstrap::new(frontend.gallery_script.clone());
                        bootstrap.globals.insert(
                            "route".to_string(),
                            StoredValue::String("gallery".to_string()),
                        );

                        start_frontend_session(
                            &mut commands,
                            &asset_server,
                            &vfs,
                            &shared_state,
                            &mut stage,
                            &mut dialogue_state,
                            &mut choice_state,
                            &choice_ui,
                            &mut dialogue_root,
                            &mut speaker_text,
                            &mut line_text,
                            &mut frontend,
                            bootstrap,
                            SceneSnapshot::default(),
                        );
                    }
                    FrontendAction::OpenLoad => {
                        frontend.screen = FrontendScreen::Load;
                        frontend.notice = None;
                        match list_save_slots() {
                            Ok(saves) => frontend.saves = saves,
                            Err(err) => frontend.notice = Some(format!("Failed to read save list: {err}")),
                        }
                        frontend.dirty = true;
                    }
                    FrontendAction::OpenSettings => {
                        frontend.screen = FrontendScreen::Settings;
                        frontend.notice = None;
                        frontend.dirty = true;
                    }
                    FrontendAction::BackToTitle => {
                        frontend.screen = FrontendScreen::Title;
                        frontend.notice = None;
                        frontend.dirty = true;
                    }
                    FrontendAction::LoadSlot(slot) => match load_save_data(&slot) {
                        Ok(save_data) => {
                            if frontend.runtime_started {
                                continue;
                            }
                            let snapshot = save_data.scene.clone();
                            let bootstrap = ScriptBootstrap::from_save(&save_data);
                            start_frontend_session(
                                &mut commands,
                                &asset_server,
                                &vfs,
                                &shared_state,
                                &mut stage,
                                &mut dialogue_state,
                                &mut choice_state,
                                &choice_ui,
                                &mut dialogue_root,
                                &mut speaker_text,
                                &mut line_text,
                                &mut frontend,
                                bootstrap,
                                snapshot,
                            );
                        }
                        Err(err) => {
                            frontend.notice = Some(format!("Failed to load slot {slot}: {err}"));
                            frontend.dirty = true;
                        }
                    },
                    FrontendAction::AdjustBgm(delta) => {
                        user_settings.bgm_volume = adjusted_volume(user_settings.bgm_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                    FrontendAction::AdjustVoice(delta) => {
                        user_settings.voice_volume = adjusted_volume(user_settings.voice_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                    FrontendAction::AdjustSfx(delta) => {
                        user_settings.sfx_volume = adjusted_volume(user_settings.sfx_volume, delta);
                        frontend.notice = write_user_settings(user_settings.as_ref())
                            .err()
                            .map(format_storage_error);
                        frontend.dirty = true;
                    }
                }
            }
            Interaction::Hovered => {
                *color = FRONTEND_BUTTON_HOVERED.into();
            }
            Interaction::None => {
                *color = FRONTEND_BUTTON.into();
            }
        }
    }
}

pub fn process_script_commands(ctx: SceneCommandContext) {
    let Some(inbox) = ctx.inbox else {
        return;
    };

    let mut commands = ctx.commands;
    let asset_server = ctx.asset_server;
    let vfs = ctx.vfs;
    let shared_state = ctx.shared_state;
    let characters = ctx.characters;
    let mut user_settings = ctx.user_settings;
    let ui_fonts = ctx.ui_fonts;
    let mut ui_style = ctx.ui_style;
    let mut frontend = ctx.frontend;
    let mut stage = ctx.stage;
    let mut waits = ctx.waits;
    let mut pending_cancels = ctx.pending_cancels;
    let mut pending_script_commands = ctx.pending_script_commands;
    let mut active_batches = ctx.active_batches;
    let mut dialogue_state = ctx.dialogue_state;
    let mut choice_state = ctx.choice_state;
    let mut screen_state = ctx.screen_state;
    let mut animations = ctx.animations;
    let mut voice_state = ctx.voice_state;
    let mut pending_characters = ctx.pending_characters;
    let transition_mesh = ctx.transition_mesh;
    let mut custom_effect_materials = ctx.custom_effect_materials;
    let mut rule_materials = ctx.rule_materials;
    let choice_ui_roots = ctx.choice_ui_roots;
    let mut dialogue_root = ctx.dialogue_root;
    let mut dialogue_background = ctx.dialogue_background;
    let mut dialogue_border = ctx.dialogue_border;
    let mut speaker_text = ctx.speaker_text;
    let mut line_text = ctx.line_text;
    let line_text_entity = ctx.line_text_entity;
    let mut speaker_color = ctx.speaker_color;
    let mut line_color = ctx.line_color;
    let mut hint_color = ctx.hint_color;

    let Ok(receiver) = inbox.0.lock() else {
        return;
    };

    while let Some(command) = pending_script_commands
        .items
        .pop_front()
        .or_else(|| receiver.try_recv().ok())
    {
        match command {
            ScriptCommand::Log(message) => info!("[rhai] {message}"),
            ScriptCommand::SetBackground {
                path,
                fade,
                animation_id,
                done,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }
                let image = asset_server.load(path.clone());
                let mut sprite = Sprite::from_image(image);
                let background = if let Some(duration) = fade {
                    sprite.color = sprite.color.with_alpha(0.0);
                    commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                            VisualTween {
                                from_alpha: Some(0.0),
                                to_alpha: Some(1.0),
                                from_translation: None,
                                to_translation: None,
                                from_scale: None,
                                to_scale: None,
                                timer: Timer::new(duration, TimerMode::Once),
                                animation_id,
                                done,
                                despawn_on_finish: false,
                            },
                        ))
                        .id()
                } else {
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    entity
                };

                if let Some(previous) = stage.background.replace(background) {
                    if let Some(duration) = fade {
                        commands.entity(previous).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id: None,
                            done: None,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(previous).try_despawn();
                    }
                }

                shared_state.0.lock().unwrap().background = Some(ImageLayerSnapshot { path });
            }
            ScriptCommand::RuleTransitionBg {
                path,
                rule_path,
                duration,
                vague,
                animation_id,
                done,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }

                let Some(previous_background) = stage.background else {
                    let image = asset_server.load(path.clone());
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            Sprite::from_image(image),
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    shared_state.0.lock().unwrap().background = Some(ImageLayerSnapshot { path });
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let Some(previous_path) = shared_state
                    .0
                    .lock()
                    .unwrap()
                    .background
                    .as_ref()
                    .map(|background| background.path.clone())
                else {
                    let image = asset_server.load(path.clone());
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            Sprite::from_image(image),
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    stage.background = Some(entity);
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let target_image = asset_server.load(path.clone());
                let previous_image = asset_server.load(previous_path);
                let rule_image = asset_server.load(rule_path);
                let material = rule_materials.add(RuleTransitionMaterial {
                    from_texture: previous_image,
                    to_texture: target_image.clone(),
                    rule_texture: rule_image,
                    progress: 0.0,
                    vague,
                });
                let transition_entity = commands
                    .spawn((
                        Mesh2d(transition_mesh.0.clone()),
                        MeshMaterial2d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_BACKGROUND + 1.0),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        RuleTransitionPlayer {
                            material,
                            target_path: path,
                            target_image,
                            previous_background,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                        },
                    ))
                    .id();
                stage.transition = Some(transition_entity);
            }
            ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
                done,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }

                let source_image = asset_server.load(options.from_path.clone());
                let target_image = asset_server.load(options.to_path.clone());
                let rule_image = asset_server.load(options.rule_path.clone());
                let aux0_image = asset_server.load(options.aux0_path.clone());
                let aux1_image = asset_server.load(options.aux1_path.clone());

                let previous_background = if options.commit_to_bg {
                    stage.background.take()
                } else {
                    stage.background
                };

                let material = custom_effect_materials.add(CustomScreenEffectMaterial {
                    source_texture: source_image,
                    target_texture: target_image.clone(),
                    rule_texture: rule_image,
                    aux0_texture: aux0_image,
                    aux1_texture: aux1_image,
                    progress: 0.0,
                    duration: options.duration.as_secs_f32(),
                    time: 0.0,
                    mode: options.mode,
                    p0: options.p0,
                    p1: options.p1,
                    p2: options.p2,
                    p3: options.p3,
                });

                let effect_entity = commands
                    .spawn((
                        Mesh2d(transition_mesh.0.clone()),
                        MeshMaterial2d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_OVERLAY - 0.5),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        CustomScreenEffectPlayer {
                            material,
                            timer: Timer::new(options.duration, TimerMode::Once),
                            target_path: options.commit_to_bg.then_some(options.to_path),
                            target_image: options.commit_to_bg.then_some(target_image),
                            previous_background,
                            animation_id,
                            done,
                        },
                    ))
                    .id();
                stage.screen_effect = Some(effect_entity);
            }
            ScriptCommand::ShowSprite {
                id,
                path,
                position,
                layer,
                scale,
                fade,
                animation_id,
                done,
            } => {
                let handle = asset_server.load(path.clone());
                let entity = if let Some(entity) = stage.sprites.get(&id).copied() {
                    let mut sprite = Sprite::from_image(handle);
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    commands.entity(entity).insert((
                        SpriteActor {
                            id: id.clone(),
                            path: path.clone(),
                        },
                        sprite,
                        Transform {
                            translation: Vec3::new(position.x, position.y, STAGE_Z_SPRITE + layer),
                            scale: Vec3::splat(scale),
                            ..default()
                        },
                    ));
                    entity
                } else {
                    let mut sprite = Sprite::from_image(handle);
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    let entity = commands
                        .spawn((
                            SpriteActor {
                                id: id.clone(),
                                path: path.clone(),
                            },
                            sprite,
                            Transform {
                                translation: Vec3::new(position.x, position.y, STAGE_Z_SPRITE + layer),
                                scale: Vec3::splat(scale),
                                ..default()
                            },
                        ))
                        .id();
                    stage.sprites.insert(id.clone(), entity);
                    entity
                };

                if let Some(duration) = fade {
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                        done,
                        despawn_on_finish: false,
                    });
                } else {
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::HideSprite {
                id,
                fade,
                animation_id,
                done,
            } => {
                if let Some(entity) = stage.sprites.remove(&id) {
                    if let Some(duration) = fade {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(entity).try_despawn();
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::SetOverlay {
                alpha,
                fade,
                animation_id,
                done,
            } => {
                if let Some(overlay) = stage.overlay {
                    if let Some(duration) = fade {
                        let current_alpha = shared_state.0.lock().unwrap().overlay_alpha;
                        commands.entity(overlay).insert(VisualTween {
                            from_alpha: Some(current_alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        commands.entity(overlay).insert(Sprite::from_color(
                            Color::BLACK.with_alpha(alpha),
                            Vec2::new(6000.0, 6000.0),
                        ));
                        if let Some(done) = done {
                            let _ = done.send(ScriptResponse::Continue);
                        }
                        if let Some(animation_id) = animation_id {
                            animations.completed.insert(animation_id);
                        }
                    }
                    shared_state.0.lock().unwrap().overlay_alpha = alpha;
                }
            }
            ScriptCommand::Say {
                speaker,
                text,
                animation_id,
                done,
            } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
                }
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_style,
                        &text,
                        0,
                        None,
                        None,
                    );
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance { animation_id, done });
                shared_state.0.lock().unwrap().dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::AwaitDialogueAdvance { done } => {
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id: None,
                    done: Some(done),
                });
            }
            ScriptCommand::SetDialogue {
                speaker,
                text,
                reveal_from,
                animation_id,
                done,
            } => {
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_style,
                        &text,
                        reveal_from.unwrap_or_else(|| text.chars().count()),
                        animation_id,
                        done,
                    );
                }
                shared_state.0.lock().unwrap().dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::ClearDialogue => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
                }
                clear_dialogue_spans(&mut commands, &mut dialogue_state);
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Hidden;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = String::new();
                }
                if let Ok(mut line_node) = line_text.single_mut() {
                    **line_node = String::new();
                }
                shared_state.0.lock().unwrap().dialogue = None;
            }
            ScriptCommand::SetTextEffect(effect) => {
                apply_text_effect_spec(&mut dialogue_state.effect, effect);
                shared_state.0.lock().unwrap().text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::ResetTextEffect => {
                dialogue_state.effect = DialogueTextEffect::default();
                shared_state.0.lock().unwrap().text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::ApplyUserSettings(settings) => {
                *user_settings = settings;
            }
            ScriptCommand::ApplyUiStyle(style_patch) => {
                apply_ui_style_patch(&mut ui_style, style_patch);
                refresh_dialogue_ui_style(
                    &ui_style,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ResetUiStyle => {
                *ui_style = UiStyle::default();
                refresh_dialogue_ui_style(
                    &ui_style,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ShowScreen { screen, done } => {
                clear_screen_ui(&mut commands, &mut screen_state);
                let root = spawn_screen_ui(&mut commands, &ui_fonts, &ui_style, &screen);
                screen_state.active_root = Some(root);
                screen_state.waiting = Some(done);
            }
            ScriptCommand::Choose {
                prompt,
                options,
                done,
            } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                spawn_choice_ui(&mut commands, &ui_fonts, &ui_style, &prompt, &options);
                choice_state.waiting = Some(done);
                choice_state.options = options;
            }
            ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                position,
                scale,
                fade,
                animation_id,
                done,
            } => {
                let Some(character) = characters.characters.get(&character_name).cloned() else {
                    warn!("character `{character_name}` not found in catalog");
                    complete_missing_animation(&mut animations, animation_id, done);
                    continue;
                };

                despawn_character_actor(&mut commands, &mut stage, &mut pending_characters, &actor_id);
                stage.character_positions.insert(actor_id.clone(), position);
                queue_character_show(
                    &mut commands,
                    &asset_server,
                    &mut stage,
                    &mut pending_characters,
                    &mut animations,
                    actor_id,
                    character,
                    position,
                    scale,
                    fade,
                    animation_id,
                    done,
                );
            }
            ScriptCommand::HideCharacter { actor_id } => {
                despawn_character_actor(&mut commands, &mut stage, &mut pending_characters, &actor_id);
                stage.character_positions.remove(&actor_id);
            }
            ScriptCommand::JumpCharacter {
                actor_id,
                height,
                duration,
                animation_id,
                done,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Jump { height },
                    duration,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::ShakeCharacter {
                actor_id,
                amplitude,
                duration,
                animation_id,
                done,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Shake { amplitude },
                    duration,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::AnimateCharacter {
                actor_id,
                keyframes,
                animation_id,
                done,
            } => {
                apply_character_timeline(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    keyframes,
                    animation_id,
                    done,
                    &mut animations,
                );
            }
            ScriptCommand::RestoreSnapshot { snapshot, done } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                commands.insert_resource(CameraShakeState::default());
                commands.insert_resource(AnimationState::default());
                pending_characters.items.clear();
                restore_scene_snapshot(
                    &mut commands,
                    &asset_server,
                    &mut stage,
                    &mut dialogue_state,
                    &mut choice_state,
                    &mut dialogue_root,
                    &mut speaker_text,
                    &mut line_text,
                    snapshot.clone(),
                );
                *shared_state.0.lock().unwrap() = snapshot;
                let _ = done.send(ScriptResponse::Continue);
            }
            ScriptCommand::MoveSprite {
                id,
                position,
                duration,
                animation_id,
                done,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = shared_state.0.lock().unwrap();
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: Some(from),
                            to_translation: Some(Vec3::new(
                                position.x,
                                position.y,
                                STAGE_Z_SPRITE + sprite.layer,
                            )),
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during move");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for move_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::ScaleSprite {
                id,
                scale,
                duration,
                animation_id,
                done,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = shared_state.0.lock().unwrap();
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::splat(sprite.scale);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: None,
                            to_translation: None,
                            from_scale: Some(from),
                            to_scale: Some(Vec3::splat(scale)),
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during scale");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for scale_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::FadeSprite {
                id,
                alpha,
                duration,
                animation_id,
                done,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = shared_state.0.lock().unwrap();
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(sprite.alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            done,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during fade");
                        complete_missing_animation(&mut animations, animation_id, done);
                    }
                } else {
                    warn!("sprite `{id}` not found for fade_sprite");
                    complete_missing_animation(&mut animations, animation_id, done);
                }
            }
            ScriptCommand::Wait {
                duration,
                animation_id,
                done,
            } => {
                waits.items.push(PendingWait {
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id,
                    done,
                });
            }
            ScriptCommand::WaitAnimations { ids, done } => {
                if ids.iter().all(|id| animations.completed.contains(id)) {
                    let _ = done.send(ScriptResponse::Continue);
                } else {
                    animations.waits.push(PendingAnimationWait { ids, done });
                }
            }
            ScriptCommand::Shake {
                duration,
                amplitude,
                animation_id,
                done,
            } => {
                commands.insert_resource(CameraShakeState {
                    active: Some(CameraShake {
                        timer: Timer::new(duration, TimerMode::Once),
                        amplitude,
                        animation_id,
                        done,
                    }),
                });
            }
            ScriptCommand::PlayBgm {
                path,
                volume,
                fade_in,
                animation_id,
                done,
            } => {
                let volume = apply_volume_setting(volume, user_settings.bgm_volume);
                if let Some(previous) = stage.bgm.take() {
                    commands.entity(previous).try_despawn();
                }
                let start_volume = if fade_in.is_some() { 0.0 } else { volume };
                let bgm = commands
                    .spawn((
                        BgmChannel { path: path.clone() },
                        AudioPlayer::new(asset_server.load(path.clone())),
                        PlaybackSettings::LOOP.with_volume(Volume::Linear(start_volume)),
                    ))
                    .id();
                if let Some(fade_in) = fade_in {
                    commands.entity(bgm).insert(BgmFade {
                        from: start_volume,
                        to: volume,
                        timer: Timer::new(fade_in, TimerMode::Once),
                        animation_id,
                        done,
                    });
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
                }
                stage.bgm = Some(bgm);
                shared_state.0.lock().unwrap().bgm = Some(AudioSnapshot { path, volume });
            }
            ScriptCommand::SetBgmVolume { volume } => {
                let volume = apply_volume_setting(volume, user_settings.bgm_volume);
                if let Some(bgm) = stage.bgm {
                    commands.entity(bgm).insert(BgmFade {
                        from: volume,
                        to: volume,
                        timer: Timer::new(std::time::Duration::ZERO, TimerMode::Once),
                        animation_id: None,
                        done: None,
                    });
                }
                if let Some(snapshot) = shared_state.0.lock().unwrap().bgm.as_mut() {
                    snapshot.volume = volume;
                }
            }
            ScriptCommand::FadeBgm {
                volume,
                duration,
                animation_id,
                done,
            } => {
                let volume = apply_volume_setting(volume, user_settings.bgm_volume);
                let from = shared_state
                    .0
                    .lock()
                    .unwrap()
                    .bgm
                    .as_ref()
                    .map(|bgm| bgm.volume)
                    .unwrap_or(volume);
                if let Some(bgm) = stage.bgm {
                    commands.entity(bgm).insert(BgmFade {
                        from,
                        to: volume,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                        done,
                    });
                    if let Some(snapshot) = shared_state.0.lock().unwrap().bgm.as_mut() {
                        snapshot.volume = volume;
                    }
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    if let Some(done) = done {
                        let _ = done.send(ScriptResponse::Continue);
                    }
                }
            }
            ScriptCommand::StopBgm => {
                if let Some(previous) = stage.bgm.take() {
                    commands.entity(previous).try_despawn();
                }
                shared_state.0.lock().unwrap().bgm = None;
            }
            ScriptCommand::PlayVoice {
                path,
                volume,
                animation_id,
                done,
            } => {
                let volume = apply_volume_setting(volume, user_settings.voice_volume);
                finish_active_voice(&mut commands, &mut animations, &mut voice_state);
                let voice = commands
                    .spawn((
                        VoiceChannel { path: path.clone() },
                        AudioPlayer::new(asset_server.load(path.clone())),
                        PlaybackSettings::ONCE.with_volume(Volume::Linear(volume)),
                    ))
                    .id();
                voice_state.active = Some(ActiveVoice {
                    entity: voice,
                    animation_id,
                    done,
                });
            }
            ScriptCommand::StopVoice => {
                finish_active_voice(&mut commands, &mut animations, &mut voice_state);
            }
            ScriptCommand::PlaySfx { path, volume } => {
                let volume = apply_volume_setting(volume, user_settings.sfx_volume);
                commands.spawn((
                    AudioPlayer::new(asset_server.load(path)),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume)),
                ));
            }
            ScriptCommand::SubmitBatch { mode, items } => match mode {
                BatchSubmitMode::Parallel => {
                    pending_script_commands
                        .items
                        .extend(items.into_iter().map(|item| *item.command));
                }
                BatchSubmitMode::Sequence => {
                    let mut remaining = items.into_iter().collect::<VecDeque<_>>();
                    let Some(first) = remaining.pop_front() else {
                        continue;
                    };
                    let current_handle = first.handle.clone();
                    pending_script_commands.items.push_back(*first.command);
                    active_batches.items.push(ActiveScriptBatch {
                        remaining,
                        current_handle,
                    });
                }
            },
            ScriptCommand::CancelAnimations { ids } => {
                pending_cancels.ids.extend(ids);
            }
            ScriptCommand::ReturnToTitle => {
                finish_active_voice(&mut commands, &mut animations, &mut voice_state);
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                pending_characters.items.clear();
                *shared_state.0.lock().unwrap() = SceneSnapshot::default();
                restore_scene_snapshot(
                    &mut commands,
                    &asset_server,
                    &mut stage,
                    &mut dialogue_state,
                    &mut choice_state,
                    &mut dialogue_root,
                    &mut speaker_text,
                    &mut line_text,
                    SceneSnapshot::default(),
                );
                frontend.runtime_started = true;
                frontend.screen = FrontendScreen::InGame;
                frontend.notice = None;
                frontend.dirty = false;
                if !frontend.startup_script.is_empty() {
                    spawn_script_runtime(
                        &mut commands,
                        vfs.0.clone(),
                        shared_state.0.clone(),
                        ScriptBootstrap::new(frontend.startup_script.clone()),
                    );
                }
            }
        }
    }
}

pub fn animate_bgm_fades(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    shared_state: ResMut<SceneSharedState>,
    mut bgms: Query<(Entity, Option<&mut AudioSink>, &mut BgmFade)>,
) {
    for (entity, sink, mut fade) in &mut bgms {
        fade.timer.tick(time.delta());
        let fraction = tween_fraction(&fade.timer);
        let volume = fade.from + (fade.to - fade.from) * fraction;

        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(volume));
        }
        if let Some(snapshot) = shared_state.0.lock().unwrap().bgm.as_mut() {
            snapshot.volume = volume;
        }

        if fade.timer.is_finished() {
            if let Some(animation_id) = fade.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if let Some(done) = fade.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
            commands.entity(entity).try_remove::<BgmFade>();
        }
    }
}

pub fn handle_choice_buttons(
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    ui_style: Res<UiStyle>,
    mut interaction_query: Query<
        (&Interaction, &mut BackgroundColor, &ChoiceButton),
        Changed<Interaction>,
    >,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                *color = ui_style.choice_button_pressed.into();
                resolve_choice(&mut commands, &mut choice_state, &choice_ui, button.index);
            }
            Interaction::Hovered => {
                *color = ui_style.choice_button_hovered.into();
            }
            Interaction::None => {
                *color = ui_style.choice_button_bg.into();
            }
        }
    }
}

pub fn handle_screen_buttons(
    mut commands: Commands,
    mut screen_state: ResMut<ScreenUiState>,
    ui_style: Res<UiStyle>,
    mut interaction_query: Query<
        (&Interaction, &mut BackgroundColor, &ScreenUiButton),
        Changed<Interaction>,
    >,
) {
    for (interaction, mut color, button) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                *color = ui_style.choice_button_pressed.into();
                let Some(done) = screen_state.waiting.take() else {
                    continue;
                };
                clear_screen_ui(&mut commands, &mut screen_state);
                let _ = done.send(ScriptResponse::Choice(button.value.clone()));
            }
            Interaction::Hovered => {
                *color = ui_style.choice_button_hovered.into();
            }
            Interaction::None => {
                *color = ui_style.choice_button_bg.into();
            }
        }
    }
}

pub fn handle_choice_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    if choice_state.waiting.is_none() {
        return;
    }

    let selected = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::Numpad1,
        KeyCode::Numpad2,
        KeyCode::Numpad3,
        KeyCode::Numpad4,
        KeyCode::Numpad5,
        KeyCode::Numpad6,
        KeyCode::Numpad7,
        KeyCode::Numpad8,
        KeyCode::Numpad9,
    ]
    .into_iter()
    .find_map(|key| {
        if keys.just_pressed(key) {
            Some(match key {
                KeyCode::Digit1 | KeyCode::Numpad1 => 0,
                KeyCode::Digit2 | KeyCode::Numpad2 => 1,
                KeyCode::Digit3 | KeyCode::Numpad3 => 2,
                KeyCode::Digit4 | KeyCode::Numpad4 => 3,
                KeyCode::Digit5 | KeyCode::Numpad5 => 4,
                KeyCode::Digit6 | KeyCode::Numpad6 => 5,
                KeyCode::Digit7 | KeyCode::Numpad7 => 6,
                KeyCode::Digit8 | KeyCode::Numpad8 => 7,
                KeyCode::Digit9 | KeyCode::Numpad9 => 8,
                _ => unreachable!(),
            })
        } else {
            None
        }
    });

    if let Some(index) = selected {
        resolve_choice(&mut commands, &mut choice_state, &choice_ui, index);
    }
}

pub fn advance_dialogue_on_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    inline_control: Res<InlineDialogueControlResource>,
    mut pending_cancels: ResMut<PendingAnimationCancels>,
    mut dialogue_chars: Query<&mut DialogueCharSpan>,
    choice_state: Res<ChoiceState>,
) {
    if choice_state.waiting.is_some() {
        return;
    }

    let advance = keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Space)
        || mouse.just_pressed(MouseButton::Left);

    if !advance {
        return;
    }

    let mut inline = inline_control.0.lock().unwrap();
    if inline.active {
        inline.skip_requested = true;
        if let Some(handle) = inline.current_handle.clone() {
            pending_cancels.ids.push(handle);
        }
        drop(inline);
        reveal_all_dialogue_chars(&mut dialogue_state, &mut dialogue_chars);
        return;
    }
    drop(inline);

    if dialogue_reveal_has_hidden_chars(&dialogue_state) {
        reveal_all_dialogue_chars(&mut dialogue_state, &mut dialogue_chars);
        return;
    }

    if let Some(waiting) = dialogue_state.waiting.take() {
        if let Some(animation_id) = waiting.animation_id {
            animations.completed.insert(animation_id);
        }
        if let Some(done) = waiting.done {
            let _ = done.send(ScriptResponse::Continue);
        }
    }
}

pub fn animate_dialogue_text_reveal(
    time: Res<Time>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<(&mut TextColor, &mut DialogueCharSpan)>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    reveal.accumulator += time.delta_secs();
    while reveal.next_index < reveal.spans.len() && reveal.accumulator >= reveal.interval {
        reveal.accumulator -= reveal.interval;
        if let Some(entity) = reveal.spans.get(reveal.next_index).copied()
            && let Ok((_, mut span)) = dialogue_chars.get_mut(entity)
        {
            span.revealed = true;
            span.age = 0.0;
        }
        reveal.next_index += 1;
    }

    let mut fully_visible = reveal.next_index >= reveal.spans.len();
    for &entity in &reveal.spans {
        if let Ok((mut color, mut span)) = dialogue_chars.get_mut(entity) {
            if span.revealed {
                span.age = (span.age + time.delta_secs()).min(reveal.fade_seconds);
                let alpha = if reveal.fade_seconds <= f32::EPSILON {
                    span.target_alpha
                } else {
                    span.target_alpha * (span.age / reveal.fade_seconds).clamp(0.0, 1.0)
                };
                color.0.set_alpha(alpha);
                if span.age + f32::EPSILON < reveal.fade_seconds {
                    fully_visible = false;
                }
            } else {
                color.0.set_alpha(0.0);
                fully_visible = false;
            }
        }
    }

    if fully_visible {
        if let Some(animation_id) = reveal.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        if let Some(done) = reveal.done.take() {
            let _ = done.send(ScriptResponse::Continue);
        }
        dialogue_state.reveal = None;
    }
}

pub fn tick_pending_waits(time: Res<Time>, mut waits: ResMut<PendingWaits>, mut animations: ResMut<AnimationState>) {
    for wait in waits.items.iter_mut() {
        wait.timer.tick(time.delta());
    }

    let mut completed = Vec::new();
    waits.items.retain(|wait| {
        if wait.timer.is_finished() {
            completed.push((wait.animation_id.clone(), wait.done.clone()));
            false
        } else {
            true
        }
    });

    for (animation_id, done) in completed {
        if let Some(animation_id) = animation_id {
            animations.completed.insert(animation_id);
        }
        let _ = done.send(ScriptResponse::Continue);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_animation_cancellations(
    mut commands: Commands,
    mut stage: ResMut<StageState>,
    mut waits: ResMut<PendingWaits>,
    mut dialogue_state: ResMut<DialogueState>,
    mut pending_cancels: ResMut<PendingAnimationCancels>,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    mut voice_state: ResMut<VoiceState>,
    mut pending_characters: ResMut<PendingCharacterShows>,
    mut tweens: Query<(Entity, Option<&SpriteActor>, &mut VisualTween)>,
    mut bgm_fades: Query<(Entity, &mut BgmFade)>,
    mut motion_queries: ParamSet<(
        Query<'_, '_, &'static mut Transform, With<MainCamera>>,
        Query<'_, '_, (
            Entity,
            &'static mut Transform,
            Option<&'static mut CharacterJumpEffect>,
            Option<&'static mut CharacterShakeEffect>,
            Option<&'static mut CharacterTimelineEffect>,
        ), Without<MainCamera>>,
    )>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    if pending_cancels.ids.is_empty() {
        return;
    }

    let cancelled = pending_cancels.ids.drain(..).collect::<HashSet<_>>();
    for id in &cancelled {
        animations.completed.insert(id.clone());
    }

    waits.items.retain(|wait| {
        if wait
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            let _ = wait.done.send(ScriptResponse::Continue);
            false
        } else {
            true
        }
    });

    if dialogue_state
        .waiting
        .as_ref()
        .and_then(|waiting| waiting.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        if let Some(waiting) = dialogue_state.waiting.take() {
            complete_missing_animation(&mut animations, waiting.animation_id, waiting.done);
        }
    }

    if let Some(shake) = shake_state.active.as_mut()
        && shake
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        if let Ok(mut camera) = motion_queries.p0().single_mut() {
            camera.translation.x = 0.0;
            camera.translation.y = 0.0;
        }
        complete_missing_animation(&mut animations, shake.animation_id.take(), shake.done.take());
        shake_state.active = None;
    }

    if voice_state
        .active
        .as_ref()
        .and_then(|voice| voice.animation_id.as_ref())
        .is_some_and(|animation_id| cancelled.contains(animation_id))
    {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }

    pending_characters.items.retain_mut(|item| {
        if item
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            for entity in item.entities.drain(..) {
                commands.entity(entity).try_despawn();
            }
            for id in item.entity_ids.drain(..) {
                stage.sprites.remove(&id);
            }
            complete_missing_animation(&mut animations, item.animation_id.take(), item.done.take());
            false
        } else {
            true
        }
    });

    for (entity, actor, mut tween) in &mut tweens {
        if tween
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if tween.despawn_on_finish && let Some(actor) = actor {
                stage.sprites.insert(actor.id.clone(), entity);
            }
            complete_missing_animation(&mut animations, tween.animation_id.take(), tween.done.take());
            commands.entity(entity).try_remove::<VisualTween>();
        }
    }

    for (entity, mut fade) in &mut bgm_fades {
        if fade
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, fade.animation_id.take(), fade.done.take());
            commands.entity(entity).try_remove::<BgmFade>();
        }
    }

    for (entity, mut transform, jump, shake, timeline) in &mut motion_queries.p1() {
        let mut reset_translation = false;
        let origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin));

        if let Some(mut effect) = jump
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
            commands.entity(entity).try_remove::<CharacterJumpEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = shake
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
            commands.entity(entity).try_remove::<CharacterShakeEffect>();
            reset_translation = true;
        }

        if let Some(mut effect) = timeline
            && effect
                .animation_id
                .as_ref()
                .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if let Some(final_keyframe) = effect.keyframes.last() {
                stage.character_positions.insert(effect.actor_id.clone(), final_keyframe.position);
            }
            complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
            commands.entity(entity).try_remove::<CharacterTimelineEffect>();
            reset_translation = true;
        }

        if reset_translation {
            if let Some(origin) = origin {
                transform.translation = origin;
            }
        }
    }

    for (entity, mut transition) in &mut transitions {
        if transition
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.transition == Some(entity) {
                stage.transition = None;
            }
            complete_missing_animation(
                &mut animations,
                transition.animation_id.take(),
                transition.done.take(),
            );
            commands.entity(entity).try_despawn();
        }
    }

    for (entity, mut effect) in &mut effects {
        if effect
            .animation_id
            .as_ref()
            .is_some_and(|animation_id| cancelled.contains(animation_id))
        {
            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
            commands.entity(entity).try_despawn();
        }
    }
}

pub fn animate_visual_tweens(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut sprites: Query<(Entity, &mut Sprite, &mut Transform, &mut VisualTween)>,
) {
    for (entity, mut sprite, mut transform, mut tween) in &mut sprites {
        tween.timer.tick(time.delta());
        let fraction = tween_fraction(&tween.timer);

        if let (Some(from), Some(to)) = (tween.from_alpha, tween.to_alpha) {
            sprite.color.set_alpha(from + (to - from) * fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_translation, tween.to_translation) {
            transform.translation = from.lerp(to, fraction);
        }
        if let (Some(from), Some(to)) = (tween.from_scale, tween.to_scale) {
            transform.scale = from.lerp(to, fraction);
        }

        if tween.timer.is_finished() {
            if let Some(to) = tween.to_alpha {
                sprite.color.set_alpha(to);
            }
            if let Some(to) = tween.to_translation {
                transform.translation = to;
            }
            if let Some(to) = tween.to_scale {
                transform.scale = to;
            }
            if let Some(animation_id) = tween.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if let Some(done) = tween.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
            if tween.despawn_on_finish {
                commands.entity(entity).try_despawn();
            } else {
                commands.entity(entity).try_remove::<VisualTween>();
            }
        }
    }
}

pub fn animate_camera_shake(
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut shake_state: ResMut<CameraShakeState>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let Ok(mut camera) = camera.single_mut() else {
        return;
    };

    let Some(shake) = shake_state.active.as_mut() else {
        camera.translation.x = 0.0;
        camera.translation.y = 0.0;
        return;
    };

    shake.timer.tick(time.delta());
    let decay = 1.0 - tween_fraction(&shake.timer);
    let elapsed = shake.timer.elapsed_secs();
    let amplitude = shake.amplitude * decay;

    camera.translation.x = (elapsed * 43.0).sin() * amplitude;
    camera.translation.y = (elapsed * 31.0).cos() * amplitude;

    if shake.timer.is_finished() {
        camera.translation.x = 0.0;
        camera.translation.y = 0.0;
        if let Some(animation_id) = shake.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        if let Some(done) = shake.done.take() {
            let _ = done.send(ScriptResponse::Continue);
        }
        shake_state.active = None;
    }
}

pub fn animate_character_motion_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut stage: ResMut<StageState>,
    mut movers: Query<
        (
            Entity,
            &'static mut Transform,
            Option<&'static mut CharacterJumpEffect>,
            Option<&'static mut CharacterShakeEffect>,
            Option<&'static mut CharacterTimelineEffect>,
        ),
        Without<MainCamera>,
    >,
) {
    for (entity, mut transform, jump, shake, timeline) in &mut movers {
        let base_origin = timeline
            .as_ref()
            .map(|effect| effect.origin)
            .or_else(|| jump.as_ref().map(|effect| effect.origin))
            .or_else(|| shake.as_ref().map(|effect| effect.origin))
            .unwrap_or(transform.translation);

        let mut translation = base_origin;

        if let Some(mut effect) = timeline {
            effect.elapsed = (effect.elapsed + time.delta_secs()).min(effect.duration);
            let actor_position = character_timeline_position(effect.actor_origin, &effect.keyframes, effect.elapsed);
            translation += (actor_position - effect.actor_origin).extend(0.0);
            stage.character_positions.insert(effect.actor_id.clone(), actor_position);

            if effect.elapsed >= effect.duration {
                complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
                commands.entity(entity).try_remove::<CharacterTimelineEffect>();
            }
        }

        if let Some(mut effect) = jump {
            effect.timer.tick(time.delta());
            let progress = tween_fraction(&effect.timer);
            translation.y += (std::f32::consts::PI * progress).sin().max(0.0) * effect.height;
            if effect.timer.is_finished() {
                complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
                commands.entity(entity).try_remove::<CharacterJumpEffect>();
            }
        }

        if let Some(mut effect) = shake {
            effect.timer.tick(time.delta());
            let decay = 1.0 - tween_fraction(&effect.timer);
            let elapsed = effect.timer.elapsed_secs();
            translation += Vec3::new(
                (elapsed * 52.0).sin() * effect.amplitude * decay,
                (elapsed * 39.0).cos() * effect.amplitude * 0.35 * decay,
                0.0,
            );
            if effect.timer.is_finished() {
                complete_missing_animation(&mut animations, effect.animation_id.take(), effect.done.take());
                commands.entity(entity).try_remove::<CharacterShakeEffect>();
            }
        }

        transform.translation = translation;
    }
}

pub fn animate_rule_transitions(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut rule_materials: ResMut<Assets<RuleTransitionMaterial>>,
    mut transitions: Query<(Entity, &mut RuleTransitionPlayer)>,
) {
    for (entity, mut transition) in &mut transitions {
        transition.timer.tick(time.delta());
        let progress = tween_fraction(&transition.timer);

        if let Some(material) = rule_materials.get_mut(&transition.material) {
            material.progress = progress;
        }

        if transition.timer.is_finished() {
            let new_background = commands
                .spawn((
                    BackgroundLayer {
                        path: transition.target_path.clone(),
                    },
                    Sprite::from_image(transition.target_image.clone()),
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id();

            stage.background = Some(new_background);
            if stage.transition == Some(entity) {
                stage.transition = None;
            }

            commands.entity(transition.previous_background).try_despawn();
            commands.entity(entity).try_despawn();

            if let Some(animation_id) = transition.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if let Some(done) = transition.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
        }
    }
}

pub fn animate_custom_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut materials: ResMut<Assets<CustomScreenEffectMaterial>>,
    mut effects: Query<(Entity, &mut CustomScreenEffectPlayer)>,
) {
    for (entity, mut effect) in &mut effects {
        effect.timer.tick(time.delta());
        let progress = tween_fraction(&effect.timer);

        if let Some(material) = materials.get_mut(&effect.material) {
            material.progress = progress;
            material.time = effect.timer.elapsed_secs();
        }

        if effect.timer.is_finished() {
            if let Some(target_path) = effect.target_path.take()
                && let Some(target_image) = effect.target_image.take()
            {
                let new_background = commands
                    .spawn((
                        BackgroundLayer { path: target_path },
                        Sprite::from_image(target_image),
                        Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                    ))
                    .id();
                stage.background = Some(new_background);
            }

            if let Some(previous_background) = effect.previous_background.take()
                && stage.background != Some(previous_background)
            {
                commands.entity(previous_background).try_despawn();
            }

            if stage.screen_effect == Some(entity) {
                stage.screen_effect = None;
            }
            commands.entity(entity).try_despawn();

            if let Some(animation_id) = effect.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            if let Some(done) = effect.done.take() {
                let _ = done.send(ScriptResponse::Continue);
            }
        }
    }
}

pub fn tick_animation_waits(mut animations: ResMut<AnimationState>) {
    let completed = animations.completed.clone();
    let mut resolved = Vec::new();
    animations.waits.retain(|wait| {
        if wait.ids.iter().all(|id| completed.contains(id)) {
            resolved.push(wait.done.clone());
            false
        } else {
            true
        }
    });

    for done in resolved {
        let _ = done.send(ScriptResponse::Continue);
    }
}

pub fn tick_script_batches(
    mut pending_script_commands: ResMut<PendingScriptCommands>,
    mut active_batches: ResMut<ActiveScriptBatches>,
    animations: Res<AnimationState>,
) {
    let completed = animations.completed.clone();
    active_batches.items.retain_mut(|batch| {
        if !completed.contains(&batch.current_handle) {
            return true;
        }

        while let Some(next) = batch.remaining.pop_front() {
            if completed.contains(&next.handle) {
                continue;
            }

            batch.current_handle = next.handle;
            pending_script_commands.items.push_back(*next.command);
            return true;
        }

        false
    });
}

pub fn poll_voice_playback(
    mut commands: Commands,
    mut animations: ResMut<AnimationState>,
    mut voice_state: ResMut<VoiceState>,
    sinks: Query<&AudioSink>,
) {
    let Some(active) = voice_state.active.as_ref() else {
        return;
    };

    let entity = active.entity;
    let finished = match sinks.get(entity) {
        Ok(sink) => sink.empty(),
        Err(_) => false,
    };

    if finished {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }
}

pub fn poll_pending_character_shows(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut stage: ResMut<StageState>,
    mut animations: ResMut<AnimationState>,
    mut pending: ResMut<PendingCharacterShows>,
    sprite_entities: Query<(), (With<Sprite>, With<Visibility>)>,
    mut sprites: Query<(&mut Sprite, &mut Visibility)>,
) {
    let mut completed = Vec::new();
    pending.items.retain_mut(|item| {
        let has_failed = item
            .handles
            .iter()
            .any(|handle| matches!(asset_server.load_state(handle.id()), bevy::asset::LoadState::Failed(_)));
        if has_failed {
            warn!("failed to load one or more parts for character `{}`", item.actor_id);
            for entity in item.entities.drain(..) {
                commands.entity(entity).try_despawn();
            }
            for id in item.entity_ids.drain(..) {
                stage.sprites.remove(&id);
            }
            complete_missing_animation(
                &mut animations,
                item.animation_id.take(),
                item.done.take(),
            );
            return false;
        }

        if !item
            .handles
            .iter()
            .all(|handle| asset_server.is_loaded_with_dependencies(handle.id()))
        {
            return true;
        }

        if !item
            .entities
            .iter()
            .all(|entity| sprite_entities.get(*entity).is_ok())
        {
            return true;
        }

        completed.push((
            item.entities.clone(),
            item.fade,
            item.animation_id.take(),
            item.done.take(),
        ));
        false
    });

    for (entities, fade, animation_id, done) in completed {
        let mut pending_done = done;
        let mut pending_animation = animation_id;
        for (index, entity) in entities.into_iter().enumerate() {
            if let Ok((mut sprite, mut visibility)) = sprites.get_mut(entity) {
                *visibility = Visibility::Visible;
                if let Some(fade) = fade {
                    sprite.color.set_alpha(0.0);
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(fade, TimerMode::Once),
                        animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                        done: (index == 0).then(|| pending_done.take()).flatten(),
                        despawn_on_finish: false,
                    });
                }
            }
        }

        if fade.is_none() {
            complete_missing_animation(&mut animations, pending_animation, pending_done);
        }
    }
}

pub fn sync_scene_snapshot(
    shared_state: Res<SceneSharedState>,
    stage: Res<StageState>,
    dialogue_state: Res<DialogueState>,
    background_layers: Query<&BackgroundLayer>,
    bgms: Query<(&BgmChannel, Option<&AudioSink>)>,
    overlay: Query<&Sprite, With<OverlayMarker>>,
    sprites: Query<(&SpriteActor, &Sprite, &Transform)>,
) {
    let mut snapshot = shared_state.0.lock().unwrap();

    snapshot.background = stage
        .background
        .and_then(|entity| background_layers.get(entity).ok())
        .map(|layer| ImageLayerSnapshot {
            path: layer.path.clone(),
        });

    let mut sprite_snapshots = sprites
        .iter()
        .map(|(actor, sprite, transform)| SpriteSnapshot {
            id: actor.id.clone(),
            path: actor.path.clone(),
            x: transform.translation.x,
            y: transform.translation.y,
            layer: transform.translation.z - STAGE_Z_SPRITE,
            scale: transform.scale.x,
            alpha: sprite.color.alpha(),
        })
        .collect::<Vec<_>>();
    sprite_snapshots.sort_by(|left, right| left.id.cmp(&right.id));
    snapshot.sprites = sprite_snapshots;
    snapshot.character_positions = stage
        .character_positions
        .iter()
        .map(|(actor_id, position)| (actor_id.clone(), [position.x, position.y]))
        .collect();

    snapshot.bgm = stage.bgm.and_then(|entity| {
        bgms.get(entity).ok().map(|(bgm, sink)| AudioSnapshot {
            path: bgm.path.clone(),
            volume: sink.map(|sink| sink.volume().to_linear()).unwrap_or(1.0),
        })
    });

    if let Ok(overlay_sprite) = overlay.single() {
        snapshot.overlay_alpha = overlay_sprite.color.alpha();
    }

    snapshot.text_effect = text_effect_snapshot(&dialogue_state.effect);
}

fn queue_character_show(
    commands: &mut Commands,
    asset_server: &AssetServer,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    animations: &mut AnimationState,
    actor_id: String,
    character: CharacterDefinition,
    position: Vec2,
    scale: f32,
    fade: Option<std::time::Duration>,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
) {
    let mut entities = Vec::new();
    let mut entity_ids = Vec::new();
    let mut handles = Vec::new();

    for part in &character.parts {
        let sprite_id = character_part_id(&actor_id, &part.id);
        let handle: Handle<Image> = asset_server.load(part.path.clone());
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite_id.clone(),
                    path: part.path.clone(),
                },
                Sprite::from_image(handle.clone()),
                Visibility::Hidden,
                Transform {
                    translation: Vec3::new(
                        position.x + part.offset.x * scale,
                        position.y + part.offset.y * scale,
                        STAGE_Z_SPRITE + part.layer,
                    ),
                    scale: Vec3::splat(scale),
                    ..default()
                },
            ))
            .id();

        stage.sprites.insert(sprite_id.clone(), entity);
        entities.push(entity);
        entity_ids.push(sprite_id);
        handles.push(handle);
    }

    if entities.is_empty() {
        complete_missing_animation(
            animations,
            animation_id,
            done,
        );
        return;
    }

    pending.items.push(PendingCharacterShow {
        actor_id,
        entity_ids,
        entities,
        handles,
        fade,
        animation_id,
        done,
    });
}

#[allow(clippy::too_many_arguments)]
fn apply_character_motion(
    commands: &mut Commands,
    stage: &mut StageState,
    shared_state: &SceneSharedState,
    actor_id: &str,
    kind: CharacterMotionKind,
    duration: std::time::Duration,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
    animations: &mut AnimationState,
) {
    let prefix = character_part_prefix(actor_id);
    let snapshot = shared_state.0.lock().unwrap().clone();
    let mut part_ids = snapshot
        .sprites
        .iter()
        .filter(|sprite| sprite.id.starts_with(&prefix))
        .map(|sprite| (sprite.id.clone(), sprite.x, sprite.y, sprite.layer))
        .collect::<Vec<_>>();
    part_ids.sort_by(|left, right| left.0.cmp(&right.0));

    if part_ids.is_empty() {
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let mut pending_animation = animation_id;
    let mut pending_done = done;
    for (index, (id, x, y, layer)) in part_ids.into_iter().enumerate() {
        let Some(entity) = stage.sprites.get(&id).copied() else {
            continue;
        };
        match kind {
            CharacterMotionKind::Jump { height } => {
                commands.entity(entity).insert(CharacterJumpEffect {
                    origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
                    timer: Timer::new(duration, TimerMode::Once),
                    height,
                    animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                    done: (index == 0).then(|| pending_done.take()).flatten(),
                });
            }
            CharacterMotionKind::Shake { amplitude } => {
                commands.entity(entity).insert(CharacterShakeEffect {
                    origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
                    timer: Timer::new(duration, TimerMode::Once),
                    amplitude,
                    animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
                    done: (index == 0).then(|| pending_done.take()).flatten(),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_character_timeline(
    commands: &mut Commands,
    stage: &mut StageState,
    shared_state: &SceneSharedState,
    actor_id: &str,
    keyframes: Vec<ResolvedCharacterKeyframe>,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
    animations: &mut AnimationState,
) {
    let Some(actor_origin) = stage.character_positions.get(actor_id).copied() else {
        complete_missing_animation(animations, animation_id, done);
        return;
    };

    let duration = keyframes.last().map(|frame| frame.time).unwrap_or(0.0);
    if duration <= f32::EPSILON {
        if let Some(final_position) = keyframes.last().map(|frame| frame.position) {
            stage.character_positions.insert(actor_id.to_string(), final_position);
        }
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let prefix = character_part_prefix(actor_id);
    let snapshot = shared_state.0.lock().unwrap().clone();
    let mut part_ids = snapshot
        .sprites
        .iter()
        .filter(|sprite| sprite.id.starts_with(&prefix))
        .map(|sprite| (sprite.id.clone(), sprite.x, sprite.y, sprite.layer))
        .collect::<Vec<_>>();
    part_ids.sort_by(|left, right| left.0.cmp(&right.0));

    if part_ids.is_empty() {
        complete_missing_animation(animations, animation_id, done);
        return;
    }

    let mut pending_animation = animation_id;
    let mut pending_done = done;
    for (index, (id, x, y, layer)) in part_ids.into_iter().enumerate() {
        let Some(entity) = stage.sprites.get(&id).copied() else {
            continue;
        };
        commands.entity(entity).insert(CharacterTimelineEffect {
            origin: Vec3::new(x, y, STAGE_Z_SPRITE + layer),
            actor_id: actor_id.to_string(),
            actor_origin,
            keyframes: keyframes.clone(),
            elapsed: 0.0,
            duration,
            animation_id: (index == 0).then(|| pending_animation.take()).flatten(),
            done: (index == 0).then(|| pending_done.take()).flatten(),
        });
    }
}

fn character_timeline_position(
    actor_origin: Vec2,
    keyframes: &[ResolvedCharacterKeyframe],
    elapsed: f32,
) -> Vec2 {
    let Some(first) = keyframes.first() else {
        return actor_origin;
    };

    if elapsed <= first.time {
        return interpolate_character_position(actor_origin, 0.0, first.clone(), elapsed);
    }

    let mut previous = ResolvedCharacterKeyframe {
        time: 0.0,
        position: actor_origin,
        ease: CharacterEase::Linear,
    };
    for keyframe in keyframes {
        if elapsed <= keyframe.time {
            return interpolate_character_position(previous.position, previous.time, keyframe.clone(), elapsed);
        }
        previous = keyframe.clone();
    }

    keyframes.last().map(|frame| frame.position).unwrap_or(actor_origin)
}

fn interpolate_character_position(
    start: Vec2,
    start_time: f32,
    end: ResolvedCharacterKeyframe,
    elapsed: f32,
) -> Vec2 {
    let duration = (end.time - start_time).max(f32::EPSILON);
    let fraction = ((elapsed - start_time) / duration).clamp(0.0, 1.0);
    let fraction = apply_character_ease(end.ease, fraction);
    start.lerp(end.position, fraction)
}

fn apply_character_ease(ease: CharacterEase, t: f32) -> f32 {
    match ease {
        CharacterEase::Linear => t,
        CharacterEase::Ease | CharacterEase::EaseInOut => t * t * (3.0 - 2.0 * t),
        CharacterEase::EaseIn => t * t,
        CharacterEase::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        CharacterEase::Bounce => {
            let n1 = 7.5625;
            let d1 = 2.75;
            if t < 1.0 / d1 {
                n1 * t * t
            } else if t < 2.0 / d1 {
                let t = t - 1.5 / d1;
                n1 * t * t + 0.75
            } else if t < 2.5 / d1 {
                let t = t - 2.25 / d1;
                n1 * t * t + 0.9375
            } else {
                let t = t - 2.625 / d1;
                n1 * t * t + 0.984375
            }
        }
    }
}

fn despawn_character_actor(
    commands: &mut Commands,
    stage: &mut StageState,
    pending: &mut PendingCharacterShows,
    actor_id: &str,
) {
    let prefix = character_part_prefix(actor_id);

    pending.items.retain_mut(|item| {
        if item.actor_id != actor_id {
            return true;
        }

        for entity in item.entities.drain(..) {
            commands.entity(entity).try_despawn();
        }
        false
    });

    let ids = stage
        .sprites
        .keys()
        .filter(|id| id.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    for id in ids {
        if let Some(entity) = stage.sprites.remove(&id) {
            commands.entity(entity).try_despawn();
        }
    }
}

fn character_part_prefix(actor_id: &str) -> String {
    format!("character::{actor_id}::")
}

fn character_part_id(actor_id: &str, part_id: &str) -> String {
    format!("{}{}", character_part_prefix(actor_id), part_id)
}

#[allow(clippy::too_many_arguments)]
fn start_frontend_session(
    commands: &mut Commands,
    asset_server: &AssetServer,
    vfs: &VfsResource,
    shared_state: &SceneSharedState,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    dialogue_root: &mut Query<&mut Visibility, With<DialogueRoot>>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    frontend: &mut FrontendState,
    bootstrap: ScriptBootstrap,
    snapshot: SceneSnapshot,
) {
    if let Some(root) = frontend.root.take() {
        commands.entity(root).try_despawn();
    }

    clear_choice_ui(commands, choice_ui);
    *shared_state.0.lock().unwrap() = snapshot.clone();
    restore_scene_snapshot(
        commands,
        asset_server,
        stage,
        dialogue_state,
        choice_state,
        dialogue_root,
        speaker_text,
        line_text,
        snapshot,
    );

    frontend.notice = None;
    frontend.runtime_started = true;
    frontend.screen = FrontendScreen::InGame;
    frontend.dirty = false;

    spawn_script_runtime(commands, vfs.0.clone(), shared_state.0.clone(), bootstrap);
}

fn shorten_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn adjusted_volume(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(0.0, 1.0)
}

fn apply_volume_setting(volume: f32, setting: f32) -> f32 {
    (volume * setting).clamp(0.0, 1.0)
}

fn format_storage_error(error: StorageError) -> String {
    format!("Failed to write settings: {error}")
}

fn apply_ui_style_patch(ui_style: &mut UiStyle, patch: UiStylePatch) {
    if let Some(color) = patch.dialogue_bg {
        ui_style.dialogue_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.dialogue_border {
        ui_style.dialogue_border = color_from_rgba(color);
    }
    if let Some(color) = patch.speaker_color {
        ui_style.speaker_color = color_from_rgba(color);
    }
    if let Some(color) = patch.line_color {
        ui_style.line_color = color_from_rgba(color);
    }
    if let Some(color) = patch.hint_color {
        ui_style.hint_color = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_panel_bg {
        ui_style.choice_panel_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_prompt_color {
        ui_style.choice_prompt_color = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_bg {
        ui_style.choice_button_bg = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_hovered {
        ui_style.choice_button_hovered = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_pressed {
        ui_style.choice_button_pressed = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_button_border {
        ui_style.choice_button_border = color_from_rgba(color);
    }
    if let Some(color) = patch.choice_text_color {
        ui_style.choice_text_color = color_from_rgba(color);
    }
}

fn clear_dialogue_spans(commands: &mut Commands, dialogue_state: &mut DialogueState) {
    for entity in dialogue_state.span_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    dialogue_state.reveal = None;
}

fn set_dialogue_line_text(
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    line_root: Entity,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    ui_style: &UiStyle,
    text: &str,
    visible_prefix_chars: usize,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
) {
    clear_dialogue_spans(commands, dialogue_state);

    if let Ok(mut line_node) = line_text.single_mut() {
        **line_node = String::new();
    }

    let chars = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    let target_alpha = ui_style.line_color.alpha();
    let reveal_enabled = dialogue_state.effect.mode != DialogueTextEffectMode::Instant;
    let visible_prefix_chars = visible_prefix_chars.min(chars.len());

    for (index, ch) in chars.iter().enumerate() {
        let revealed = !reveal_enabled || index < visible_prefix_chars;
        let initial_alpha = if revealed { target_alpha } else { 0.0 };
        let entity = commands
            .spawn((
                TextSpan::new(ch.clone()),
                TextColor(ui_style.line_color.with_alpha(initial_alpha)),
                DialogueCharSpan {
                    target_alpha,
                    age: if revealed {
                        dialogue_state.effect.fade_seconds
                    } else {
                        0.0
                    },
                    revealed,
                },
            ))
            .id();
        commands.entity(line_root).add_child(entity);
        dialogue_state.span_entities.push(entity);
    }

    if reveal_enabled && visible_prefix_chars < chars.len() {
        dialogue_state.reveal = Some(DialogueRevealState {
            spans: dialogue_state.span_entities.clone(),
            next_index: visible_prefix_chars,
            accumulator: 0.0,
            interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
            fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
            animation_id,
            done,
        });
    } else {
        dialogue_state.reveal = None;
        if let Some(animation_id) = animation_id {
            if let Some(done) = done {
                let _ = done.send(ScriptResponse::Continue);
            }
            let _ = animation_id;
        } else if let Some(done) = done {
            let _ = done.send(ScriptResponse::Continue);
        }
    }
}

fn dialogue_reveal_has_hidden_chars(dialogue_state: &DialogueState) -> bool {
    dialogue_state
        .reveal
        .as_ref()
        .is_some_and(|reveal| reveal.next_index < reveal.spans.len())
}

fn reveal_all_dialogue_chars(
    dialogue_state: &mut DialogueState,
    dialogue_chars: &mut Query<&mut DialogueCharSpan>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    for &entity in &reveal.spans {
        if let Ok(mut span) = dialogue_chars.get_mut(entity) {
            span.revealed = true;
            span.age = reveal.fade_seconds;
        }
    }
    reveal.next_index = reveal.spans.len();
    reveal.accumulator = 0.0;
}

fn apply_text_effect_spec(effect: &mut DialogueTextEffect, spec: crate::script::DialogueTextEffectSpec) {
    if let Some(mode) = spec.mode.as_deref() {
        effect.mode = match mode {
            "instant" => DialogueTextEffectMode::Instant,
            _ => DialogueTextEffectMode::TypewriterFade,
        };
    }
    if let Some(cps) = spec.cps {
        effect.cps = cps.max(1.0);
    }
    if let Some(fade_seconds) = spec.fade_seconds {
        effect.fade_seconds = fade_seconds.max(0.0);
    }
    if let Some(fade_ms) = spec.fade_ms {
        effect.fade_seconds = (fade_ms / 1000.0).max(0.0);
    }
}

fn text_effect_snapshot(effect: &DialogueTextEffect) -> TextEffectSnapshot {
    TextEffectSnapshot {
        mode: match effect.mode {
            DialogueTextEffectMode::Instant => "instant".to_string(),
            DialogueTextEffectMode::TypewriterFade => "typewriter_fade".to_string(),
        },
        cps: effect.cps,
        fade_seconds: effect.fade_seconds,
    }
}

fn dialogue_text_effect_from_snapshot(snapshot: &TextEffectSnapshot) -> DialogueTextEffect {
    let mut effect = DialogueTextEffect::default();
    if snapshot.mode == "instant" {
        effect.mode = DialogueTextEffectMode::Instant;
    }
    if snapshot.cps > 0.0 {
        effect.cps = snapshot.cps;
    }
    if snapshot.fade_seconds >= 0.0 {
        effect.fade_seconds = snapshot.fade_seconds;
    }
    effect
}

fn refresh_dialogue_ui_style(
    ui_style: &UiStyle,
    dialogue_background: &mut Query<&mut BackgroundColor, With<DialogueRoot>>,
    dialogue_border: &mut Query<&mut BorderColor, With<DialogueRoot>>,
    speaker_color: &mut Query<
        &mut TextColor,
        (With<SpeakerText>, Without<LineText>, Without<HintText>),
    >,
    line_color: &mut Query<
        &mut TextColor,
        (With<LineText>, Without<SpeakerText>, Without<HintText>),
    >,
    hint_color: &mut Query<
        &mut TextColor,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
) {
    if let Ok(mut color) = dialogue_background.single_mut() {
        *color = ui_style.dialogue_bg.into();
    }
    if let Ok(mut color) = dialogue_border.single_mut() {
        *color = BorderColor::all(ui_style.dialogue_border);
    }
    if let Ok(mut color) = speaker_color.single_mut() {
        *color = ui_style.speaker_color.into();
    }
    if let Ok(mut color) = line_color.single_mut() {
        *color = ui_style.line_color.into();
    }
    if let Ok(mut color) = hint_color.single_mut() {
        *color = ui_style.hint_color.into();
    }
}

fn color_from_rgba(rgba: [f32; 4]) -> Color {
    Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn clear_screen_ui(commands: &mut Commands, screen_state: &mut ScreenUiState) {
    if let Some(root) = screen_state.active_root.take() {
        commands.entity(root).try_despawn();
    }
}

fn spawn_screen_ui(
    commands: &mut Commands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    screen: &ScreenSpec,
) -> Entity {
    let root = commands
        .spawn((
            ScreenUiRoot,
            ScreenUiNode,
            Node {
                position_type: PositionType::Absolute,
                left: px(0.0),
                right: px(0.0),
                top: px(0.0),
                bottom: px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                padding: UiRect::all(px(24.0)),
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.35)),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        let mut panel = parent.spawn((
            ScreenUiNode,
            Node {
                width: screen.width.map(px).unwrap_or(percent(72.0)),
                max_width: percent(92.0),
                padding: UiRect::all(px(screen.padding.max(0.0))),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(screen.gap.max(0.0)),
                ..default()
            },
            BackgroundColor(
                screen
                    .background
                    .map(color_from_rgba)
                    .unwrap_or(ui_style.choice_panel_bg),
            ),
            BorderColor::all(
                screen
                    .border
                    .map(color_from_rgba)
                    .unwrap_or(ui_style.choice_button_border),
            ),
        ));

        panel.with_children(|panel| {
            if let Some(title) = screen.title.as_ref() {
                panel.spawn((
                    ScreenUiNode,
                    Text::new(title.clone()),
                    ui_text_font(ui_fonts, 34.0),
                    TextColor(ui_style.speaker_color),
                ));
            }

            for child in &screen.children {
                spawn_screen_node(panel, ui_fonts, ui_style, child);
            }
        });
    });

    root
}

fn spawn_screen_node(
    parent: &mut bevy::ecs::hierarchy::ChildSpawnerCommands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    node: &ScreenNode,
) {
    match node {
        ScreenNode::Text(TextNode { text, size, color }) => {
            parent.spawn((
                ScreenUiNode,
                Text::new(text.clone()),
                ui_text_font(ui_fonts, *size),
                TextColor(color.map(color_from_rgba).unwrap_or(ui_style.line_color)),
            ));
        }
        ScreenNode::Button(ButtonNode {
            text,
            value,
            size,
            color,
            background,
            border,
            width,
        }) => {
            parent
                .spawn((
                    ScreenUiNode,
                    ScreenUiButton {
                        value: value.clone(),
                    },
                    Button,
                    Node {
                        width: width.map(px).unwrap_or(percent(100.0)),
                        border: UiRect::all(px(1.0)),
                        padding: UiRect::axes(px(18.0), px(14.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(14.0)),
                        ..default()
                    },
                    BackgroundColor(background.map(color_from_rgba).unwrap_or(ui_style.choice_button_bg)),
                    BorderColor::all(border.map(color_from_rgba).unwrap_or(ui_style.choice_button_border)),
                ))
                .with_children(|button| {
                    button.spawn((
                        ScreenUiNode,
                        Text::new(text.clone()),
                        ui_text_font(ui_fonts, *size),
                        TextColor(color.map(color_from_rgba).unwrap_or(ui_style.choice_text_color)),
                    ));
                });
        }
        ScreenNode::Row(ContainerNode { gap, children }) => {
            parent
                .spawn((
                    ScreenUiNode,
                    Node {
                        width: percent(100.0),
                        column_gap: px(*gap),
                        ..default()
                    },
                ))
                .with_children(|container| {
                    for child in children {
                        spawn_screen_node(container, ui_fonts, ui_style, child);
                    }
                });
        }
        ScreenNode::Column(ContainerNode { gap, children }) => {
            parent
                .spawn((
                    ScreenUiNode,
                    Node {
                        width: percent(100.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(*gap),
                        ..default()
                    },
                ))
                .with_children(|container| {
                    for child in children {
                        spawn_screen_node(container, ui_fonts, ui_style, child);
                    }
                });
        }
        ScreenNode::Spacer(SpacerNode { width, height }) => {
            parent.spawn((
                ScreenUiNode,
                Node {
                    width: px(*width),
                    height: px(*height),
                    ..default()
                },
            ));
        }
    }
}

fn ui_text_font(ui_fonts: &UiFonts, font_size: f32) -> TextFont {
    TextFont::from_font_size(font_size).with_font(ui_fonts.regular.clone())
}

fn spawn_choice_ui(
    commands: &mut Commands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    prompt: &str,
    options: &[ChoiceOption],
) {
    let root = commands
        .spawn((
            ChoiceUi,
            Node {
                position_type: PositionType::Absolute,
                left: px(24.0),
                right: px(24.0),
                bottom: px(230.0),
                padding: UiRect::all(px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(12.0),
                ..default()
            },
            BackgroundColor(ui_style.choice_panel_bg),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        if !prompt.is_empty() {
            parent.spawn((
                ChoiceUi,
                Text::new(prompt),
                ui_text_font(ui_fonts, 22.0),
                TextColor(ui_style.choice_prompt_color),
            ));
        }

        for (index, option) in options.iter().enumerate() {
            parent
                .spawn((
                    ChoiceUi,
                    ChoiceButton { index },
                    Button,
                    Node {
                        width: percent(100.0),
                        border: UiRect::all(px(1.0)),
                        padding: UiRect::axes(px(18.0), px(14.0)),
                        justify_content: JustifyContent::FlexStart,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(12.0)),
                        ..default()
                    },
                    BackgroundColor(ui_style.choice_button_bg),
                    BorderColor::all(ui_style.choice_button_border),
                ))
                .with_children(|button| {
                    button.spawn((
                        ChoiceUi,
                        Text::new(format!("{}. {}", index + 1, option.text)),
                        ui_text_font(ui_fonts, 26.0),
                        TextColor(ui_style.choice_text_color),
                    ));
                });
        }
    });
}

fn clear_choice_ui(
    commands: &mut Commands,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    let entities = choice_ui.iter().collect::<Vec<_>>();
    for entity in entities {
        commands.entity(entity).try_despawn();
    }
}

fn resolve_choice(
    commands: &mut Commands,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    index: usize,
) {
    let Some(selected) = choice_state.options.get(index).cloned() else {
        return;
    };
    let Some(done) = choice_state.waiting.take() else {
        return;
    };

    clear_choice_ui(commands, choice_ui);
    choice_state.options.clear();
    let _ = done.send(ScriptResponse::Choice(selected.value));
}

#[allow(clippy::too_many_arguments)]
fn restore_scene_snapshot(
    commands: &mut Commands,
    asset_server: &AssetServer,
    stage: &mut StageState,
    dialogue_state: &mut DialogueState,
    choice_state: &mut ChoiceState,
    dialogue_root: &mut Query<&mut Visibility, With<DialogueRoot>>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    snapshot: SceneSnapshot,
) {
    let text_effect = snapshot.text_effect.clone();

    if let Some(background) = stage.background.take() {
        commands.entity(background).try_despawn();
    }
    if let Some(effect) = stage.screen_effect.take() {
        commands.entity(effect).try_despawn();
    }
    if let Some(transition) = stage.transition.take() {
        commands.entity(transition).try_despawn();
    }
    for (_, entity) in stage.sprites.drain() {
        commands.entity(entity).try_despawn();
    }
    stage.character_positions.clear();
    if let Some(bgm) = stage.bgm.take() {
        commands.entity(bgm).try_despawn();
    }

    if let Some(background) = snapshot.background.as_ref() {
        stage.background = Some(
            commands
                .spawn((
                    BackgroundLayer {
                        path: background.path.clone(),
                    },
                    Sprite::from_image(asset_server.load(background.path.clone())),
                    Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                ))
                .id(),
        );
    }

    for sprite in &snapshot.sprites {
        let mut entity_sprite = Sprite::from_image(asset_server.load(sprite.path.clone()));
        entity_sprite.color.set_alpha(sprite.alpha);
        let entity = commands
            .spawn((
                SpriteActor {
                    id: sprite.id.clone(),
                    path: sprite.path.clone(),
                },
                entity_sprite,
                Transform {
                    translation: Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer),
                    scale: Vec3::splat(sprite.scale),
                    ..default()
                },
            ))
            .id();
        stage.sprites.insert(sprite.id.clone(), entity);
    }

    stage.character_positions = snapshot
        .character_positions
        .iter()
        .map(|(actor_id, position)| (actor_id.clone(), Vec2::new(position[0], position[1])))
        .collect();

    if let Some(bgm) = snapshot.bgm.as_ref() {
        stage.bgm = Some(
            commands
                .spawn((
                    BgmChannel {
                        path: bgm.path.clone(),
                    },
                    AudioPlayer::new(asset_server.load(bgm.path.clone())),
                    PlaybackSettings::LOOP.with_volume(Volume::Linear(bgm.volume)),
                ))
                .id(),
        );
    }

    if let Some(overlay) = stage.overlay {
        commands.entity(overlay).insert(Sprite::from_color(
            Color::BLACK.with_alpha(snapshot.overlay_alpha),
            Vec2::new(6000.0, 6000.0),
        ));
    }

    match snapshot.dialogue {
        Some(dialogue) => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Visible;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = dialogue.speaker;
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = dialogue.text;
            }
        }
        None => {
            clear_dialogue_spans(commands, dialogue_state);
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Hidden;
            }
            if let Ok(mut speaker) = speaker_text.single_mut() {
                **speaker = String::new();
            }
            if let Ok(mut line) = line_text.single_mut() {
                **line = String::new();
            }
        }
    }

    dialogue_state.effect = dialogue_text_effect_from_snapshot(&text_effect);
    dialogue_state.waiting = None;
    choice_state.waiting = None;
    choice_state.options.clear();
}

fn finish_active_voice(
    commands: &mut Commands,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    let Some(mut active) = voice_state.active.take() else {
        return;
    };

    commands.entity(active.entity).try_despawn();
    if let Some(animation_id) = active.animation_id.take() {
        animations.completed.insert(animation_id);
    }
    if let Some(done) = active.done.take() {
        let _ = done.send(ScriptResponse::Continue);
    }
}

fn complete_missing_animation(
    animations: &mut AnimationState,
    animation_id: Option<String>,
    done: Option<mpsc::Sender<ScriptResponse>>,
) {
    if let Some(animation_id) = animation_id {
        animations.completed.insert(animation_id);
    }
    if let Some(done) = done {
        let _ = done.send(ScriptResponse::Continue);
    }
}

fn tween_fraction(timer: &Timer) -> f32 {
    let duration = timer.duration().as_secs_f32();
    if duration <= f32::EPSILON {
        1.0
    } else {
        (timer.elapsed_secs() / duration).clamp(0.0, 1.0)
    }
}
