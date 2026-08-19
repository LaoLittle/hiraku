use std::{collections::BTreeMap, sync::mpsc, time::Duration};

use bevy::math::Vec2;
use serde::Deserialize;

use crate::{
    effect::custom::CustomEffectOptions,
    state::{
        ChoiceOption, SaveGameData, SceneSharedState, SceneSnapshot, StoredValue, UiStylePatch,
    },
    storage::{StorageError, save_root_path, write_save_data_to_root},
    texture::TextureCatalog,
    ui::ScreenSpec,
    vfs::VfsResource,
};

mod ir;
pub mod ui_runtime;

pub use ir::{
    CameraEffectScope, IrChoiceOption, IrCommand, IrEvent, IrExpression, IrExpressionId,
    IrInstruction, IrProgram, IrRuntime, IrValidationError, IrVm, IrVmSnapshot, IrVmStatus,
    IrWaitKind, tick_ir_runtime,
};
pub use ui_runtime::{UiContext, UiIntent, evaluate_ui_script};

pub(crate) fn compile_story_program(path: &str, source: &str) -> Result<IrProgram, String> {
    if !path.ends_with(".hks") {
        return Err(format!(
            "executable scripts must use the `.hks` extension: `{path}`"
        ));
    }
    crate::hks_prelude::compile_story_to_ir(path, source).map_err(|error| error.to_string())
}

