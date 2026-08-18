use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

#[cfg(not(target_arch = "wasm32"))]
use std::thread;

use bevy::{
    log::{error, info},
    math::{Vec2, Vec4},
    prelude::Resource,
};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg32;
use rhai::plugin::*;
use rhai::{
    Array, Blob, Dynamic, Engine as RhaiEngine, EvalAltResult, FLOAT, FnPtr, INT, ImmutableString,
    Map, Module, NativeCallContext, Position, Scope,
};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{
    audio::{AudioCatalog, load_audio_catalog},
    character::load_character_catalog,
    effect::custom::CustomEffectOptions,
    state::{
        ChoiceOption, RngState, SaveCheckpoint, SaveGameData, SavedInput, SceneSharedState,
        SceneSnapshot, ScriptPosition, StoredValue, UiStylePatch,
    },
    storage::{
        StorageError, UserSettings, load_save_data_from_root, read_user_settings, save_root_path,
        slot_path_in, write_save_data_to_root, write_user_settings,
    },
    texture::{TextureCatalog, load_texture_catalog},
    ui::{
        BarNode, ButtonNode, ContainerNode, ScreenImageButtonNode, ScreenImageNode, ScreenLayout,
        ScreenNode, ScreenSpec, ScreenTexture, SpacerNode, TextNode,
    },
    vfs::{HdpVfs, VfsError},
};

mod compiler;
mod hiraku_engine;
mod ir;
mod rng;
mod time;
mod ui;
pub mod ui_runtime;

pub use compiler::{IrCompileError, compile_to_ir};
pub use ir::{
    IrChoiceOption, IrCommand, IrEvent, IrExpressionId, IrInstruction, IrProgram, IrRuntime,
    IrValidationError, IrVm, IrVmSnapshot, IrVmStatus, IrWaitKind, tick_ir_runtime,
};
pub use ui_runtime::{UiContext, UiIntent, evaluate_ui_script};

const PRELUDE_SOURCE: &str = include_str!("script/prelude.rhai");

const JUMP_SCRIPT_SIGNAL: &str = "__hiraku_jump_script__::";
const CALL_SCRIPT_SIGNAL: &str = "__hiraku_call_script__::";
const SAVE_SCRIPT_SIGNAL: &str = "__hiraku_save_script__::";
const RETURN_SCRIPT_SIGNAL: &str = "__hiraku_return_script__";
const RETURN_TO_TITLE_SIGNAL: &str = "__hiraku_return_to_title__";
const ENGINE_STOPPED_SIGNAL: &str = "__hiraku_engine_stopped__";
const WEB_RUNTIME_YIELD_SIGNAL: &str = "__hiraku_web_runtime_yield__";

enum InlineDialogueChunk {
    Text(String),
    Command(String),
}

enum ScriptFlowAction {
    Jump(String),
    Call(String),
    Save {
        slot: String,
        resume_script: Option<String>,
    },
    Return,
    ReturnToTitle,
    EngineStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckpointDecision {
    Run,
    ReplaySkip,
}

#[derive(Debug)]
pub enum ScriptCommand {
    Log(String),
    StartIr {
        path: String,
        program: IrProgram,
    },
    StartLegacy {
        path: String,
    },
    SetBackground {
        path: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ShowSprite {
        id: String,
        path: String,
        rect: Option<[f32; 4]>,
        position: Vec2,
        layer: f32,
        scale: f32,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    HideSprite {
        id: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    SetOverlay {
        alpha: f32,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    Say {
        speaker: String,
        text: String,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    AwaitDialogueAdvance {
        done: mpsc::Sender<ScriptResponse>,
    },
    SetDialogue {
        speaker: String,
        text: String,
        reveal_from: Option<usize>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ClearDialogue,
    SetTextEffect(DialogueTextEffectSpec),
    ResetTextEffect,
    SetCamera {
        blur_intensity: Option<f32>,
        zoom: Option<f32>,
        center: Option<Vec2>,
        duration: Duration,
        ease: CharacterEase,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ApplyUserSettings(UserSettings),
    ApplyUiStyle(UiStylePatch),
    ResetUiStyle,
    ShowScreen {
        screen: ScreenSpec,
        shown: Option<mpsc::Sender<ScriptResponse>>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    WaitForScreenChoice {
        done: mpsc::Sender<ScriptResponse>,
    },
    ShowOverlay {
        name: String,
        screen: ScreenSpec,
    },
    HideOverlay {
        name: String,
    },
    Choose {
        prompt: String,
        options: Vec<ChoiceOption>,
        done: mpsc::Sender<ScriptResponse>,
    },
    ShowCharacter {
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: Vec2,
        scale: f32,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    HideCharacter {
        actor_id: String,
    },
    JumpCharacter {
        actor_id: String,
        height: f32,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ShakeCharacter {
        actor_id: String,
        amplitude: f32,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    AnimateCharacter {
        actor_id: String,
        keyframes: Vec<ResolvedCharacterKeyframe>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    RestoreSnapshot {
        snapshot: SceneSnapshot,
        done: mpsc::Sender<ScriptResponse>,
    },
    PlayCustomEffect {
        options: CustomEffectOptions,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    RuleTransitionBg {
        path: String,
        rule_path: String,
        duration: Duration,
        vague: f32,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    MoveSprite {
        id: String,
        position: Vec2,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ScaleSprite {
        id: String,
        scale: f32,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    FadeSprite {
        id: String,
        alpha: f32,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    Wait {
        duration: Duration,
        animation_id: Option<String>,
        done: mpsc::Sender<ScriptResponse>,
    },
    WaitAnimations {
        ids: Vec<String>,
        done: mpsc::Sender<ScriptResponse>,
    },
    Shake {
        duration: Duration,
        amplitude: f32,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    PlayBgm {
        path: String,
        volume: f32,
        fade_in: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    SetBgmVolume {
        volume: f32,
    },
    FadeBgm {
        volume: f32,
        duration: Duration,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    StopBgm,
    PlayVoice {
        path: String,
        volume: f32,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    StopVoice,
    PlaySfx {
        path: String,
        volume: f32,
    },
    SubmitBatch {
        mode: BatchSubmitMode,
        items: Vec<BatchSubmissionItem>,
    },
    CancelAnimations {
        ids: Vec<String>,
    },
    ReturnToTitle,
    Exit,
}

pub(crate) fn script_command_from_ir(
    command: IrCommand,
    textures: Option<&TextureCatalog>,
) -> Result<ScriptCommand, String> {
    let command = match command {
        IrCommand::Log(message) => ScriptCommand::Log(message),
        IrCommand::ClearDialogue => ScriptCommand::ClearDialogue,
        IrCommand::Say { speaker, text } => ScriptCommand::Say {
            speaker,
            text,
            animation_id: None,
            done: None,
        },
        IrCommand::StopBgm => ScriptCommand::StopBgm,
        IrCommand::PlayBgm {
            path,
            volume,
            fade_in_ms,
        } => ScriptCommand::PlayBgm {
            path,
            volume,
            fade_in: fade_in_ms.map(Duration::from_millis),
            animation_id: None,
            done: None,
        },
        IrCommand::SetCamera {
            blur,
            zoom,
            duration_ms,
            ease,
        } => ScriptCommand::SetCamera {
            blur_intensity: blur,
            zoom,
            center: None,
            duration: Duration::from_millis(duration_ms),
            ease: parse_ir_camera_ease(&ease)?,
            animation_id: None,
            done: None,
        },
        IrCommand::Exit => ScriptCommand::Exit,
        IrCommand::Choose { .. } | IrCommand::OpenUi { .. } => {
            return Err("interactive UI commands are handled by the IR runtime".to_string());
        }
        IrCommand::LoadScript { .. } => {
            return Err("load_script is handled by the IR runtime".to_string());
        }
        IrCommand::ReturnToTitle => ScriptCommand::ReturnToTitle,
        IrCommand::SetBackground { texture } => {
            let Some(definition) = textures.and_then(|catalog| catalog.resolve(&texture)) else {
                return Err(format!("texture `{texture}` is not defined"));
            };
            ScriptCommand::SetBackground {
                path: definition.path.clone(),
                fade: None,
                animation_id: None,
                done: None,
            }
        }
        IrCommand::ShowCharacter {
            actor_id,
            character_name,
            expressions,
            position,
            scale,
        } => ScriptCommand::ShowCharacter {
            actor_id,
            character_name,
            expressions,
            position: Vec2::new(position[0], position[1]),
            scale,
            fade: None,
            animation_id: None,
            done: None,
        },
    };
    Ok(command)
}

fn parse_ir_camera_ease(name: &str) -> Result<CharacterEase, String> {
    match name {
        "" | "linear" => Ok(CharacterEase::Linear),
        "ease" => Ok(CharacterEase::Ease),
        "ease_in" => Ok(CharacterEase::EaseIn),
        "ease_out" => Ok(CharacterEase::EaseOut),
        "ease_in_out" => Ok(CharacterEase::EaseInOut),
        "bounce" => Ok(CharacterEase::Bounce),
        _ => Err(format!("unsupported camera easing `{name}`")),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BatchSubmitMode {
    Sequence,
    Parallel,
}

#[derive(Debug)]
pub struct BatchSubmissionItem {
    pub handle: String,
    pub command: Box<ScriptCommand>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DialogueTextEffectSpec {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub cps: Option<f32>,
    #[serde(default)]
    pub fade_seconds: Option<f32>,
    #[serde(default)]
    pub fade_ms: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DialogueTextEffectInput {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    cps: Option<f64>,
    #[serde(default)]
    fade_seconds: Option<f64>,
    #[serde(default)]
    fade_ms: Option<f64>,
}

impl From<DialogueTextEffectInput> for DialogueTextEffectSpec {
    fn from(input: DialogueTextEffectInput) -> Self {
        Self {
            mode: input.mode,
            cps: input.cps.map(|value| value as f32),
            fade_seconds: input.fade_seconds.map(|value| value as f32),
            fade_ms: input.fade_ms.map(|value| value as f32),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCharacterKeyframe {
    pub time: f32,
    pub position: Vec2,
    pub ease: CharacterEase,
}

#[derive(Debug, Clone, Copy)]
pub enum CharacterEase {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
}

#[derive(Debug, Deserialize)]
struct CharacterAnimationKeyframeInput {
    time: f64,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    dx: Option<f64>,
    #[serde(default)]
    dy: Option<f64>,
    #[serde(default)]
    ease: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChoiceOptionInput {
    Text(String),
    Map(ChoiceOptionMapInput),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceOptionMapInput {
    text: String,
    #[serde(default)]
    value: Option<Dynamic>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiStylePatchInput {
    #[serde(default)]
    dialogue_bg: Option<[f64; 4]>,
    #[serde(default)]
    dialogue_border: Option<[f64; 4]>,
    #[serde(default)]
    dialogue_left: Option<f64>,
    #[serde(default)]
    dialogue_right: Option<f64>,
    #[serde(default)]
    dialogue_bottom: Option<f64>,
    #[serde(default)]
    dialogue_min_height: Option<f64>,
    #[serde(default)]
    dialogue_padding_x: Option<f64>,
    #[serde(default)]
    dialogue_padding_y: Option<f64>,
    #[serde(default)]
    dialogue_radius: Option<f64>,
    #[serde(default)]
    speaker_size: Option<f64>,
    #[serde(default)]
    line_size: Option<f64>,
    #[serde(default)]
    hint_size: Option<f64>,
    #[serde(default)]
    hint_visible: Option<bool>,
    #[serde(default)]
    speaker_color: Option<[f64; 4]>,
    #[serde(default)]
    line_color: Option<[f64; 4]>,
    #[serde(default)]
    hint_color: Option<[f64; 4]>,
    #[serde(default)]
    choice_panel_bg: Option<[f64; 4]>,
    #[serde(default)]
    choice_bottom: Option<f64>,
    #[serde(default)]
    choice_panel_width: Option<f64>,
    #[serde(default)]
    choice_padding: Option<f64>,
    #[serde(default)]
    choice_gap: Option<f64>,
    #[serde(default)]
    choice_prompt_size: Option<f64>,
    #[serde(default)]
    choice_button_size: Option<f64>,
    #[serde(default)]
    choice_center_text: Option<bool>,
    #[serde(default)]
    choice_show_indices: Option<bool>,
    #[serde(default)]
    choice_prompt_color: Option<[f64; 4]>,
    #[serde(default)]
    choice_button_bg: Option<[f64; 4]>,
    #[serde(default)]
    choice_button_hovered: Option<[f64; 4]>,
    #[serde(default)]
    choice_button_pressed: Option<[f64; 4]>,
    #[serde(default)]
    choice_button_border: Option<[f64; 4]>,
    #[serde(default)]
    choice_text_color: Option<[f64; 4]>,
    #[serde(default)]
    quick_menu_bottom: Option<f64>,
    #[serde(default)]
    quick_menu_gap: Option<f64>,
    #[serde(default)]
    quick_button_size: Option<f64>,
    #[serde(default)]
    quick_menu_bg: Option<[f64; 4]>,
    #[serde(default)]
    quick_button_bg: Option<[f64; 4]>,
    #[serde(default)]
    quick_button_hovered: Option<[f64; 4]>,
    #[serde(default)]
    quick_button_pressed: Option<[f64; 4]>,
    #[serde(default)]
    quick_button_border: Option<[f64; 4]>,
    #[serde(default)]
    quick_text_color: Option<[f64; 4]>,
}

impl From<UiStylePatchInput> for UiStylePatch {
    fn from(input: UiStylePatchInput) -> Self {
        Self {
            dialogue_bg: input.dialogue_bg.map(rgba_to_f32),
            dialogue_border: input.dialogue_border.map(rgba_to_f32),
            dialogue_left: input.dialogue_left.map(f64_to_f32),
            dialogue_right: input.dialogue_right.map(f64_to_f32),
            dialogue_bottom: input.dialogue_bottom.map(f64_to_f32),
            dialogue_min_height: input.dialogue_min_height.map(f64_to_f32),
            dialogue_padding_x: input.dialogue_padding_x.map(f64_to_f32),
            dialogue_padding_y: input.dialogue_padding_y.map(f64_to_f32),
            dialogue_radius: input.dialogue_radius.map(f64_to_f32),
            speaker_size: input.speaker_size.map(f64_to_f32),
            line_size: input.line_size.map(f64_to_f32),
            hint_size: input.hint_size.map(f64_to_f32),
            hint_visible: input.hint_visible,
            speaker_color: input.speaker_color.map(rgba_to_f32),
            line_color: input.line_color.map(rgba_to_f32),
            hint_color: input.hint_color.map(rgba_to_f32),
            choice_panel_bg: input.choice_panel_bg.map(rgba_to_f32),
            choice_bottom: input.choice_bottom.map(f64_to_f32),
            choice_panel_width: input.choice_panel_width.map(f64_to_f32),
            choice_padding: input.choice_padding.map(f64_to_f32),
            choice_gap: input.choice_gap.map(f64_to_f32),
            choice_prompt_size: input.choice_prompt_size.map(f64_to_f32),
            choice_button_size: input.choice_button_size.map(f64_to_f32),
            choice_center_text: input.choice_center_text,
            choice_show_indices: input.choice_show_indices,
            choice_prompt_color: input.choice_prompt_color.map(rgba_to_f32),
            choice_button_bg: input.choice_button_bg.map(rgba_to_f32),
            choice_button_hovered: input.choice_button_hovered.map(rgba_to_f32),
            choice_button_pressed: input.choice_button_pressed.map(rgba_to_f32),
            choice_button_border: input.choice_button_border.map(rgba_to_f32),
            choice_text_color: input.choice_text_color.map(rgba_to_f32),
            quick_menu_bottom: input.quick_menu_bottom.map(f64_to_f32),
            quick_menu_gap: input.quick_menu_gap.map(f64_to_f32),
            quick_button_size: input.quick_button_size.map(f64_to_f32),
            quick_menu_bg: input.quick_menu_bg.map(rgba_to_f32),
            quick_button_bg: input.quick_button_bg.map(rgba_to_f32),
            quick_button_hovered: input.quick_button_hovered.map(rgba_to_f32),
            quick_button_pressed: input.quick_button_pressed.map(rgba_to_f32),
            quick_button_border: input.quick_button_border.map(rgba_to_f32),
            quick_text_color: input.quick_text_color.map(rgba_to_f32),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScriptResponse {
    Continue,
    Choice(StoredValue),
}

#[derive(Resource)]
pub struct ScriptInbox(pub Mutex<mpsc::Receiver<ScriptCommand>>);

#[derive(Resource, Clone, Default)]
pub struct InlineDialogueControlResource(pub Arc<Mutex<InlineDialogueControl>>);

#[derive(Resource, Clone)]
pub struct ScriptRuntimeState {
    pub current_script: Arc<Mutex<String>>,
    pub script_stack: Arc<Mutex<Vec<String>>>,
    pub globals: Arc<Mutex<BTreeMap<String, StoredValue>>>,
    pub checkpoint: Arc<Mutex<CheckpointState>>,
    pub random_seed: Arc<Mutex<u64>>,
    pub rng: Arc<Mutex<Pcg32>>,
    pub time_seed: Arc<Mutex<i64>>,
    pub save_root: PathBuf,
}

/// WebAssembly cannot block while Bevy processes a pending script command.
/// The next evaluation replays already completed responses before continuing.
struct WebScriptExecution {
    responses: Vec<ScriptResponse>,
    cursor: usize,
    pending: Option<mpsc::Receiver<ScriptResponse>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Resource)]
pub struct WebScriptRuntime {
    host: ScriptHost,
    startup_scope: BTreeMap<String, StoredValue>,
    execution: Arc<Mutex<WebScriptExecution>>,
    started: bool,
    finished: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CheckpointState {
    pub current: Option<SaveCheckpoint>,
    pub ordinal: u64,
    pub input_log: Vec<SavedInput>,
    pub replay: Option<ReplayState>,
}

#[derive(Clone, Debug)]
pub struct ReplayState {
    pub target: SaveCheckpoint,
    pub input_log: Vec<SavedInput>,
    pub input_cursor: usize,
}

#[derive(Clone, Debug)]
pub struct ScriptBootstrap {
    pub startup_script: String,
    pub script_stack: Vec<String>,
    pub globals: BTreeMap<String, StoredValue>,
    pub scope: BTreeMap<String, StoredValue>,
    pub checkpoint: Option<SaveCheckpoint>,
    pub input_log: Vec<SavedInput>,
    pub random_seed: Option<u64>,
    pub rng_state: Option<RngState>,
    pub time_seed: Option<i64>,
}

impl ScriptBootstrap {
    pub fn new(startup_script: String) -> Self {
        Self {
            startup_script,
            script_stack: Vec::new(),
            globals: BTreeMap::new(),
            scope: BTreeMap::new(),
            checkpoint: None,
            input_log: Vec::new(),
            random_seed: None,
            rng_state: None,
            time_seed: None,
        }
    }

    pub fn from_save(data: &SaveGameData) -> Self {
        Self {
            startup_script: data
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.script.clone())
                .unwrap_or_else(|| data.resume_script.clone()),
            script_stack: data.script_stack.clone(),
            globals: data.globals.clone(),
            scope: data.scope.clone(),
            checkpoint: data.checkpoint.clone(),
            input_log: data.input_log.clone(),
            random_seed: Some(data.random_seed),
            rng_state: data.rng_state.clone(),
            time_seed: Some(data.time_seed),
        }
    }
}

#[derive(Clone)]
struct ScriptHost {
    vfs: Arc<HdpVfs>,
    textures: Arc<TextureCatalog>,
    audio: Arc<AudioCatalog>,
    command_tx: mpsc::Sender<ScriptCommand>,
    current_script: Arc<Mutex<String>>,
    script_stack: Arc<Mutex<Vec<String>>>,
    globals: Arc<Mutex<BTreeMap<String, StoredValue>>>,
    checkpoint: Arc<Mutex<CheckpointState>>,
    pending_scope_restore: Arc<Mutex<Option<BTreeMap<String, StoredValue>>>>,
    random_seed: Arc<Mutex<u64>>,
    time_seed: Arc<Mutex<i64>>,
    rng: Arc<Mutex<Pcg32>>,
    scene_state: Arc<Mutex<SceneSnapshot>>,
    next_animation_id: Arc<Mutex<u64>>,
    batch_registry: Arc<Mutex<BatchRegistry>>,
    character_expressions: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
    inline_dialogue_control: Arc<Mutex<InlineDialogueControl>>,
    save_root: PathBuf,
    web_execution: Option<Arc<Mutex<WebScriptExecution>>>,
}

#[derive(Default)]
pub struct InlineDialogueControl {
    pub active: bool,
    pub skip_requested: bool,
    pub current_handle: Option<String>,
}

#[derive(Default)]
struct BatchRegistry {
    active: Option<BatchScope>,
    groups: BTreeMap<String, BatchGroup>,
    next_group_id: u64,
}

struct BatchScope {
    mode: BatchMode,
    commands: Vec<CollectedCommand>,
}

struct CollectedCommand {
    handle: String,
    command: ScriptCommand,
}

struct BatchGroup {
    handles: Vec<String>,
}

#[derive(Clone, Copy)]
enum BatchMode {
    Sequence,
    Parallel,
}

impl ScriptHost {
    fn current_script_path(&self) -> String {
        self.current_script.lock().unwrap().clone()
    }

    fn set_current_script_path(&self, path: impl Into<String>) {
        let path = path.into();
        let changed = {
            let mut current = self.current_script.lock().unwrap();
            if *current == path {
                false
            } else {
                *current = path;
                true
            }
        };

        if changed && let Some(execution) = &self.web_execution {
            let mut execution = execution.lock().unwrap();
            execution.responses.clear();
            execution.cursor = 0;
        }
    }

    fn reset_script_checkpoint_counter(&self) {
        self.checkpoint.lock().unwrap().ordinal = 0;
    }

    fn checkpoint(&self, kind: &str, label: Option<String>, pos: Position) -> CheckpointDecision {
        let mut state = self.checkpoint.lock().unwrap();
        state.ordinal += 1;
        let checkpoint = SaveCheckpoint {
            script: self.current_script_path(),
            ordinal: state.ordinal,
            kind: kind.to_string(),
            label,
            position: ScriptPosition {
                line: pos.line(),
                column: pos.position(),
            },
        };

        if let Some(replay) = state.replay.as_ref() {
            if checkpoint.script == replay.target.script
                && checkpoint.ordinal < replay.target.ordinal
            {
                state.current = Some(checkpoint);
                return CheckpointDecision::ReplaySkip;
            }
            if checkpoint.script == replay.target.script
                && checkpoint.ordinal == replay.target.ordinal
            {
                state.current = Some(checkpoint);
                state.replay = None;
                return CheckpointDecision::Run;
            }
        }

        state.current = Some(checkpoint);
        CheckpointDecision::Run
    }

    fn is_replaying(&self) -> bool {
        self.checkpoint.lock().unwrap().replay.is_some()
            || self.web_execution.as_ref().is_some_and(|execution| {
                let execution = execution.lock().unwrap();
                execution.cursor < execution.responses.len()
            })
    }

    fn replay_input(&self, expected_kind: &str) -> Result<StoredValue, Box<EvalAltResult>> {
        let mut state = self.checkpoint.lock().unwrap();
        let current = state
            .current
            .clone()
            .ok_or_else(|| runtime_error("save replay reached an event without a checkpoint"))?;
        if current.kind != expected_kind {
            return Err(runtime_error(format!(
                "save replay expected `{expected_kind}` but reached `{}`",
                current.kind
            )));
        }
        let input = {
            let replay = state
                .replay
                .as_mut()
                .ok_or_else(|| runtime_error("save replay is not active"))?;
            let input_index = replay.input_log[replay.input_cursor..]
                .iter()
                .position(|input| checkpoint_matches(&input.checkpoint, &current))
                .map(|index| replay.input_cursor + index)
                .ok_or_else(|| {
                    runtime_error(format!(
                        "save replay reached `{expected_kind}` without a matching recorded value"
                    ))
                })?;
            let input = &replay.input_log[input_index];
            let value = input.value.clone();
            replay.input_cursor = input_index + 1;
            value
        };
        state.input_log.push(SavedInput {
            checkpoint: current,
            value: input.clone(),
        });
        Ok(input)
    }

    fn record_input(&self, value: StoredValue) {
        let mut state = self.checkpoint.lock().unwrap();
        if let Some(checkpoint) = state.current.clone() {
            state.input_log.push(SavedInput { checkpoint, value });
        }
    }

    fn rng_state(&self) -> RngState {
        rng_state_snapshot(&self.rng.lock().unwrap())
    }

    fn resolve_path(&self, requested: &str) -> String {
        self.vfs
            .resolve_path(Some(&self.current_script_path()), requested)
    }

    fn send(&self, command: ScriptCommand) -> Result<(), Box<EvalAltResult>> {
        if self.is_replaying() && command_suppressed_during_replay(&command) {
            return Ok(());
        }

        self.command_tx
            .send(command)
            .map_err(|_| engine_stopped_signal())
    }

    fn send_and_wait<F>(&self, builder: F) -> Result<ScriptResponse, Box<EvalAltResult>>
    where
        F: FnOnce(mpsc::Sender<ScriptResponse>) -> ScriptCommand,
    {
        let (done_tx, done_rx) = mpsc::channel();
        self.send(builder(done_tx))?;
        self.wait_for_response(done_rx)
    }

    fn send_continue<F>(&self, builder: F) -> Result<(), Box<EvalAltResult>>
    where
        F: FnOnce(mpsc::Sender<ScriptResponse>) -> ScriptCommand,
    {
        let (done_tx, done_rx) = mpsc::channel();
        let command = builder(done_tx);
        if self.web_execution.is_none()
            && self.is_replaying()
            && command_suppressed_during_replay(&command)
        {
            return Ok(());
        }

        self.send(command)?;
        match self.wait_for_response(done_rx)? {
            ScriptResponse::Continue => Ok(()),
            ScriptResponse::Choice(_) => {
                Err(runtime_error("engine returned unexpected choice response"))
            }
        }
    }

    fn wait_for_response(
        &self,
        done_rx: mpsc::Receiver<ScriptResponse>,
    ) -> Result<ScriptResponse, Box<EvalAltResult>> {
        let Some(execution) = &self.web_execution else {
            return done_rx.recv().map_err(|_| engine_stopped_signal());
        };

        let mut execution = execution.lock().unwrap();
        if execution.cursor < execution.responses.len() {
            let response = execution.responses[execution.cursor].clone();
            execution.cursor += 1;
            return Ok(response);
        }

        execution.pending = Some(done_rx);
        Err(web_runtime_yield_signal())
    }

    fn set_dialogue(&self, speaker: String, text: String) -> Result<(), Box<EvalAltResult>> {
        self.send(ScriptCommand::SetDialogue {
            speaker,
            text,
            reveal_from: None,
            animation_id: None,
            done: None,
        })
    }

    fn reveal_dialogue_tail(
        &self,
        speaker: String,
        text: String,
        reveal_from: usize,
    ) -> Result<(), Box<EvalAltResult>> {
        self.send_continue(|done| ScriptCommand::SetDialogue {
            speaker,
            text,
            reveal_from: Some(reveal_from),
            animation_id: None,
            done: Some(done),
        })
    }

    fn next_animation_id(&self, kind: &str) -> String {
        let mut counter = self.next_animation_id.lock().unwrap();
        *counter += 1;
        format!("{kind}-{}", *counter)
    }

    fn is_batch_mode(&self) -> bool {
        self.batch_registry.lock().unwrap().active.is_some()
    }

    fn character_expressions(&self, actor_id: &str) -> Vec<String> {
        self.character_expressions
            .lock()
            .unwrap()
            .get(actor_id)
            .cloned()
            .unwrap_or_default()
    }

    fn set_character_expressions(&self, actor_id: String, expressions: Vec<String>) {
        self.character_expressions
            .lock()
            .unwrap()
            .insert(actor_id, expressions);
    }

    fn begin_inline_dialogue(&self) {
        let mut control = self.inline_dialogue_control.lock().unwrap();
        control.active = true;
        control.skip_requested = false;
        control.current_handle = None;
    }

    fn end_inline_dialogue(&self) {
        let mut control = self.inline_dialogue_control.lock().unwrap();
        control.active = false;
        control.skip_requested = false;
        control.current_handle = None;
    }

    fn is_inline_dialogue_active(&self) -> bool {
        self.inline_dialogue_control.lock().unwrap().active
    }

    fn inline_skip_requested(&self) -> bool {
        let control = self.inline_dialogue_control.lock().unwrap();
        control.active && control.skip_requested
    }

    fn set_inline_current_handle(&self, handle: Option<String>) {
        self.inline_dialogue_control.lock().unwrap().current_handle = handle;
    }

    fn begin_batch(&self, mode: BatchMode) -> Result<(), Box<EvalAltResult>> {
        let mut registry = self.batch_registry.lock().unwrap();
        if registry.active.is_some() {
            return Err(runtime_error("seq/par cannot be nested"));
        }
        registry.active = Some(BatchScope {
            mode,
            commands: Vec::new(),
        });
        Ok(())
    }

    fn cancel_batch(&self) {
        self.batch_registry.lock().unwrap().active = None;
    }

    fn collect_command(
        &self,
        kind: &str,
        build: impl FnOnce(Option<String>, Option<mpsc::Sender<ScriptResponse>>) -> ScriptCommand,
    ) -> Result<String, Box<EvalAltResult>> {
        let handle = self.next_animation_id(kind);
        let (done_tx, _done_rx) = mpsc::channel();
        let command = build(Some(handle.clone()), Some(done_tx));

        let mut registry = self.batch_registry.lock().unwrap();
        let Some(scope) = registry.active.as_mut() else {
            return Err(runtime_error("internal error: no active seq/par scope"));
        };
        scope.commands.push(CollectedCommand {
            handle: handle.clone(),
            command,
        });
        Ok(handle)
    }

    fn finish_batch(&self) -> Result<String, Box<EvalAltResult>> {
        let (mode, commands) = {
            let mut registry = self.batch_registry.lock().unwrap();
            let scope = registry
                .active
                .take()
                .ok_or_else(|| runtime_error("internal error: no active seq/par scope"))?;
            (scope.mode, scope.commands)
        };

        let handles = commands
            .iter()
            .map(|command| command.handle.clone())
            .collect::<Vec<_>>();
        let mut deduped = Vec::new();
        let mut seen = BTreeSet::new();
        for handle in handles {
            if seen.insert(handle.clone()) {
                deduped.push(handle);
            }
        }

        let mut registry = self.batch_registry.lock().unwrap();
        registry.next_group_id += 1;
        let group_id = match mode {
            BatchMode::Sequence => format!("seq-{}", registry.next_group_id),
            BatchMode::Parallel => format!("par-{}", registry.next_group_id),
        };
        registry
            .groups
            .insert(group_id.clone(), BatchGroup { handles: deduped });
        drop(registry);

        self.send(ScriptCommand::SubmitBatch {
            mode: match mode {
                BatchMode::Sequence => BatchSubmitMode::Sequence,
                BatchMode::Parallel => BatchSubmitMode::Parallel,
            },
            items: commands
                .into_iter()
                .map(|item| BatchSubmissionItem {
                    handle: item.handle,
                    command: Box::new(item.command),
                })
                .collect(),
        })?;

        Ok(group_id)
    }

    fn wait_for_handle(&self, handle: String) -> Result<(), Box<EvalAltResult>> {
        self.wait_for_handles(vec![handle])
    }

    fn wait_for_handles(&self, handles: Vec<String>) -> Result<(), Box<EvalAltResult>> {
        let ids = self.expand_wait_handles(handles);
        if ids.is_empty() {
            return Ok(());
        }
        self.wait_for_animations(ids)
    }

    fn expand_wait_handles(&self, handles: Vec<String>) -> Vec<String> {
        let registry = self.batch_registry.lock().unwrap();
        let mut expanded = Vec::new();
        let mut seen = BTreeSet::new();
        for handle in handles {
            expand_wait_handle(&registry, &handle, &mut seen, &mut expanded);
        }
        expanded
    }

    fn cancel_handle(&self, handle: String) -> Result<(), Box<EvalAltResult>> {
        let ids = {
            let registry = self.batch_registry.lock().unwrap();
            let mut ids = Vec::new();
            let mut seen = BTreeSet::new();
            expand_wait_handle(&registry, &handle, &mut seen, &mut ids);
            ids
        };
        if ids.is_empty() {
            return Ok(());
        }
        self.send(ScriptCommand::CancelAnimations { ids })
    }

    fn read_text(&self, path: &str) -> Result<String, Box<EvalAltResult>> {
        let resolved = self.resolve_path(path);
        self.vfs.read_text(&resolved).map_err(vfs_to_rhai_error)
    }

    fn read_bytes(&self, path: &str) -> Result<Blob, Box<EvalAltResult>> {
        let resolved = self.resolve_path(path);
        self.vfs.read_bytes(&resolved).map_err(vfs_to_rhai_error)
    }

    fn request_choice(
        &self,
        pos: Position,
        prompt: String,
        options: Vec<ChoiceOption>,
    ) -> Result<StoredValue, Box<EvalAltResult>> {
        if self.checkpoint("choice", None, pos) == CheckpointDecision::ReplaySkip {
            return self.replay_input("choice");
        }

        match self.send_and_wait(|done| ScriptCommand::Choose {
            prompt,
            options,
            done,
        })? {
            ScriptResponse::Choice(value) => {
                self.record_input(value.clone());
                Ok(value)
            }
            ScriptResponse::Continue => Err(runtime_error(
                "engine returned unexpected continue response",
            )),
        }
    }

    fn wait_for_animations(&self, ids: Vec<String>) -> Result<(), Box<EvalAltResult>> {
        self.send_continue(|done| ScriptCommand::WaitAnimations { ids, done })
    }

    fn current_background_path(&self) -> Option<String> {
        self.scene_state
            .lock()
            .unwrap()
            .background
            .as_ref()
            .map(|background| background.path.clone())
    }

    fn character_names(&self) -> Vec<String> {
        load_character_catalog(&self.vfs)
            .map(|catalog| catalog.characters.into_keys().collect())
            .unwrap_or_default()
    }

    fn character_exists(&self, name: &str) -> bool {
        load_character_catalog(&self.vfs)
            .map(|catalog| catalog.characters.contains_key(name))
            .unwrap_or(false)
    }

    fn character_position(&self, actor_id: &str) -> Result<Vec2, Box<EvalAltResult>> {
        self.scene_state
            .lock()
            .unwrap()
            .character_positions
            .get(actor_id)
            .map(|value| Vec2::new(value[0], value[1]))
            .ok_or_else(|| runtime_error(format!("character actor `{actor_id}` is not shown")))
    }

    fn user_setting(&self, name: &str) -> Result<f32, Box<EvalAltResult>> {
        let settings = read_user_settings()
            .map_err(|err| runtime_error(format!("failed to read user settings: {err}")))?;
        match name {
            "bgm_volume" => Ok(settings.bgm_volume),
            "voice_volume" => Ok(settings.voice_volume),
            "sfx_volume" => Ok(settings.sfx_volume),
            _ => Err(runtime_error(format!("unknown setting `{name}`"))),
        }
    }

    fn set_user_setting(&self, name: &str, value: f32) -> Result<(), Box<EvalAltResult>> {
        let mut settings = read_user_settings()
            .map_err(|err| runtime_error(format!("failed to read user settings: {err}")))?;
        let value = value.clamp(0.0, 1.0);

        match name {
            "bgm_volume" => settings.bgm_volume = value,
            "voice_volume" => settings.voice_volume = value,
            "sfx_volume" => settings.sfx_volume = value,
            _ => return Err(runtime_error(format!("unknown setting `{name}`"))),
        }

        write_user_settings(&settings)
            .map_err(|err| runtime_error(format!("failed to write user settings: {err}")))?;
        self.send(ScriptCommand::ApplyUserSettings(settings))
    }

    fn save_exists(&self, slot: &str) -> Result<bool, Box<EvalAltResult>> {
        Ok(self.save_file_path(slot)?.exists())
    }

    fn save_game_with_scope(
        &self,
        slot: &str,
        resume_script: Option<String>,
        scope: BTreeMap<String, StoredValue>,
    ) -> Result<(), Box<EvalAltResult>> {
        let checkpoint_state = self.checkpoint.lock().unwrap().clone();
        let data = SaveGameData {
            version: 3,
            resume_script: resume_script.unwrap_or_else(|| self.current_script_path()),
            random_seed: *self.random_seed.lock().unwrap(),
            rng_state: Some(self.rng_state()),
            time_seed: *self.time_seed.lock().unwrap(),
            checkpoint: checkpoint_state.current.clone(),
            script_stack: self.script_stack.lock().unwrap().clone(),
            globals: self.globals.lock().unwrap().clone(),
            scope,
            input_log: checkpoint_state.input_log.clone(),
            scene: self.scene_state.lock().unwrap().clone(),
        };

        write_save_data_to_root(&self.save_root, slot, &data)
            .map_err(|err| runtime_error(format!("failed to write save slot `{slot}`: {err}")))
    }

    fn load_game(&self, slot: &str) -> Result<String, Box<EvalAltResult>> {
        let data = load_save_data_from_root(&self.save_root, slot)
            .map_err(|err| runtime_error(format!("failed to load save slot `{slot}`: {err}")))?;

        *self.script_stack.lock().unwrap() = data.script_stack.clone();
        *self.globals.lock().unwrap() = data.globals.clone();
        *self.scene_state.lock().unwrap() = data.scene.clone();
        *self.pending_scope_restore.lock().unwrap() = Some(data.scope.clone());
        *self.random_seed.lock().unwrap() = data.random_seed;
        *self.time_seed.lock().unwrap() = data.time_seed;
        let restored_rng = if data.checkpoint.is_none() {
            data.rng_state.as_ref().map(rng_from_state_snapshot)
        } else {
            None
        };
        *self.rng.lock().unwrap() =
            restored_rng.unwrap_or_else(|| Pcg32::seed_from_u64(data.random_seed));
        *self.checkpoint.lock().unwrap() = CheckpointState {
            current: data.checkpoint.clone(),
            ordinal: 0,
            input_log: Vec::new(),
            replay: data.checkpoint.clone().map(|target| ReplayState {
                target,
                input_log: data.input_log.clone(),
                input_cursor: 0,
            }),
        };
        self.send_continue(|done| ScriptCommand::RestoreSnapshot {
            snapshot: data.scene.clone(),
            done,
        })?;

        Ok(data
            .checkpoint
            .map(|checkpoint| checkpoint.script)
            .unwrap_or(data.resume_script))
    }

    fn set_global(&self, name: &str, value: Dynamic) -> Result<(), Box<EvalAltResult>> {
        let stored = dynamic_to_stored_value(value)?;
        self.globals
            .lock()
            .unwrap()
            .insert(name.to_string(), stored);
        Ok(())
    }

    fn get_global(&self, name: &str) -> Dynamic {
        self.globals
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(stored_value_to_dynamic)
            .unwrap_or(Dynamic::UNIT)
    }

    fn get_global_or(&self, name: &str, fallback: Dynamic) -> Dynamic {
        self.globals
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(stored_value_to_dynamic)
            .unwrap_or(fallback)
    }

    fn has_global(&self, name: &str) -> bool {
        self.globals.lock().unwrap().contains_key(name)
    }

    fn remove_global(&self, name: &str) -> bool {
        self.globals.lock().unwrap().remove(name).is_some()
    }

    fn clear_globals(&self) {
        self.globals.lock().unwrap().clear();
    }

    fn save_file_path(&self, slot: &str) -> Result<PathBuf, Box<EvalAltResult>> {
        slot_path_in(&self.save_root, slot)
            .map_err(|err| runtime_error(format!("failed to resolve save slot `{slot}`: {err}")))
    }

    fn resolve_texture(&self, name: &str) -> Result<ScreenTexture, Box<EvalAltResult>> {
        let texture = self
            .textures
            .resolve(name)
            .ok_or_else(|| runtime_error(format!("texture `{name}` is not defined")))?;
        Ok(ScreenTexture {
            path: texture.path.clone(),
            rect: texture.rect,
        })
    }

    fn resolve_image_path(&self, name: &str) -> Result<String, Box<EvalAltResult>> {
        let texture = self.resolve_texture(name)?;
        if texture.rect.is_some() {
            return Err(runtime_error(format!(
                "texture `{name}` is an atlas region and cannot be used where a full image is required"
            )));
        }
        Ok(texture.path)
    }

    fn resolve_music(&self, name: &str) -> Result<String, Box<EvalAltResult>> {
        self.audio
            .resolve_music(name)
            .map(|audio| audio.path.clone())
            .ok_or_else(|| runtime_error(format!("music `{name}` is not defined")))
    }

    fn resolve_voice(&self, name: &str) -> Result<String, Box<EvalAltResult>> {
        self.audio
            .resolve_voice(name)
            .map(|audio| audio.path.clone())
            .ok_or_else(|| runtime_error(format!("voice `{name}` is not defined")))
    }

    fn resolve_sfx(&self, name: &str) -> Result<String, Box<EvalAltResult>> {
        self.audio
            .resolve_sfx(name)
            .map(|audio| audio.path.clone())
            .ok_or_else(|| runtime_error(format!("sfx `{name}` is not defined")))
    }
}

pub fn spawn_script_runtime(
    commands: &mut bevy::prelude::Commands,
    vfs: Arc<HdpVfs>,
    scene_state: Arc<Mutex<SceneSnapshot>>,
    bootstrap: ScriptBootstrap,
) {
    let textures = match load_texture_catalog(&vfs) {
        Ok(catalog) => Arc::new(catalog),
        Err(err) => {
            error!("failed to load texture catalog: {err}");
            Arc::new(TextureCatalog::default())
        }
    };
    let audio = match load_audio_catalog(&vfs) {
        Ok(catalog) => Arc::new(catalog),
        Err(err) => {
            error!("failed to load audio catalog: {err}");
            Arc::new(AudioCatalog::default())
        }
    };
    let (command_tx, command_rx) = mpsc::channel();
    let current_script = Arc::new(Mutex::new(bootstrap.startup_script.clone()));
    let script_stack = Arc::new(Mutex::new(bootstrap.script_stack));
    let seed = bootstrap.random_seed.unwrap_or_else(new_random_seed);
    let random_seed = Arc::new(Mutex::new(seed));
    let time_seed = Arc::new(Mutex::new(
        bootstrap.time_seed.unwrap_or_else(new_time_seed),
    ));
    let restored_rng = if bootstrap.checkpoint.is_none() {
        bootstrap.rng_state.as_ref().map(rng_from_state_snapshot)
    } else {
        None
    };
    let rng = Arc::new(Mutex::new(
        restored_rng.unwrap_or_else(|| Pcg32::seed_from_u64(seed)),
    ));
    let checkpoint = Arc::new(Mutex::new(CheckpointState {
        current: bootstrap.checkpoint.clone(),
        ordinal: 0,
        input_log: Vec::new(),
        replay: bootstrap.checkpoint.clone().map(|target| ReplayState {
            target,
            input_log: bootstrap.input_log.clone(),
            input_cursor: 0,
        }),
    }));
    let pending_scope_restore = Arc::new(Mutex::new(None));
    let next_animation_id = Arc::new(Mutex::new(0));
    let batch_registry = Arc::new(Mutex::new(BatchRegistry::default()));
    let character_expressions = Arc::new(Mutex::new(BTreeMap::new()));
    let inline_dialogue_control = Arc::new(Mutex::new(InlineDialogueControl::default()));
    let startup_scope = bootstrap.scope.clone();

    commands.insert_resource(ScriptInbox(Mutex::new(command_rx)));
    commands.insert_resource(InlineDialogueControlResource(
        inline_dialogue_control.clone(),
    ));

    let host = ScriptHost {
        vfs,
        textures,
        audio,
        command_tx,
        current_script: current_script.clone(),
        script_stack: script_stack.clone(),
        globals: Arc::new(Mutex::new(bootstrap.globals)),
        checkpoint: checkpoint.clone(),
        pending_scope_restore,
        random_seed: random_seed.clone(),
        time_seed: time_seed.clone(),
        rng: rng.clone(),
        scene_state,
        next_animation_id,
        batch_registry,
        character_expressions,
        inline_dialogue_control,
        save_root: save_root_path(),
        web_execution: None,
    };

    commands.insert_resource(ScriptRuntimeState {
        current_script,
        script_stack,
        globals: host.globals.clone(),
        checkpoint,
        random_seed,
        rng,
        time_seed,
        save_root: host.save_root.clone(),
    });

    #[cfg(not(target_arch = "wasm32"))]
    let startup_script = bootstrap.startup_script;

    #[cfg(target_arch = "wasm32")]
    {
        let execution = Arc::new(Mutex::new(WebScriptExecution {
            responses: Vec::new(),
            cursor: 0,
            pending: None,
        }));
        let mut host = host;
        host.web_execution = Some(execution.clone());
        commands.insert_resource(WebScriptRuntime {
            host,
            startup_scope,
            execution,
            started: false,
            finished: false,
        });
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    thread::Builder::new()
        .name("hiraku-rhai".to_string())
        .spawn(move || run_script_loop(host, startup_script, startup_scope))
        .expect("failed to spawn script runtime thread");
}

#[cfg(target_arch = "wasm32")]
pub fn drive_web_script_runtime(runtime: Option<bevy::prelude::ResMut<WebScriptRuntime>>) {
    let Some(mut runtime) = runtime else {
        return;
    };
    if runtime.finished {
        return;
    }

    let execution_handle = runtime.execution.clone();
    let should_run = {
        let mut execution = execution_handle.lock().unwrap();
        if !runtime.started {
            runtime.started = true;
            true
        } else {
            let Some(pending) = execution.pending.as_ref() else {
                return;
            };
            match pending.try_recv() {
                Ok(response) => {
                    execution.responses.push(response);
                    execution.pending = None;
                    true
                }
                Err(mpsc::TryRecvError::Empty) => false,
                Err(mpsc::TryRecvError::Disconnected) => {
                    error!("script runtime stopped while waiting for an ECS response");
                    runtime.finished = true;
                    false
                }
            }
        }
    };

    if !should_run {
        return;
    }

    runtime.execution.lock().unwrap().cursor = 0;
    let startup_script = runtime.host.current_script_path();
    if run_script_loop(
        runtime.host.clone(),
        startup_script,
        runtime.startup_scope.clone(),
    ) == ScriptLoopOutcome::Finished
    {
        runtime.finished = true;
    }
}

pub fn save_runtime_slot(
    slot: &str,
    runtime_state: &ScriptRuntimeState,
    shared_state: &SceneSharedState,
) -> Result<(), StorageError> {
    save_runtime_slot_with_scope(slot, runtime_state, shared_state, BTreeMap::new())
}

pub fn save_runtime_slot_with_scope(
    slot: &str,
    runtime_state: &ScriptRuntimeState,
    shared_state: &SceneSharedState,
    scope: BTreeMap<String, StoredValue>,
) -> Result<(), StorageError> {
    let checkpoint_state = runtime_state.checkpoint.lock().unwrap().clone();
    let data = SaveGameData {
        version: 3,
        resume_script: runtime_state.current_script.lock().unwrap().clone(),
        random_seed: *runtime_state.random_seed.lock().unwrap(),
        rng_state: Some(rng_state_snapshot(&runtime_state.rng.lock().unwrap())),
        time_seed: *runtime_state.time_seed.lock().unwrap(),
        checkpoint: checkpoint_state.current,
        script_stack: runtime_state.script_stack.lock().unwrap().clone(),
        globals: runtime_state.globals.lock().unwrap().clone(),
        scope,
        input_log: checkpoint_state.input_log,
        scene: shared_state.0.lock().unwrap().clone(),
    };
    write_save_data_to_root(&runtime_state.save_root, slot, &data)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptLoopOutcome {
    Finished,
    Yielded,
}

fn run_script_loop(
    host: ScriptHost,
    startup_script: String,
    startup_scope: BTreeMap<String, StoredValue>,
) -> ScriptLoopOutcome {
    let mut engine = RhaiEngine::new();
    register_api(&mut engine, &host);

    let mut next_script = startup_script;
    let mut scope = scope_from_stored_values(startup_scope);

    loop {
        host.set_current_script_path(next_script.clone());
        host.reset_script_checkpoint_counter();

        let source = match host.vfs.read_text(&next_script) {
            Ok(source) => source,
            Err(err) => {
                error!("failed to read script `{}`: {err}", next_script);
                return ScriptLoopOutcome::Finished;
            }
        };

        let source = format!("{PRELUDE_SOURCE}\n{source}");
        match engine.eval_with_scope::<Dynamic>(&mut scope, &source) {
            Ok(_) => return ScriptLoopOutcome::Finished,
            Err(err) => {
                if is_web_runtime_yield(err.as_ref()) {
                    return ScriptLoopOutcome::Yielded;
                }
                if let Some(action) = extract_script_flow_action(err.as_ref()) {
                    match action {
                        ScriptFlowAction::Jump(target) => {
                            if let Some(restored_scope) =
                                host.pending_scope_restore.lock().unwrap().take()
                            {
                                scope = scope_from_stored_values(restored_scope);
                            }
                            if let Some(program) = compile_ir_script(&host, &target) {
                                host.set_current_script_path(target.clone());
                                host.reset_script_checkpoint_counter();
                                if let Err(err) = host.send(ScriptCommand::StartIr {
                                    path: target.clone(),
                                    program,
                                }) {
                                    error!("failed to hand off `{target}` to IR runtime: {err}");
                                }
                                return ScriptLoopOutcome::Finished;
                            }
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::Call(target) => {
                            host.script_stack.lock().unwrap().push(next_script.clone());
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::Save {
                            slot,
                            resume_script,
                        } => {
                            match scope_to_stored_values(&scope).and_then(|scope| {
                                host.save_game_with_scope(&slot, resume_script, scope)
                            }) {
                                Ok(()) => return ScriptLoopOutcome::Finished,
                                Err(err) => {
                                    error!("script `{}` failed to save: {err}", next_script);
                                    return ScriptLoopOutcome::Finished;
                                }
                            }
                        }
                        ScriptFlowAction::Return => {
                            let Some(target) = host.script_stack.lock().unwrap().pop() else {
                                error!(
                                    "script `{}` tried to return with an empty call stack",
                                    next_script
                                );
                                return ScriptLoopOutcome::Finished;
                            };
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::ReturnToTitle => return ScriptLoopOutcome::Finished,
                        ScriptFlowAction::EngineStopped => return ScriptLoopOutcome::Finished,
                    }
                }

                error!("script `{}` failed: {err}", next_script);
                return ScriptLoopOutcome::Finished;
            }
        }
    }
}

fn compile_ir_script(host: &ScriptHost, path: &str) -> Option<IrProgram> {
    let source = match host.vfs.read_text(path) {
        Ok(source) => source,
        Err(err) => {
            error!("failed to read candidate IR script `{path}`: {err}");
            return None;
        }
    };
    match compile_to_ir(path, &source) {
        Ok(program) => Some(program),
        Err(err) => {
            info!("keeping legacy runtime for `{path}`: {err}");
            None
        }
    }
}

fn command_suppressed_during_replay(command: &ScriptCommand) -> bool {
    matches!(
        command,
        ScriptCommand::Log(_)
            | ScriptCommand::StartIr { .. }
            | ScriptCommand::StartLegacy { .. }
            | ScriptCommand::SetBackground { .. }
            | ScriptCommand::ShowSprite { .. }
            | ScriptCommand::HideSprite { .. }
            | ScriptCommand::SetOverlay { .. }
            | ScriptCommand::Say { .. }
            | ScriptCommand::AwaitDialogueAdvance { .. }
            | ScriptCommand::SetDialogue { .. }
            | ScriptCommand::ClearDialogue
            | ScriptCommand::SetTextEffect(_)
            | ScriptCommand::ResetTextEffect
            | ScriptCommand::SetCamera { .. }
            | ScriptCommand::ApplyUserSettings(_)
            | ScriptCommand::ApplyUiStyle(_)
            | ScriptCommand::ResetUiStyle
            | ScriptCommand::ShowScreen { .. }
            | ScriptCommand::ShowOverlay { .. }
            | ScriptCommand::HideOverlay { .. }
            | ScriptCommand::WaitForScreenChoice { .. }
            | ScriptCommand::Choose { .. }
            | ScriptCommand::ShowCharacter { .. }
            | ScriptCommand::HideCharacter { .. }
            | ScriptCommand::JumpCharacter { .. }
            | ScriptCommand::ShakeCharacter { .. }
            | ScriptCommand::AnimateCharacter { .. }
            | ScriptCommand::PlayCustomEffect { .. }
            | ScriptCommand::RuleTransitionBg { .. }
            | ScriptCommand::MoveSprite { .. }
            | ScriptCommand::ScaleSprite { .. }
            | ScriptCommand::FadeSprite { .. }
            | ScriptCommand::Wait { .. }
            | ScriptCommand::WaitAnimations { .. }
            | ScriptCommand::Shake { .. }
            | ScriptCommand::PlayBgm { .. }
            | ScriptCommand::SetBgmVolume { .. }
            | ScriptCommand::FadeBgm { .. }
            | ScriptCommand::StopBgm
            | ScriptCommand::PlayVoice { .. }
            | ScriptCommand::StopVoice
            | ScriptCommand::PlaySfx { .. }
            | ScriptCommand::SubmitBatch { .. }
            | ScriptCommand::CancelAnimations { .. }
            | ScriptCommand::Exit
    )
}

fn register_api(engine: &mut RhaiEngine, host: &ScriptHost) {
    engine.set_default_tag(Dynamic::from(host.clone()));
    engine
        .register_custom_operator("@", 160)
        .expect("@ must remain available for the character dialogue operator");
    engine.register_type_with_name::<hiraku_engine::CharacterFlow>("CharacterFlow");
    engine.register_type_with_name::<hiraku_engine::CameraFlow>("CameraFlow");
    engine.register_type_with_name::<hiraku_engine::CameraHandle>("CameraHandle");
    engine.register_global_module(exported_module!(hiraku_engine::HirakuEngine).into());
    engine.register_fn("@", hiraku_engine::character_say);
    engine.register_global_module(exported_module!(rng::RNG).into());
    engine.register_global_module(exported_module!(time::Time).into());
    engine.register_static_module("ui", exported_module!(ui::Ui).into());
    let mut camera_module = Module::new();
    camera_module.set_var("camera", hiraku_engine::camera(host.clone()));
    engine.register_global_module(camera_module.into());
}

fn new_random_seed() -> u64 {
    rand::rng().next_u64()
}

fn new_time_seed() -> i64 {
    ::time::OffsetDateTime::now_utc().unix_timestamp()
}

fn rng_state_snapshot(rng: &Pcg32) -> RngState {
    RngState {
        state: rng.state(),
        stream: rng.stream(),
    }
}

fn rng_from_state_snapshot(state: &RngState) -> Pcg32 {
    Pcg32::from_state(state.state, state.stream)
}

fn checkpoint_matches(left: &SaveCheckpoint, right: &SaveCheckpoint) -> bool {
    left.script == right.script && left.ordinal == right.ordinal && left.kind == right.kind
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

    #[test]
    fn rng_state_round_trips() {
        let mut rng = Pcg32::seed_from_u64(42);
        let _ = rng.random::<u64>();
        let state = rng_state_snapshot(&rng);
        let mut restored = rng_from_state_snapshot(&state);

        assert_eq!(rng.random::<u64>(), restored.random::<u64>());
    }

    #[test]
    fn replay_suppresses_screen_choice_waits() {
        let (done, _response) = mpsc::channel();
        assert!(command_suppressed_during_replay(
            &ScriptCommand::WaitForScreenChoice { done }
        ));
    }

    #[test]
    fn ir_commands_map_to_scene_commands_without_callbacks() {
        let command = script_command_from_ir(
            IrCommand::ShowCharacter {
                actor_id: "ema".to_string(),
                character_name: "Ema".to_string(),
                expressions: vec!["smile".to_string()],
                position: [12.0, -4.0],
                scale: 0.5,
            },
            None,
        )
        .unwrap();

        assert!(matches!(
            command,
            ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                expressions,
                position,
                scale,
                fade: None,
                animation_id: None,
                done: None,
            } if actor_id == "ema"
                && character_name == "Ema"
                && expressions == vec!["smile".to_string()]
                && position == Vec2::new(12.0, -4.0)
                && scale == 0.5
        ));
    }

    #[test]
    fn embedded_prelude_compiles() {
        let mut engine = RhaiEngine::new();
        engine.register_custom_operator("@", 160).unwrap();
        engine.register_fn("@", |left: INT, right: INT| left + right);
        engine.compile(PRELUDE_SOURCE).unwrap();
        assert_eq!(engine.eval_expression::<INT>("2 @ 3").unwrap(), 5);
    }

    #[test]
    fn fluent_character_syntax_compiles() {
        let mut engine = RhaiEngine::new();
        engine.register_custom_operator("@", 160).unwrap();
        engine.register_type_with_name::<hiraku_engine::CharacterFlow>("CharacterFlow");
        engine.register_type_with_name::<hiraku_engine::CameraFlow>("CameraFlow");
        engine.register_type_with_name::<hiraku_engine::CameraHandle>("CameraHandle");
        engine.register_global_module(exported_module!(hiraku_engine::HirakuEngine).into());
        engine.register_fn("@", hiraku_engine::character_say);

        engine
            .compile(
                "char(\"alice\").e(\"open_mouth\").e(\"happy_face\").at(\"left\").scale(0.5) @ \"Hello\";\nchar(\"alice\").e(\"neutral\").finish();\ncamera.blur().duration(2).ease(\"ease_out\").finish();\ncamera.zoom(1.5).duration(1).zoom_at(1, 2);\npar(|| { camera.blur(0).duration(2).ease(\"ease_in\"); camera.zoom(1).duration(1).ease(\"bounce\"); });",
            )
            .unwrap();
    }
}

fn scope_from_stored_values(values: BTreeMap<String, StoredValue>) -> Scope<'static> {
    let mut scope = Scope::new();
    for (name, value) in values {
        scope.push_dynamic(name, stored_value_to_dynamic(value));
    }
    scope
}

fn scope_to_stored_values(
    scope: &Scope,
) -> Result<BTreeMap<String, StoredValue>, Box<EvalAltResult>> {
    let mut values = BTreeMap::new();
    for (name, _, value) in scope.iter() {
        if name == "camera" {
            continue;
        }
        values.insert(name.to_string(), dynamic_to_stored_value(value)?);
    }
    Ok(values)
}

fn extract_script_flow_action(error: &EvalAltResult) -> Option<ScriptFlowAction> {
    match error {
        EvalAltResult::ErrorRuntime(value, _) => extract_signal_string(value),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => extract_script_flow_action(inner),
        _ => None,
    }
}

fn is_web_runtime_yield(error: &EvalAltResult) -> bool {
    match error {
        EvalAltResult::ErrorRuntime(value, _) => value
            .clone()
            .try_cast::<ImmutableString>()
            .is_some_and(|value| value == WEB_RUNTIME_YIELD_SIGNAL),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => is_web_runtime_yield(inner),
        _ => false,
    }
}

fn extract_signal_string(value: &Dynamic) -> Option<ScriptFlowAction> {
    if !value.is_string() {
        return None;
    }

    let payload = value.clone_cast::<ImmutableString>();
    let payload = payload.as_str();

    if let Some(target) = payload.strip_prefix(JUMP_SCRIPT_SIGNAL) {
        return Some(ScriptFlowAction::Jump(target.to_string()));
    }
    if let Some(target) = payload.strip_prefix(CALL_SCRIPT_SIGNAL) {
        return Some(ScriptFlowAction::Call(target.to_string()));
    }
    if let Some(payload) = payload.strip_prefix(SAVE_SCRIPT_SIGNAL) {
        let (slot, resume_script) = payload
            .split_once('\n')
            .map(|(slot, resume_script)| (slot, Some(resume_script.to_string())))
            .unwrap_or((payload, None));
        return Some(ScriptFlowAction::Save {
            slot: slot.to_string(),
            resume_script,
        });
    }
    if payload == RETURN_SCRIPT_SIGNAL {
        return Some(ScriptFlowAction::Return);
    }
    if payload == RETURN_TO_TITLE_SIGNAL {
        return Some(ScriptFlowAction::ReturnToTitle);
    }
    if payload == ENGINE_STOPPED_SIGNAL {
        return Some(ScriptFlowAction::EngineStopped);
    }

    None
}

fn jump_script_signal(target: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        format!("{JUMP_SCRIPT_SIGNAL}{target}").into(),
        Position::NONE,
    ))
}

fn call_script_signal(target: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        format!("{CALL_SCRIPT_SIGNAL}{target}").into(),
        Position::NONE,
    ))
}

fn save_script_signal(slot: String, resume_script: Option<String>) -> Box<EvalAltResult> {
    let payload = match resume_script {
        Some(resume_script) => format!("{SAVE_SCRIPT_SIGNAL}{slot}\n{resume_script}"),
        None => format!("{SAVE_SCRIPT_SIGNAL}{slot}"),
    };
    Box::new(EvalAltResult::ErrorRuntime(payload.into(), Position::NONE))
}

fn return_script_signal() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        RETURN_SCRIPT_SIGNAL.into(),
        Position::NONE,
    ))
}

fn return_to_title_signal() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        RETURN_TO_TITLE_SIGNAL.into(),
        Position::NONE,
    ))
}

fn engine_stopped_signal() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        ENGINE_STOPPED_SIGNAL.into(),
        Position::NONE,
    ))
}

fn web_runtime_yield_signal() -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        WEB_RUNTIME_YIELD_SIGNAL.into(),
        Position::NONE,
    ))
}

fn parse_choice_options(options: Array) -> Result<Vec<ChoiceOption>, Box<EvalAltResult>> {
    if options.is_empty() {
        return Err(runtime_error("choice requires at least one option"));
    }

    options
        .into_iter()
        .map(parse_choice_option)
        .collect::<Result<Vec<_>, _>>()
}

fn say_with_inline_commands(
    ctx: &NativeCallContext,
    host: &ScriptHost,
    speaker: String,
    text: String,
) -> Result<(), Box<EvalAltResult>> {
    let kind = if speaker.is_empty() { "narrate" } else { "say" };
    if host.checkpoint(kind, None, ctx.call_position()) == CheckpointDecision::ReplaySkip {
        return Ok(());
    }

    let chunks = parse_inline_dialogue_chunks(&text)?;
    if chunks
        .iter()
        .all(|chunk| matches!(chunk, InlineDialogueChunk::Text(_)))
    {
        if host.is_batch_mode() {
            let _ =
                run_blocking_or_collected(host, "say", |animation_id, done| ScriptCommand::Say {
                    speaker,
                    text,
                    animation_id,
                    done,
                })?;
            return Ok(());
        }

        return host.send_continue(|done| ScriptCommand::Say {
            speaker,
            text,
            animation_id: None,
            done: Some(done),
        });
    }

    if host.is_batch_mode() {
        return Err(runtime_error(
            "inline dialogue commands are not supported inside seq/par",
        ));
    }

    host.begin_inline_dialogue();
    let mut rendered = String::new();
    let mut saw_text = false;

    let result = (|| -> Result<(), Box<EvalAltResult>> {
        for chunk in chunks {
            match chunk {
                InlineDialogueChunk::Text(segment) => {
                    if segment.is_empty() {
                        continue;
                    }
                    let reveal_from = rendered.chars().count();
                    rendered.push_str(&segment);
                    saw_text = true;
                    if host.inline_skip_requested() {
                        host.set_dialogue(speaker.clone(), rendered.clone())?;
                    } else {
                        host.reveal_dialogue_tail(speaker.clone(), rendered.clone(), reveal_from)?;
                    }
                }
                InlineDialogueChunk::Command(code) => {
                    let _ = ctx.engine().eval_expression::<Dynamic>(&code)?;
                }
            }
        }

        if saw_text {
            host.end_inline_dialogue();
            host.send_continue(|done| ScriptCommand::AwaitDialogueAdvance { done })
        } else {
            Ok(())
        }
    })();

    host.end_inline_dialogue();
    result
}

fn parse_inline_dialogue_chunks(
    text: &str,
) -> Result<Vec<InlineDialogueChunk>, Box<EvalAltResult>> {
    let mut chunks = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find("@{") {
        let start = cursor + relative_start;
        if start > cursor {
            chunks.push(InlineDialogueChunk::Text(text[cursor..start].to_string()));
        }

        let command_start = start + 2;
        let command_end = find_inline_command_end(text, command_start)?;
        let code = text[command_start..command_end].trim().to_string();
        if code.is_empty() {
            return Err(runtime_error("inline dialogue command cannot be empty"));
        }
        chunks.push(InlineDialogueChunk::Command(code));
        cursor = command_end + 1;
    }

    if cursor < text.len() {
        chunks.push(InlineDialogueChunk::Text(text[cursor..].to_string()));
    }

    if chunks.is_empty() {
        chunks.push(InlineDialogueChunk::Text(String::new()));
    }

    Ok(chunks)
}

fn find_inline_command_end(text: &str, start: usize) -> Result<usize, Box<EvalAltResult>> {
    let mut depth = 1usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(start + offset);
                }
            }
            _ => {}
        }
    }

    Err(runtime_error("unterminated inline dialogue command"))
}

fn parse_ui_style_patch(options: Map) -> Result<UiStylePatch, Box<EvalAltResult>> {
    let input: UiStylePatchInput = from_dynamic(Dynamic::from_map(options), "ui style options")?;
    Ok(input.into())
}

pub(crate) trait ScreenTextureResolver {
    fn resolve_screen_texture(&self, name: &str) -> Result<ScreenTexture, Box<EvalAltResult>>;
}

impl ScreenTextureResolver for ScriptHost {
    fn resolve_screen_texture(&self, name: &str) -> Result<ScreenTexture, Box<EvalAltResult>> {
        self.resolve_texture(name)
    }
}

impl ScreenTextureResolver for TextureCatalog {
    fn resolve_screen_texture(&self, name: &str) -> Result<ScreenTexture, Box<EvalAltResult>> {
        let definition = self.resolve(name).ok_or_else(|| {
            runtime_error(format!("texture `{name}` was not found in the catalog"))
        })?;
        Ok(ScreenTexture {
            path: definition.path.clone(),
            rect: definition.rect,
        })
    }
}

pub(crate) fn parse_screen_spec(
    resolver: &impl ScreenTextureResolver,
    mut screen: Map,
) -> Result<ScreenSpec, Box<EvalAltResult>> {
    let title = take_optional_string(&mut screen, "title");
    let panel = take_optional_bool(&mut screen, "panel")?.unwrap_or(true);
    let width = take_optional_number(&mut screen, "width")?;
    let background_texture = take_optional_string(&mut screen, "background_texture")
        .map(|name| resolver.resolve_screen_texture(&name))
        .transpose()?;
    let xalign = take_optional_number(&mut screen, "xalign")?.unwrap_or(0.5);
    let yalign = take_optional_number(&mut screen, "yalign")?.unwrap_or(0.5);
    let padding = take_optional_number(&mut screen, "padding")?.unwrap_or(24.0);
    let gap = take_optional_number(&mut screen, "gap")?.unwrap_or(16.0);
    let overlay = take_optional_rgba(&mut screen, "overlay")?;
    let background = take_optional_rgba(&mut screen, "background")?;
    let border = take_optional_rgba(&mut screen, "border")?;
    let children = take_required_array(&mut screen, "children")?
        .into_iter()
        .map(|node| parse_screen_node(resolver, node))
        .collect::<Result<Vec<_>, _>>()?;

    if !screen.is_empty() {
        let unknown = screen.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(runtime_error(format!(
            "unknown screen option(s): {unknown}"
        )));
    }

    Ok(ScreenSpec {
        title,
        panel,
        width: width.map(|value| value as f32),
        background_texture,
        xalign: xalign as f32,
        yalign: yalign as f32,
        padding: padding as f32,
        gap: gap as f32,
        overlay,
        background,
        border,
        children,
    })
}

fn parse_screen_node(
    resolver: &impl ScreenTextureResolver,
    value: Dynamic,
) -> Result<ScreenNode, Box<EvalAltResult>> {
    let mut node = value
        .try_cast::<Map>()
        .ok_or_else(|| runtime_error("screen nodes must be maps"))?;
    let node_type = take_required_string(&mut node, "type")?;

    match node_type.as_str() {
        "text" => {
            let text = take_required_string(&mut node, "text")?;
            let size = take_optional_number(&mut node, "size")?.unwrap_or(26.0) as f32;
            let color = take_optional_rgba(&mut node, "color")?;
            let align = take_optional_number(&mut node, "align")?.map(|value| value as f32);
            let layout = take_screen_layout(&mut node)?;
            ensure_no_unknown_options("text", &node)?;
            Ok(ScreenNode::Text(TextNode {
                text,
                size,
                color,
                align,
                layout,
            }))
        }
        "button" => {
            let text = take_required_string(&mut node, "text")?;
            let value = node
                .remove("value")
                .map(dynamic_to_stored_value)
                .transpose()?;
            let action = take_optional_string(&mut node, "action");
            if value.is_none() && action.is_none() {
                return Err(runtime_error("screen button requires `value` or `action`"));
            }
            let enabled = take_optional_bool(&mut node, "enabled")?.unwrap_or(true);
            let size = take_optional_number(&mut node, "size")?.unwrap_or(28.0) as f32;
            let color = take_optional_rgba(&mut node, "color")?;
            let hovered_color = take_optional_rgba(&mut node, "hovered_color")?;
            let pressed_color = take_optional_rgba(&mut node, "pressed_color")?;
            let insensitive_color = take_optional_rgba(&mut node, "insensitive_color")?;
            let background = take_optional_rgba(&mut node, "background")?;
            let border = take_optional_rgba(&mut node, "border")?;
            let hovered_background = take_optional_rgba(&mut node, "hovered_background")?;
            let pressed_background = take_optional_rgba(&mut node, "pressed_background")?;
            let align = take_optional_number(&mut node, "align")?.map(|value| value as f32);
            let padding = take_optional_number(&mut node, "padding")?.map(|value| value as f32);
            let padding_x = take_optional_number(&mut node, "padding_x")?
                .map(|value| value as f32)
                .or(padding);
            let padding_y = take_optional_number(&mut node, "padding_y")?
                .map(|value| value as f32)
                .or(padding);
            let border_width =
                take_optional_number(&mut node, "border_width")?.map(|value| value as f32);
            let radius = take_optional_number(&mut node, "radius")?.map(|value| value as f32);
            let layout = take_screen_layout(&mut node)?;
            ensure_no_unknown_options("button", &node)?;
            Ok(ScreenNode::Button(ButtonNode {
                text,
                value,
                action,
                enabled,
                size,
                color,
                hovered_color,
                pressed_color,
                insensitive_color,
                background,
                border,
                hovered_background,
                pressed_background,
                align,
                padding_x,
                padding_y,
                border_width,
                radius,
                layout,
            }))
        }
        "image" => {
            let texture = take_required_string(&mut node, "texture")?;
            let layout = take_screen_layout(&mut node)?;
            ensure_no_unknown_options("image", &node)?;
            Ok(ScreenNode::Image(ScreenImageNode {
                texture: resolver.resolve_screen_texture(&texture)?,
                layout,
            }))
        }
        "image_button" => {
            let texture = take_required_string(&mut node, "texture")?;
            let hovered_texture = take_optional_string(&mut node, "hovered_texture")
                .map(|name| resolver.resolve_screen_texture(&name))
                .transpose()?;
            let hovered_layout = take_optional_screen_layout(&mut node, "hovered_layout")?;
            let value = node
                .remove("value")
                .map(dynamic_to_stored_value)
                .transpose()?
                .ok_or_else(|| runtime_error("image_button requires `value`"))?;
            let enabled = take_optional_bool(&mut node, "enabled")?.unwrap_or(true);
            let hovered_when_disabled =
                take_optional_bool(&mut node, "hovered_when_disabled")?.unwrap_or(false);
            let layout = take_screen_layout(&mut node)?;
            ensure_no_unknown_options("image_button", &node)?;
            Ok(ScreenNode::ImageButton(ScreenImageButtonNode {
                texture: resolver.resolve_screen_texture(&texture)?,
                hovered_texture,
                hovered_layout,
                value,
                enabled,
                hovered_when_disabled,
                layout,
            }))
        }
        "bar" => {
            let value = take_optional_number(&mut node, "value")?.unwrap_or(0.0) as f32;
            let min = take_optional_number(&mut node, "min")?.unwrap_or(0.0) as f32;
            let max = take_optional_number(&mut node, "max")?.unwrap_or(1.0) as f32;
            let width = take_optional_number(&mut node, "width")?.unwrap_or(320.0) as f32;
            let height = take_optional_number(&mut node, "height")?.unwrap_or(18.0) as f32;
            let background = take_optional_rgba(&mut node, "background")?;
            let fill = take_optional_rgba(&mut node, "fill")?;
            let border = take_optional_rgba(&mut node, "border")?;
            ensure_no_unknown_options("bar", &node)?;
            Ok(ScreenNode::Bar(BarNode {
                value,
                min,
                max,
                width,
                height,
                background,
                fill,
                border,
            }))
        }
        "vbox" | "hbox" | "frame" => {
            let gap = take_optional_number(&mut node, "gap")?.unwrap_or(12.0) as f32;
            let padding = take_optional_number(&mut node, "padding")?.unwrap_or(0.0) as f32;
            let background = take_optional_rgba(&mut node, "background")?;
            let border = take_optional_rgba(&mut node, "border")?;
            let justify = take_optional_string(&mut node, "justify");
            let align_items = take_optional_string(&mut node, "align_items");
            let layout = take_screen_layout(&mut node)?;
            let children = take_required_array(&mut node, "children")?
                .into_iter()
                .map(|child| parse_screen_node(resolver, child))
                .collect::<Result<Vec<_>, _>>()?;
            ensure_no_unknown_options(&node_type, &node)?;
            let container = ContainerNode {
                gap,
                padding,
                background,
                border,
                justify,
                align_items,
                layout,
                children,
            };
            if node_type == "vbox" || node_type == "frame" {
                Ok(ScreenNode::Column(container))
            } else {
                Ok(ScreenNode::Row(container))
            }
        }
        "spacer" => {
            let width = take_optional_number(&mut node, "width")?.unwrap_or(0.0) as f32;
            let height = take_optional_number(&mut node, "height")?.unwrap_or(0.0) as f32;
            ensure_no_unknown_options("spacer", &node)?;
            Ok(ScreenNode::Spacer(SpacerNode { width, height }))
        }
        other => Err(runtime_error(format!("unknown screen node type `{other}`"))),
    }
}

fn parse_custom_effect_options(
    host: &ScriptHost,
    mut options: Map,
) -> Result<CustomEffectOptions, Box<EvalAltResult>> {
    let duration = options
        .remove("duration")
        .or_else(|| options.remove("ms"))
        .ok_or_else(|| runtime_error("effect options require `duration` or `ms`"))?;
    let duration = duration_from_dynamic(duration)?;

    let current_background = host.current_background_path();

    let from_path = match take_optional_string(&mut options, "from") {
        Some(texture) => host.resolve_image_path(&texture)?,
        None => current_background.clone().ok_or_else(|| {
            runtime_error("effect options require `from` or an existing background")
        })?,
    };
    let to_path = match take_optional_string(&mut options, "to") {
        Some(texture) => host.resolve_image_path(&texture)?,
        None => from_path.clone(),
    };
    let rule_path = match take_optional_string(&mut options, "rule") {
        Some(texture) => host.resolve_image_path(&texture)?,
        None => from_path.clone(),
    };
    let aux0_path = match take_optional_string(&mut options, "tex0") {
        Some(texture) => host.resolve_image_path(&texture)?,
        None => from_path.clone(),
    };
    let aux1_path = match take_optional_string(&mut options, "tex1") {
        Some(texture) => host.resolve_image_path(&texture)?,
        None => from_path.clone(),
    };

    let mode = take_optional::<f32>(&mut options, "mode")?.unwrap_or(0.0);
    let commit_to_bg = take_optional::<bool>(&mut options, "commit_to_bg")?.unwrap_or(false);

    let p0 = take_vec4(&mut options, "p0")?;
    let p1 = take_vec4(&mut options, "p1")?;
    let p2 = take_vec4(&mut options, "p2")?;
    let p3 = take_vec4(&mut options, "p3")?;

    if !options.is_empty() {
        let unknown = options.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(runtime_error(format!(
            "unknown effect option(s): {unknown}"
        )));
    }

    Ok(CustomEffectOptions {
        from_path,
        to_path,
        rule_path,
        aux0_path,
        aux1_path,
        duration,
        mode,
        p0,
        p1,
        p2,
        p3,
        commit_to_bg,
    })
}

fn parse_text_effect_spec(options: Map) -> Result<DialogueTextEffectSpec, Box<EvalAltResult>> {
    let input: DialogueTextEffectInput =
        from_dynamic(Dynamic::from_map(options), "text effect options")?;
    Ok(input.into())
}

fn parse_character_animation_keyframes(
    keyframes: Array,
    current: Vec2,
) -> Result<Vec<ResolvedCharacterKeyframe>, Box<EvalAltResult>> {
    if keyframes.is_empty() {
        return Err(runtime_error("animate requires at least one keyframe"));
    }
    let inputs: Vec<CharacterAnimationKeyframeInput> =
        from_dynamic(Dynamic::from_array(keyframes), "animate keyframes")?;

    let mut resolved = Vec::with_capacity(inputs.len());
    let mut previous_time = 0.0f64;
    let mut cursor = current;

    for input in inputs {
        if input.time < 0.0 {
            return Err(runtime_error("animate keyframe time cannot be negative"));
        }
        if input.time < previous_time {
            return Err(runtime_error(
                "animate keyframes must be sorted by time ascending",
            ));
        }

        let x = input.x.map(f64_to_f32);
        let y = input.y.map(f64_to_f32);
        let dx = input.dx.map(f64_to_f32);
        let dy = input.dy.map(f64_to_f32);
        let position = Vec2::new(
            x.unwrap_or(cursor.x + dx.unwrap_or(0.0)),
            y.unwrap_or(cursor.y + dy.unwrap_or(0.0)),
        );
        let ease = parse_character_ease(input.ease.as_deref().unwrap_or("linear"))?;

        resolved.push(ResolvedCharacterKeyframe {
            time: input.time as f32,
            position,
            ease,
        });

        previous_time = input.time;
        cursor = position;
    }

    Ok(resolved)
}

fn parse_character_ease(name: &str) -> Result<CharacterEase, Box<EvalAltResult>> {
    match name {
        "linear" => Ok(CharacterEase::Linear),
        "ease" => Ok(CharacterEase::Ease),
        "ease_in" | "easein" => Ok(CharacterEase::EaseIn),
        "ease_out" | "easeout" => Ok(CharacterEase::EaseOut),
        "ease_in_out" | "easeinout" => Ok(CharacterEase::EaseInOut),
        "bounce" => Ok(CharacterEase::Bounce),
        other => Err(runtime_error(format!("unknown animation ease `{other}`"))),
    }
}

fn from_dynamic<T>(value: Dynamic, context: &str) -> Result<T, Box<EvalAltResult>>
where
    T: DeserializeOwned,
{
    rhai::serde::from_dynamic(&value)
        .map_err(|err| runtime_error(format!("invalid {context}: {err}")))
}

fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

fn rgba_to_f32(value: [f64; 4]) -> [f32; 4] {
    [
        value[0] as f32,
        value[1] as f32,
        value[2] as f32,
        value[3] as f32,
    ]
}

fn take_optional<T>(options: &mut Map, key: &str) -> Result<Option<T>, Box<EvalAltResult>>
where
    T: DeserializeOwned,
{
    options
        .remove(key)
        .map(|value| from_dynamic(value, key))
        .transpose()
}

fn take_optional_string(options: &mut Map, key: &str) -> Option<String> {
    take_optional::<String>(options, key).ok().flatten()
}

fn take_required_string(options: &mut Map, key: &str) -> Result<String, Box<EvalAltResult>> {
    take_optional_string(options, key)
        .ok_or_else(|| runtime_error(format!("missing required string option `{key}`")))
}

fn take_required_array(options: &mut Map, key: &str) -> Result<Array, Box<EvalAltResult>> {
    options
        .remove(key)
        .and_then(|value| value.try_cast::<Array>())
        .ok_or_else(|| runtime_error(format!("missing required array option `{key}`")))
}

fn take_optional_number(options: &mut Map, key: &str) -> Result<Option<f64>, Box<EvalAltResult>> {
    take_optional::<f64>(options, key)
}

fn take_optional_bool(options: &mut Map, key: &str) -> Result<Option<bool>, Box<EvalAltResult>> {
    take_optional::<bool>(options, key)
}

fn take_screen_layout(options: &mut Map) -> Result<ScreenLayout, Box<EvalAltResult>> {
    Ok(ScreenLayout {
        width: take_optional_number(options, "width")?.map(|value| value as f32),
        height: take_optional_number(options, "height")?.map(|value| value as f32),
        width_percent: take_optional_number(options, "width_percent")?.map(|value| value as f32),
        height_percent: take_optional_number(options, "height_percent")?.map(|value| value as f32),
        min_width: take_optional_number(options, "min_width")?.map(|value| value as f32),
        left: take_optional_number(options, "left")?.map(|value| value as f32),
        left_percent: take_optional_number(options, "left_percent")?.map(|value| value as f32),
        right: take_optional_number(options, "right")?.map(|value| value as f32),
        right_percent: take_optional_number(options, "right_percent")?.map(|value| value as f32),
        top: take_optional_number(options, "top")?.map(|value| value as f32),
        top_percent: take_optional_number(options, "top_percent")?.map(|value| value as f32),
        bottom: take_optional_number(options, "bottom")?.map(|value| value as f32),
        bottom_percent: take_optional_number(options, "bottom_percent")?.map(|value| value as f32),
    })
}

fn take_optional_screen_layout(
    options: &mut Map,
    key: &str,
) -> Result<Option<ScreenLayout>, Box<EvalAltResult>> {
    let Some(value) = options.remove(key) else {
        return Ok(None);
    };
    let mut layout = value
        .try_cast::<Map>()
        .ok_or_else(|| runtime_error(format!("`{key}` must be a map")))?;
    let result = take_screen_layout(&mut layout)?;
    ensure_no_unknown_options(key, &layout)?;
    Ok(Some(result))
}

fn ensure_no_unknown_options(kind: &str, options: &Map) -> Result<(), Box<EvalAltResult>> {
    if options.is_empty() {
        return Ok(());
    }

    let unknown = options.keys().cloned().collect::<Vec<_>>().join(", ");
    Err(runtime_error(format!(
        "unknown {kind} option(s): {unknown}"
    )))
}

fn take_optional_rgba(
    options: &mut Map,
    key: &str,
) -> Result<Option<[f32; 4]>, Box<EvalAltResult>> {
    let Some(value) = options.remove(key) else {
        return Ok(None);
    };

    let array: Vec<f64> = from_dynamic(value, key)?;
    if array.len() != 3 && array.len() != 4 {
        return Err(runtime_error(format!(
            "ui style option `{key}` must contain three or four numbers"
        )));
    }

    let alpha = if array.len() == 4 {
        array[3] as f32
    } else {
        1.0
    };

    Ok(Some([
        array[0] as f32,
        array[1] as f32,
        array[2] as f32,
        alpha,
    ]))
}

fn take_vec4(options: &mut Map, key: &str) -> Result<Vec4, Box<EvalAltResult>> {
    let Some(value) = options.remove(key) else {
        return Ok(Vec4::ZERO);
    };

    let array: Vec<f64> = from_dynamic(value, key)?;
    if array.len() != 4 {
        return Err(runtime_error(format!(
            "effect option `{key}` must contain exactly four numbers"
        )));
    }

    Ok(Vec4::new(
        array[0] as f32,
        array[1] as f32,
        array[2] as f32,
        array[3] as f32,
    ))
}

fn duration_from_dynamic(value: Dynamic) -> Result<Duration, Box<EvalAltResult>> {
    if value.is::<INT>() {
        return duration_from_millis(value.cast::<INT>());
    }
    if value.is::<FLOAT>() {
        return duration_from_millis(value.cast::<FLOAT>() as i64);
    }

    Err(runtime_error("duration must be a number"))
}

fn reject_known_unsupported_audio_path(path: &str) -> Result<(), Box<EvalAltResult>> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    if matches!(extension.as_deref(), Some("opus")) {
        return Err(runtime_error(
            "`.opus` audio is not supported by the current Bevy audio build; convert it to `.ogg`, `.wav`, `.mp3`, or `.flac` first",
        ));
    }

    Ok(())
}

fn parse_choice_option(option: Dynamic) -> Result<ChoiceOption, Box<EvalAltResult>> {
    match from_dynamic::<ChoiceOptionInput>(
        option,
        "choice option; expected a string or #{ text: ..., value: ... }",
    )? {
        ChoiceOptionInput::Text(text) => Ok(ChoiceOption {
            value: StoredValue::String(text.clone()),
            text,
        }),
        ChoiceOptionInput::Map(option) => {
            let value = option
                .value
                .map(dynamic_to_stored_value)
                .transpose()?
                .unwrap_or_else(|| StoredValue::String(option.text.clone()));
            Ok(ChoiceOption {
                text: option.text,
                value,
            })
        }
    }
}

fn parse_animation_ids(ids: Array) -> Result<Vec<String>, Box<EvalAltResult>> {
    from_dynamic(Dynamic::from_array(ids), "animation handle array")
}

fn expand_wait_handle(
    registry: &BatchRegistry,
    handle: &str,
    seen: &mut BTreeSet<String>,
    expanded: &mut Vec<String>,
) {
    if !seen.insert(handle.to_string()) {
        return;
    }

    if let Some(group) = registry.groups.get(handle) {
        for nested in &group.handles {
            expand_wait_handle(registry, nested, seen, expanded);
        }
        return;
    }

    expanded.push(handle.to_string());
}

fn run_blocking_or_collected<F>(
    host: &ScriptHost,
    kind: &str,
    build: F,
) -> Result<Dynamic, Box<EvalAltResult>>
where
    F: FnOnce(Option<String>, Option<mpsc::Sender<ScriptResponse>>) -> ScriptCommand,
{
    if host.is_batch_mode() {
        Ok(host.collect_command(kind, build)?.into())
    } else if host.inline_skip_requested() {
        let handle = host.next_animation_id(kind);
        let (done_tx, _done_rx) = mpsc::channel();
        host.send(build(Some(handle), Some(done_tx)))?;
        Ok(Dynamic::UNIT)
    } else if host.is_inline_dialogue_active() {
        let handle = host.next_animation_id(kind);
        let (done_tx, done_rx) = mpsc::channel();
        host.set_inline_current_handle(Some(handle.clone()));
        host.send(build(Some(handle), Some(done_tx)))?;
        let response = host.wait_for_response(done_rx);
        host.set_inline_current_handle(None);
        match response? {
            ScriptResponse::Continue => Ok(Dynamic::UNIT),
            ScriptResponse::Choice(_) => {
                Err(runtime_error("engine returned unexpected choice response"))
            }
        }
    } else {
        host.send_continue(|done| build(None, Some(done)))?;
        Ok(Dynamic::UNIT)
    }
}

fn dynamic_to_stored_value(value: Dynamic) -> Result<StoredValue, Box<EvalAltResult>> {
    if value.is::<bool>() {
        return Ok(StoredValue::Bool(value.cast::<bool>()));
    }
    if value.is::<INT>() {
        return Ok(StoredValue::Int(value.cast::<INT>()));
    }
    if value.is::<FLOAT>() {
        return Ok(StoredValue::Float(value.cast::<FLOAT>()));
    }
    if value.is_string() {
        return Ok(StoredValue::String(
            value.cast::<ImmutableString>().to_string(),
        ));
    }
    if value.is_array() {
        return value
            .cast::<Array>()
            .into_iter()
            .map(dynamic_to_stored_value)
            .collect::<Result<Vec<_>, _>>()
            .map(StoredValue::Array);
    }
    if value.is_map() {
        let mut stored = BTreeMap::new();
        for (key, value) in value.cast::<Map>() {
            stored.insert(key.to_string(), dynamic_to_stored_value(value)?);
        }
        return Ok(StoredValue::Map(stored));
    }

    Err(runtime_error(
        "only bool, int, float, string, array and map values are supported here",
    ))
}

fn stored_value_to_dynamic(value: StoredValue) -> Dynamic {
    match value {
        StoredValue::Bool(value) => value.into(),
        StoredValue::Int(value) => value.into(),
        StoredValue::Float(value) => value.into(),
        StoredValue::String(value) => value.into(),
        StoredValue::Array(values) => values
            .into_iter()
            .map(stored_value_to_dynamic)
            .collect::<Array>()
            .into(),
        StoredValue::Map(values) => values
            .into_iter()
            .map(|(key, value)| (key.into(), stored_value_to_dynamic(value)))
            .collect::<Map>()
            .into(),
    }
}

fn duration_from_millis(ms: i64) -> Result<Duration, Box<EvalAltResult>> {
    if ms < 0 {
        return Err(runtime_error("duration cannot be negative"));
    }

    Ok(Duration::from_millis(ms as u64))
}

fn duration_from_seconds(seconds: FLOAT) -> Result<Duration, Box<EvalAltResult>> {
    if seconds < 0.0 {
        return Err(runtime_error("duration cannot be negative"));
    }

    Ok(Duration::from_secs_f64(seconds as f64))
}

fn clamp_volume(value: FLOAT) -> f32 {
    value.clamp(0.0, 1.0) as f32
}

fn positive_scale(value: FLOAT) -> Result<f32, Box<EvalAltResult>> {
    if value <= 0.0 {
        Err(runtime_error("scale must be greater than zero"))
    } else {
        Ok(value as f32)
    }
}

fn non_negative_amplitude(value: FLOAT) -> f32 {
    value.max(0.0) as f32
}

fn normalize_vague(value: FLOAT) -> f32 {
    let value = if value > 1.0 { value / 255.0 } else { value };
    value.clamp(0.0001, 1.0) as f32
}

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        Position::NONE,
    ))
}

fn vfs_to_rhai_error(error: VfsError) -> Box<EvalAltResult> {
    runtime_error(error.to_string())
}