#[derive(Debug)]
pub enum ScriptCommand {
    Log(String),
    StartIr {
        path: String,
        program: IrProgram,
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
        scope: CameraEffectScope,
        center: Option<Vec2>,
        duration: Duration,
        ease: CharacterEase,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ApplyUserSettings(crate::storage::UserSettings),
    AdjustUserSetting {
        name: String,
        delta: f32,
    },
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
        mode: VoicePlaybackMode,
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
    Ok(match command {
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
        IrCommand::PlayVoice { path, volume } => ScriptCommand::PlayVoice {
            path,
            volume,
            mode: VoicePlaybackMode::Exclusive,
            animation_id: None,
            done: None,
        },
        IrCommand::SetCamera {
            blur,
            zoom,
            scope,
            duration_ms,
            ease,
        } => ScriptCommand::SetCamera {
            blur_intensity: blur,
            zoom,
            scope,
            center: None,
            duration: Duration::from_millis(duration_ms),
            ease: parse_ir_camera_ease(&ease)?,
            animation_id: None,
            done: None,
        },
        IrCommand::AdjustSetting { name, delta } => {
            ScriptCommand::AdjustUserSetting { name, delta }
        }
        IrCommand::Exit => ScriptCommand::Exit,
        IrCommand::Choose { .. } | IrCommand::OpenUi { .. } => {
            return Err("interactive UI commands are handled by the IR runtime".to_string());
        }
        IrCommand::LoadScript { .. } => {
            return Err("loadScript is handled by the IR runtime".to_string());
        }
        IrCommand::ReturnToTitle => ScriptCommand::ReturnToTitle,
        IrCommand::SetBackground { texture } => {
            let definition = textures
                .and_then(|catalog| catalog.resolve(&texture))
                .ok_or_else(|| format!("texture `{texture}` is not defined"))?;
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
        IrCommand::HksStatement { .. } | IrCommand::WaitHksTask { .. } => {
            return Err("HKS native statements are handled by the IR runtime".to_string());
        }
    })
}

fn parse_ir_camera_ease(name: &str) -> Result<CharacterEase, String> {
    match name {
        "" | "linear" => Ok(CharacterEase::Linear),
        "ease" => Ok(CharacterEase::Ease),
        "easeIn" | "ease_in" => Ok(CharacterEase::EaseIn),
        "easeOut" | "ease_out" => Ok(CharacterEase::EaseOut),
        "easeInOut" | "ease_in_out" => Ok(CharacterEase::EaseInOut),
        "bounce" => Ok(CharacterEase::Bounce),
        _ => Err(format!("unsupported camera easing `{name}`")),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BatchSubmitMode {
    Sequence,
    Parallel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePlaybackMode {
    Exclusive,
    Concurrent,
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

#[derive(Debug, Clone)]
pub enum ScriptResponse {
    Continue,
    Choice(StoredValue),
}

#[derive(Clone, Debug)]
pub struct ScriptBootstrap {
    pub startup_script: String,
    pub values: BTreeMap<String, StoredValue>,
    pub snapshot: Option<IrVmSnapshot>,
    pub pending_input_variable: Option<String>,
    pub pending_ui_screen: Option<String>,
}

impl ScriptBootstrap {
    pub fn new(startup_script: String) -> Self {
        Self {
            startup_script,
            values: BTreeMap::new(),
            snapshot: None,
            pending_input_variable: None,
            pending_ui_screen: None,
        }
    }

    pub fn from_save(data: &SaveGameData) -> Self {
        let mut values = data.globals.clone();
        values.extend(data.scope.clone());
        Self {
            startup_script: data.resume_script.clone(),
            values,
            snapshot: data.ir_snapshot.clone(),
            pending_input_variable: data.pending_input_variable.clone(),
            pending_ui_screen: data.pending_ui_screen.clone(),
        }
    }
}

pub fn start_hks_runtime(
    vfs: &VfsResource,
    runtime: &mut IrRuntime,
    bootstrap: ScriptBootstrap,
) -> Result<(), String> {
    let source = vfs
        .0
        .read_text(&bootstrap.startup_script)
        .map_err(|error| error.to_string())?;
    let program = compile_story_program(&bootstrap.startup_script, &source)?;
    let vm = if let Some(snapshot) = bootstrap.snapshot {
        IrVm::restore(program, snapshot).map_err(|error| error.to_string())?
    } else {
        let mut vm = IrVm::new(program).map_err(|error| error.to_string())?;
        for (name, value) in bootstrap.values {
            vm.set_stored_value(name, value);
        }
        vm
    };
    let restored_wait = match vm.status() {
        IrVmStatus::Waiting(wait) => Some(wait.clone()),
        _ => None,
    };
    runtime.vm = Some(vm);
    runtime.current_script = Some(bootstrap.startup_script);
    runtime.events.clear();
    runtime.wait_response = None;
    runtime.pending_input_variable = bootstrap.pending_input_variable;
    runtime.pending_ui_screen = bootstrap.pending_ui_screen;
    runtime.pending_response = None;
    if let Some(wait) = restored_wait {
        runtime.events.push_back(IrEvent::Waiting(wait.clone()));
        if matches!(wait, IrWaitKind::UiIntent | IrWaitKind::ScreenChoice)
            && let (Some(path), Some(result)) = (
                runtime.pending_ui_screen.clone(),
                runtime.pending_input_variable.clone(),
            )
        {
            runtime
                .events
                .push_back(IrEvent::Command(IrCommand::OpenUi { path, result }));
        }
    }
    Ok(())
}

pub fn save_runtime_slot(
    slot: &str,
    runtime: &IrRuntime,
    shared_state: &SceneSharedState,
) -> Result<(), StorageError> {
    let current_script = runtime.current_script.clone().unwrap_or_default();
    let values = runtime
        .vm
        .as_ref()
        .map(IrVm::story_values)
        .unwrap_or_default();
    let data = SaveGameData {
        version: 6,
        resume_script: current_script,
        globals: values,
        scene: shared_state
            .0
            .lock()
            .expect("scene state mutex must not be poisoned while saving")
            .clone(),
        ir_snapshot: runtime.vm.as_ref().map(IrVm::snapshot),
        pending_input_variable: runtime.pending_input_variable.clone(),
        pending_ui_screen: runtime.pending_ui_screen.clone(),
        ..Default::default()
    };
    write_save_data_to_root(&save_root_path(), slot, &data)
}
