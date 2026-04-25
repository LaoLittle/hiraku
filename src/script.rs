use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use bevy::{log::error, math::{Vec2, Vec4}, prelude::Resource};
use rhai::{
    Array, Blob, Dynamic, Engine as RhaiEngine, EvalAltResult, FLOAT, INT, ImmutableString,
    NativeCallContext, Position, FnPtr,
};

use crate::{
    character::load_character_catalog,
    effect::CustomEffectOptions,
    storage::{UserSettings, read_user_settings, save_root_path, write_user_settings},
    state::{ChoiceOption, SaveGameData, SceneSnapshot, StoredValue, UiStylePatch},
    ui::{
        BarNode, ButtonNode, ContainerNode, ScreenImageNode, ScreenLayout, ScreenNode, ScreenSpec,
        SpacerNode, TextNode,
    },
    vfs::{HdpVfs, VfsError},
};

const JUMP_SCRIPT_SIGNAL: &str = "__hiraku_jump_script__::";
const CALL_SCRIPT_SIGNAL: &str = "__hiraku_call_script__::";
const RETURN_SCRIPT_SIGNAL: &str = "__hiraku_return_script__";
const RETURN_TO_TITLE_SIGNAL: &str = "__hiraku_return_to_title__";
const ENGINE_STOPPED_SIGNAL: &str = "__hiraku_engine_stopped__";

enum InlineDialogueChunk {
    Text(String),
    Command(String),
}

enum ScriptFlowAction {
    Jump(String),
    Call(String),
    Return,
    ReturnToTitle,
    EngineStopped,
}

#[derive(Debug)]
pub enum ScriptCommand {
    Log(String),
    SetBackground {
        path: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
        done: Option<mpsc::Sender<ScriptResponse>>,
    },
    ShowSprite {
        id: String,
        path: String,
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
    ApplyUserSettings(UserSettings),
    ApplyUiStyle(UiStylePatch),
    ResetUiStyle,
    ShowScreen {
        screen: ScreenSpec,
        done: mpsc::Sender<ScriptResponse>,
    },
    Choose {
        prompt: String,
        options: Vec<ChoiceOption>,
        done: mpsc::Sender<ScriptResponse>,
    },
    ShowCharacter {
        actor_id: String,
        character_name: String,
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

#[derive(Debug, Clone, Default)]
pub struct DialogueTextEffectSpec {
    pub mode: Option<String>,
    pub cps: Option<f32>,
    pub fade_seconds: Option<f32>,
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
pub struct CharacterAnimationKeyframeInput {
    pub time: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub dx: Option<f32>,
    pub dy: Option<f32>,
    pub ease: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScriptEffectOptions {
    pub from_path: Option<String>,
    pub to_path: Option<String>,
    pub rule_path: Option<String>,
    pub aux0_path: Option<String>,
    pub aux1_path: Option<String>,
    pub duration: Duration,
    pub mode: f32,
    pub p0: Vec4,
    pub p1: Vec4,
    pub p2: Vec4,
    pub p3: Vec4,
    pub commit_to_bg: bool,
}

impl Default for ScriptEffectOptions {
    fn default() -> Self {
        Self {
            from_path: None,
            to_path: None,
            rule_path: None,
            aux0_path: None,
            aux1_path: None,
            duration: Duration::from_millis(0),
            mode: 0.0,
            p0: Vec4::ZERO,
            p1: Vec4::ZERO,
            p2: Vec4::ZERO,
            p3: Vec4::ZERO,
            commit_to_bg: false,
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
    pub save_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ScriptBootstrap {
    pub startup_script: String,
    pub script_stack: Vec<String>,
    pub globals: BTreeMap<String, StoredValue>,
}

impl ScriptBootstrap {
    pub fn new(startup_script: String) -> Self {
        Self {
            startup_script,
            script_stack: Vec::new(),
            globals: BTreeMap::new(),
        }
    }

    pub fn from_save(data: &SaveGameData) -> Self {
        Self {
            startup_script: data.resume_script.clone(),
            script_stack: data.script_stack.clone(),
            globals: data.globals.clone(),
        }
    }
}

#[derive(Clone)]
struct ScriptHost {
    vfs: Arc<HdpVfs>,
    command_tx: mpsc::Sender<ScriptCommand>,
    current_script: Arc<Mutex<String>>,
    script_stack: Arc<Mutex<Vec<String>>>,
    globals: Arc<Mutex<BTreeMap<String, StoredValue>>>,
    scene_state: Arc<Mutex<SceneSnapshot>>,
    next_animation_id: Arc<Mutex<u64>>,
    batch_registry: Arc<Mutex<BatchRegistry>>,
    inline_dialogue_control: Arc<Mutex<InlineDialogueControl>>,
    save_root: PathBuf,
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
        *self.current_script.lock().unwrap() = path.into();
    }

    fn resolve_path(&self, requested: &str) -> String {
        self.vfs
            .resolve_path(Some(&self.current_script_path()), requested)
    }

    fn send(&self, command: ScriptCommand) -> Result<(), Box<EvalAltResult>> {
        self.command_tx
            .send(command)
            .map_err(|err| runtime_error(format!("failed to send command to engine: {err}")))
    }

    fn send_and_wait<F>(&self, builder: F) -> Result<ScriptResponse, Box<EvalAltResult>>
    where
        F: FnOnce(mpsc::Sender<ScriptResponse>) -> ScriptCommand,
    {
        let (done_tx, done_rx) = mpsc::channel();
        self.send(builder(done_tx))?;
        done_rx
            .recv()
            .map_err(|_| engine_stopped_signal())
    }

    fn send_continue<F>(&self, builder: F) -> Result<(), Box<EvalAltResult>>
    where
        F: FnOnce(mpsc::Sender<ScriptResponse>) -> ScriptCommand,
    {
        match self.send_and_wait(builder)? {
            ScriptResponse::Continue => Ok(()),
            ScriptResponse::Choice(_) => Err(runtime_error("engine returned unexpected choice response")),
        }
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
        registry.groups.insert(
            group_id.clone(),
            BatchGroup {
                handles: deduped,
            },
        );
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
        prompt: String,
        options: Vec<ChoiceOption>,
    ) -> Result<StoredValue, Box<EvalAltResult>> {
        match self.send_and_wait(|done| ScriptCommand::Choose {
            prompt,
            options,
            done,
        })? {
            ScriptResponse::Choice(value) => Ok(value),
            ScriptResponse::Continue => Err(runtime_error("engine returned unexpected continue response")),
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

    fn save_game(
        &self,
        slot: &str,
        resume_script: Option<String>,
    ) -> Result<(), Box<EvalAltResult>> {
        fs::create_dir_all(&self.save_root)
            .map_err(|err| runtime_error(format!("failed to create save directory: {err}")))?;

        let save_path = self.save_file_path(slot)?;
        let data = SaveGameData {
            resume_script: resume_script.unwrap_or_else(|| self.current_script_path()),
            script_stack: self.script_stack.lock().unwrap().clone(),
            globals: self.globals.lock().unwrap().clone(),
            scene: self.scene_state.lock().unwrap().clone(),
        };

        let payload = toml::to_string_pretty(&data)
            .map_err(|err| runtime_error(format!("failed to serialize save data: {err}")))?;
        fs::write(&save_path, payload).map_err(|err| {
            runtime_error(format!("failed to write save file `{}`: {err}", save_path.display()))
        })
    }

    fn load_game(&self, slot: &str) -> Result<String, Box<EvalAltResult>> {
        let save_path = self.save_file_path(slot)?;
        let payload = fs::read_to_string(&save_path).map_err(|err| {
            runtime_error(format!("failed to read save file `{}`: {err}", save_path.display()))
        })?;
        let data: SaveGameData = toml::from_str(&payload)
            .map_err(|err| runtime_error(format!("failed to parse save file: {err}")))?;

        *self.script_stack.lock().unwrap() = data.script_stack.clone();
        *self.globals.lock().unwrap() = data.globals.clone();
        *self.scene_state.lock().unwrap() = data.scene.clone();
        self.send_continue(|done| ScriptCommand::RestoreSnapshot {
            snapshot: data.scene.clone(),
            done,
        })?;

        Ok(data.resume_script)
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
        let slot = sanitize_slot_name(slot)?;
        Ok(self.save_root.join(format!("{slot}.toml")))
    }
}

pub fn spawn_script_runtime(
    commands: &mut bevy::prelude::Commands,
    vfs: Arc<HdpVfs>,
    scene_state: Arc<Mutex<SceneSnapshot>>,
    bootstrap: ScriptBootstrap,
) {
    let (command_tx, command_rx) = mpsc::channel();
    let current_script = Arc::new(Mutex::new(bootstrap.startup_script.clone()));
    let script_stack = Arc::new(Mutex::new(bootstrap.script_stack));
    let next_animation_id = Arc::new(Mutex::new(0));
    let batch_registry = Arc::new(Mutex::new(BatchRegistry::default()));
    let inline_dialogue_control = Arc::new(Mutex::new(InlineDialogueControl::default()));

    commands.insert_resource(ScriptInbox(Mutex::new(command_rx)));
    commands.insert_resource(InlineDialogueControlResource(inline_dialogue_control.clone()));

    let host = ScriptHost {
        vfs,
        command_tx,
        current_script: current_script.clone(),
        script_stack: script_stack.clone(),
        globals: Arc::new(Mutex::new(bootstrap.globals)),
        scene_state,
        next_animation_id,
        batch_registry,
        inline_dialogue_control,
        save_root: save_root_path(),
    };

    commands.insert_resource(ScriptRuntimeState {
        current_script,
        script_stack,
        globals: host.globals.clone(),
        save_root: host.save_root.clone(),
    });

    let startup_script = bootstrap.startup_script;

    thread::Builder::new()
        .name("hiraku-rhai".to_string())
        .spawn(move || run_script_loop(host, startup_script))
        .expect("failed to spawn script runtime thread");
}

fn run_script_loop(host: ScriptHost, startup_script: String) {
    let mut engine = RhaiEngine::new();
    register_api(&mut engine, &host);

    let mut next_script = startup_script;

    loop {
        host.set_current_script_path(next_script.clone());

        let source = match host.vfs.read_text(&next_script) {
            Ok(source) => source,
            Err(err) => {
                error!("failed to read script `{}`: {err}", next_script);
                return;
            }
        };

        let mut scope = rhai::Scope::new();
        match engine.eval_with_scope::<Dynamic>(&mut scope, &source) {
            Ok(_) => return,
            Err(err) => {
                if let Some(action) = extract_script_flow_action(err.as_ref()) {
                    match action {
                        ScriptFlowAction::Jump(target) => {
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::Call(target) => {
                            host.script_stack.lock().unwrap().push(next_script.clone());
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::Return => {
                            let Some(target) = host.script_stack.lock().unwrap().pop() else {
                                error!("script `{}` tried to return with an empty call stack", next_script);
                                return;
                            };
                            next_script = target;
                            continue;
                        }
                        ScriptFlowAction::ReturnToTitle => return,
                        ScriptFlowAction::EngineStopped => return,
                    }
                }

                error!("script `{}` failed: {err}", next_script);
                return;
            }
        }
    }
}

fn register_api(engine: &mut RhaiEngine, host: &ScriptHost) {
    register_typed_data_api(engine);

    let log_host = host.clone();
    engine.register_fn("log", move |message: ImmutableString| {
        let _ = log_host.send(ScriptCommand::Log(message.to_string()));
    });

    let seq_host = host.clone();
    engine.register_fn(
        "seq",
        move |ctx: NativeCallContext, callback: FnPtr| -> Result<String, Box<EvalAltResult>> {
            seq_host.begin_batch(BatchMode::Sequence)?;
            match callback.call_within_context::<Dynamic>(&ctx, ()) {
                Ok(_) => seq_host.finish_batch(),
                Err(err) => {
                    seq_host.cancel_batch();
                    Err(err)
                }
            }
        },
    );

    let par_host = host.clone();
    engine.register_fn(
        "par",
        move |ctx: NativeCallContext, callback: FnPtr| -> Result<String, Box<EvalAltResult>> {
            par_host.begin_batch(BatchMode::Parallel)?;
            match callback.call_within_context::<Dynamic>(&ctx, ()) {
                Ok(_) => par_host.finish_batch(),
                Err(err) => {
                    par_host.cancel_batch();
                    Err(err)
                }
            }
        },
    );

    let pause_seconds_host = host.clone();
    engine.register_fn("pause", move |seconds: FLOAT| -> Result<Dynamic, Box<EvalAltResult>> {
        let duration = duration_from_seconds(seconds)?;
        run_blocking_or_collected(&pause_seconds_host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    });

    let pause_int_host = host.clone();
    engine.register_fn("pause", move |seconds: INT| -> Result<Dynamic, Box<EvalAltResult>> {
        let duration = duration_from_seconds(seconds as FLOAT)?;
        run_blocking_or_collected(&pause_int_host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    });

    let pause_ms_host = host.clone();
    engine.register_fn("pause_ms", move |ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
        let duration = duration_from_millis(ms)?;
        run_blocking_or_collected(&pause_ms_host, "pause", |animation_id, done| ScriptCommand::Wait {
            duration,
            animation_id,
            done: done.expect("blocking or collected waits always provide a completion sender"),
        })
    });

    let wait_handle_host = host.clone();
    engine.register_fn("wait", move |handle: ImmutableString| -> Result<(), Box<EvalAltResult>> {
        if wait_handle_host.is_batch_mode() {
            return Err(runtime_error("wait cannot be used inside seq/par; wait after the block returns a handle"));
        }
        wait_handle_host.wait_for_handle(handle.to_string())
    });

    let wait_handles_host = host.clone();
    engine.register_fn("wait", move |handles: Array| -> Result<(), Box<EvalAltResult>> {
        if wait_handles_host.is_batch_mode() {
            return Err(runtime_error("wait cannot be used inside seq/par; wait after the block returns a handle"));
        }
        let handles = parse_animation_ids(handles)?;
        wait_handles_host.wait_for_handles(handles)
    });

    let cancel_host = host.clone();
    engine.register_fn("cancel", move |handle: ImmutableString| -> Result<(), Box<EvalAltResult>> {
        if cancel_host.is_batch_mode() {
            return Err(runtime_error("cancel cannot be used inside seq/par"));
        }
        cancel_host.cancel_handle(handle.to_string())
    });

    let say_host = host.clone();
    engine.register_fn(
        "say",
        move |
            ctx: NativeCallContext,
            speaker: ImmutableString,
            text: ImmutableString,
        | -> Result<(), Box<EvalAltResult>> {
            say_with_inline_commands(&ctx, &say_host, speaker.to_string(), text.to_string())
        },
    );

    let narrate_host = host.clone();
    engine.register_fn(
        "narrate",
        move |ctx: NativeCallContext, text: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            say_with_inline_commands(&ctx, &narrate_host, String::new(), text.to_string())
        },
    );

    let clear_host = host.clone();
    engine.register_fn("clear_text", move || -> Result<(), Box<EvalAltResult>> {
        clear_host.send(ScriptCommand::ClearDialogue)
    });

    let bg_host = host.clone();
    engine.register_fn(
        "bg",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let path = bg_host.resolve_path(&path);
            bg_host.send(ScriptCommand::SetBackground {
                path,
                fade: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let bg_fade_host = host.clone();
    engine.register_fn(
        "bg_fade",
        move |path: ImmutableString, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            let path = bg_fade_host.resolve_path(&path);
            run_blocking_or_collected(&bg_fade_host, "bg-fade", |animation_id, done| ScriptCommand::SetBackground {
                path,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let bg_rule_host = host.clone();
    engine.register_fn(
        "bg_rule",
        move |
            path: ImmutableString,
            rule_path: ImmutableString,
            ms: i64,
            vague: FLOAT,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            let path = bg_rule_host.resolve_path(&path);
            let rule_path = bg_rule_host.resolve_path(&rule_path);
            run_blocking_or_collected(&bg_rule_host, "bg-rule", |animation_id, done| ScriptCommand::RuleTransitionBg {
                path,
                rule_path,
                duration,
                vague: normalize_vague(vague),
                animation_id,
                done,
            })
        },
    );

    let effect_host = host.clone();
    engine.register_fn(
        "effect",
        move |options: ScriptEffectOptions| -> Result<Dynamic, Box<EvalAltResult>> {
            let options = resolve_custom_effect_options(&effect_host, options)?;
            run_blocking_or_collected(&effect_host, "effect", |animation_id, done| ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
                done,
            })
        },
    );

    let show_host = host.clone();
    engine.register_fn(
        "show",
        move |id: ImmutableString, path: ImmutableString, x: FLOAT, y: FLOAT| -> Result<(), Box<EvalAltResult>> {
            let path = show_host.resolve_path(&path);
            show_host.send(ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: 0.0,
                scale: 1.0,
                fade: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let show_scaled_host = host.clone();
    engine.register_fn(
        "show",
        move |
            id: ImmutableString,
            path: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
        | -> Result<(), Box<EvalAltResult>> {
            let path = show_scaled_host.resolve_path(&path);
            let scale = positive_scale(scale)?;
            show_scaled_host.send(ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: 0.0,
                scale,
                fade: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let show_layered_host = host.clone();
    engine.register_fn(
        "show",
        move |
            id: ImmutableString,
            path: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
            layer: FLOAT,
        | -> Result<(), Box<EvalAltResult>> {
            let path = show_layered_host.resolve_path(&path);
            let scale = positive_scale(scale)?;
            show_layered_host.send(ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: layer as f32,
                scale,
                fade: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let show_fade_host = host.clone();
    engine.register_fn(
        "show_fade",
        move |id: ImmutableString, path: ImmutableString, x: FLOAT, y: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            let path = show_fade_host.resolve_path(&path);
            run_blocking_or_collected(&show_fade_host, "show", |animation_id, done| ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: 0.0,
                scale: 1.0,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let show_fade_scaled_host = host.clone();
    engine.register_fn(
        "show_fade",
        move |
            id: ImmutableString,
            path: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
            ms: i64,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            let path = show_fade_scaled_host.resolve_path(&path);
            let scale = positive_scale(scale)?;
            run_blocking_or_collected(&show_fade_scaled_host, "show", |animation_id, done| ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: 0.0,
                scale,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let show_fade_layered_host = host.clone();
    engine.register_fn(
        "show_fade",
        move |
            id: ImmutableString,
            path: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
            layer: FLOAT,
            ms: i64,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            let path = show_fade_layered_host.resolve_path(&path);
            let scale = positive_scale(scale)?;
            run_blocking_or_collected(&show_fade_layered_host, "show", |animation_id, done| ScriptCommand::ShowSprite {
                id: id.to_string(),
                path,
                position: Vec2::new(x as f32, y as f32),
                layer: layer as f32,
                scale,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let hide_host = host.clone();
    engine.register_fn(
        "hide",
        move |id: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            hide_host.send(ScriptCommand::HideSprite {
                id: id.to_string(),
                fade: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let show_character_host = host.clone();
    engine.register_fn(
        "show_character",
        move |
            name: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let scale = positive_scale(scale)?;
            let actor_id = name.to_string();
            let character_name = actor_id.clone();
            run_blocking_or_collected(&show_character_host, "character-show", |animation_id, done| ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                position: Vec2::new(x as f32, y as f32),
                scale,
                fade: None,
                animation_id,
                done,
            })
        },
    );

    let show_character_fade_host = host.clone();
    engine.register_fn(
        "show_character",
        move |
            name: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
            ms: i64,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let scale = positive_scale(scale)?;
            let fade = duration_from_millis(ms)?;
            let actor_id = name.to_string();
            let character_name = actor_id.clone();
            run_blocking_or_collected(&show_character_fade_host, "character-show", |animation_id, done| ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                position: Vec2::new(x as f32, y as f32),
                scale,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let show_character_as_host = host.clone();
    engine.register_fn(
        "show_character",
        move |
            actor_id: ImmutableString,
            name: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let scale = positive_scale(scale)?;
            run_blocking_or_collected(&show_character_as_host, "character-show", |animation_id, done| ScriptCommand::ShowCharacter {
                actor_id: actor_id.to_string(),
                character_name: name.to_string(),
                position: Vec2::new(x as f32, y as f32),
                scale,
                fade: None,
                animation_id,
                done,
            })
        },
    );

    let show_character_as_fade_host = host.clone();
    engine.register_fn(
        "show_character",
        move |
            actor_id: ImmutableString,
            name: ImmutableString,
            x: FLOAT,
            y: FLOAT,
            scale: FLOAT,
            ms: i64,
        | -> Result<Dynamic, Box<EvalAltResult>> {
            let scale = positive_scale(scale)?;
            let fade = duration_from_millis(ms)?;
            run_blocking_or_collected(&show_character_as_fade_host, "character-show", |animation_id, done| ScriptCommand::ShowCharacter {
                actor_id: actor_id.to_string(),
                character_name: name.to_string(),
                position: Vec2::new(x as f32, y as f32),
                scale,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let hide_character_host = host.clone();
    engine.register_fn(
        "hide_character",
        move |actor_id: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            hide_character_host.send(ScriptCommand::HideCharacter {
                actor_id: actor_id.to_string(),
            })
        },
    );

    let animate_character_host = host.clone();
    engine.register_fn(
        "animate",
        move |actor_id: ImmutableString, keyframes: Array| -> Result<Dynamic, Box<EvalAltResult>> {
            let actor_id = actor_id.to_string();
            let current = animate_character_host.character_position(&actor_id)?;
            let keyframes = parse_character_animation_keyframes(keyframes, current)?;
            run_blocking_or_collected(&animate_character_host, "character-animate", |animation_id, done| {
                ScriptCommand::AnimateCharacter {
                    actor_id,
                    keyframes,
                    animation_id,
                    done,
                }
            })
        },
    );

    let jump_character_host = host.clone();
    engine.register_fn(
        "jump_character",
        move |actor_id: ImmutableString, height: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&jump_character_host, "character-jump", |animation_id, done| ScriptCommand::JumpCharacter {
                actor_id: actor_id.to_string(),
                height: non_negative_amplitude(height),
                duration,
                animation_id,
                done,
            })
        },
    );

    let shake_character_host = host.clone();
    engine.register_fn(
        "shake_character",
        move |actor_id: ImmutableString, amplitude: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&shake_character_host, "character-shake", |animation_id, done| ScriptCommand::ShakeCharacter {
                actor_id: actor_id.to_string(),
                amplitude: non_negative_amplitude(amplitude),
                duration,
                animation_id,
                done,
            })
        },
    );

    let hide_fade_host = host.clone();
    engine.register_fn(
        "hide_fade",
        move |id: ImmutableString, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            run_blocking_or_collected(&hide_fade_host, "hide", |animation_id, done| ScriptCommand::HideSprite {
                id: id.to_string(),
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let overlay_host = host.clone();
    engine.register_fn(
        "screen_fade",
        move |alpha: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let fade = duration_from_millis(ms)?;
            run_blocking_or_collected(&overlay_host, "screen-fade", |animation_id, done| ScriptCommand::SetOverlay {
                alpha: alpha.clamp(0.0, 1.0) as f32,
                fade: Some(fade),
                animation_id,
                done,
            })
        },
    );

    let move_sprite_host = host.clone();
    engine.register_fn(
        "move_sprite",
        move |id: ImmutableString, x: FLOAT, y: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&move_sprite_host, "move", |animation_id, done| ScriptCommand::MoveSprite {
                id: id.to_string(),
                position: Vec2::new(x as f32, y as f32),
                duration,
                animation_id,
                done,
            })
        },
    );

    let move_sprite_by_host = host.clone();
    engine.register_fn(
        "move_sprite_by",
        move |id: ImmutableString, dx: FLOAT, dy: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            let current = move_sprite_by_host
                .scene_state
                .lock()
                .unwrap()
                .sprites
                .iter()
                .find(|sprite| sprite.id == id.as_str())
                .map(|sprite| Vec2::new(sprite.x, sprite.y))
                .ok_or_else(|| runtime_error(format!("sprite `{}` not found", id)))?;
            run_blocking_or_collected(&move_sprite_by_host, "move", |animation_id, done| ScriptCommand::MoveSprite {
                id: id.to_string(),
                position: current + Vec2::new(dx as f32, dy as f32),
                duration,
                animation_id,
                done,
            })
        },
    );

    let scale_sprite_host = host.clone();
    engine.register_fn(
        "scale_sprite",
        move |id: ImmutableString, scale: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            let scale = positive_scale(scale)?;
            run_blocking_or_collected(&scale_sprite_host, "scale", |animation_id, done| ScriptCommand::ScaleSprite {
                id: id.to_string(),
                scale,
                duration,
                animation_id,
                done,
            })
        },
    );

    let fade_sprite_host = host.clone();
    engine.register_fn(
        "fade_sprite",
        move |id: ImmutableString, alpha: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&fade_sprite_host, "fade", |animation_id, done| ScriptCommand::FadeSprite {
                id: id.to_string(),
                alpha: alpha.clamp(0.0, 1.0) as f32,
                duration,
                animation_id,
                done,
            })
        },
    );

    let shake_host = host.clone();
    engine.register_fn(
        "shake",
        move |ms: i64, amplitude: FLOAT| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&shake_host, "shake", |animation_id, done| ScriptCommand::Shake {
                duration,
                amplitude: non_negative_amplitude(amplitude),
                animation_id,
                done,
            })
        },
    );

    let bgm_host = host.clone();
    engine.register_fn(
        "play_bgm",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = bgm_host.resolve_path(&path);
            bgm_host.send(ScriptCommand::PlayBgm {
                path,
                volume: 1.0,
                fade_in: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let bgm_volume_host = host.clone();
    engine.register_fn(
        "play_bgm",
        move |path: ImmutableString, volume: FLOAT| -> Result<(), Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = bgm_volume_host.resolve_path(&path);
            bgm_volume_host.send(ScriptCommand::PlayBgm {
                path,
                volume: clamp_volume(volume),
                fade_in: None,
                animation_id: None,
                done: None,
            })
        },
    );

    let bgm_fadein_host = host.clone();
    engine.register_fn(
        "play_bgm",
        move |path: ImmutableString, volume: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let fade_in = duration_from_millis(ms)?;
            let path = bgm_fadein_host.resolve_path(&path);
            run_blocking_or_collected(&bgm_fadein_host, "bgm", |animation_id, done| ScriptCommand::PlayBgm {
                path,
                volume: clamp_volume(volume),
                fade_in: Some(fade_in),
                animation_id,
                done,
            })
        },
    );

    let set_bgm_volume_host = host.clone();
    engine.register_fn(
        "set_bgm_volume",
        move |volume: FLOAT| -> Result<(), Box<EvalAltResult>> {
            set_bgm_volume_host.send(ScriptCommand::SetBgmVolume {
                volume: clamp_volume(volume),
            })
        },
    );

    let fade_bgm_host = host.clone();
    engine.register_fn(
        "fade_bgm",
        move |volume: FLOAT, ms: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let duration = duration_from_millis(ms)?;
            run_blocking_or_collected(&fade_bgm_host, "bgm-fade", |animation_id, done| ScriptCommand::FadeBgm {
                volume: clamp_volume(volume),
                duration,
                animation_id,
                done,
            })
        },
    );

    let stop_bgm_host = host.clone();
    engine.register_fn("stop_bgm", move || -> Result<(), Box<EvalAltResult>> {
        stop_bgm_host.send(ScriptCommand::StopBgm)
    });

    let voice_host = host.clone();
    engine.register_fn(
        "play_voice",
        move |path: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = voice_host.resolve_path(&path);
            run_blocking_or_collected(&voice_host, "voice", |animation_id, done| ScriptCommand::PlayVoice {
                path,
                volume: 1.0,
                animation_id,
                done,
            })
        },
    );

    let voice_volume_host = host.clone();
    engine.register_fn(
        "play_voice",
        move |path: ImmutableString, volume: FLOAT| -> Result<Dynamic, Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = voice_volume_host.resolve_path(&path);
            run_blocking_or_collected(&voice_volume_host, "voice", |animation_id, done| ScriptCommand::PlayVoice {
                path,
                volume: clamp_volume(volume),
                animation_id,
                done,
            })
        },
    );

    let stop_voice_host = host.clone();
    engine.register_fn("stop_voice", move || -> Result<(), Box<EvalAltResult>> {
        stop_voice_host.send(ScriptCommand::StopVoice)
    });

    let sfx_host = host.clone();
    engine.register_fn(
        "play_sfx",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = sfx_host.resolve_path(&path);
            sfx_host.send(ScriptCommand::PlaySfx { path, volume: 1.0 })
        },
    );

    let sfx_volume_host = host.clone();
    engine.register_fn(
        "play_sfx",
        move |path: ImmutableString, volume: FLOAT| -> Result<(), Box<EvalAltResult>> {
            reject_known_unsupported_audio_path(&path)?;
            let path = sfx_volume_host.resolve_path(&path);
            sfx_volume_host.send(ScriptCommand::PlaySfx {
                path,
                volume: clamp_volume(volume),
            })
        },
    );

    let choice_host = host.clone();
    engine.register_fn(
        "choice",
        move |options: Array| -> Result<Dynamic, Box<EvalAltResult>> {
            let options = parse_choice_options(options)?;
            let selection = choice_host.request_choice(String::new(), options)?;
            Ok(stored_value_to_dynamic(selection))
        },
    );

    let screen_host = host.clone();
    engine.register_fn(
        "screen",
        move |mut screen: ScreenSpec| -> Result<Dynamic, Box<EvalAltResult>> {
            resolve_screen_spec_paths(&screen_host, &mut screen);
            match screen_host.send_and_wait(|done| ScriptCommand::ShowScreen { screen, done })? {
                ScriptResponse::Choice(value) => Ok(stored_value_to_dynamic(value)),
                ScriptResponse::Continue => Err(runtime_error("engine returned unexpected continue response")),
            }
        },
    );

    let prompt_choice_host = host.clone();
    engine.register_fn(
        "choice",
        move |prompt: ImmutableString, options: Array| -> Result<Dynamic, Box<EvalAltResult>> {
            let options = parse_choice_options(options)?;
            let selection = prompt_choice_host.request_choice(prompt.to_string(), options)?;
            Ok(stored_value_to_dynamic(selection))
        },
    );

    let set_global_host = host.clone();
    engine.register_fn(
        "set_global",
        move |name: ImmutableString, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
            set_global_host.set_global(&name, value)
        },
    );

    let get_global_host = host.clone();
    engine.register_fn("get_global", move |name: ImmutableString| -> Dynamic {
        get_global_host.get_global(&name)
    });

    let get_global_or_host = host.clone();
    engine.register_fn(
        "get_global_or",
        move |name: ImmutableString, fallback: Dynamic| -> Dynamic {
            get_global_or_host.get_global_or(&name, fallback)
        },
    );

    let has_global_host = host.clone();
    engine.register_fn("has_global", move |name: ImmutableString| -> bool {
        has_global_host.has_global(&name)
    });

    let remove_global_host = host.clone();
    engine.register_fn("remove_global", move |name: ImmutableString| -> bool {
        remove_global_host.remove_global(&name)
    });

    let clear_globals_host = host.clone();
    engine.register_fn("clear_globals", move || {
        clear_globals_host.clear_globals();
    });

    let setting_host = host.clone();
    engine.register_fn(
        "get_setting",
        move |name: ImmutableString| -> Result<FLOAT, Box<EvalAltResult>> {
            Ok(setting_host.user_setting(&name)? as FLOAT)
        },
    );

    let set_setting_host = host.clone();
    engine.register_fn(
        "set_setting",
        move |name: ImmutableString, value: FLOAT| -> Result<(), Box<EvalAltResult>> {
            set_setting_host.set_user_setting(&name, value as f32)
        },
    );

    let ui_style_host = host.clone();
    engine.register_fn("set_ui_style", move |style: UiStylePatch| -> Result<(), Box<EvalAltResult>> {
        ui_style_host.send(ScriptCommand::ApplyUiStyle(style))
    });

    let reset_ui_style_host = host.clone();
    engine.register_fn("reset_ui_style", move || -> Result<(), Box<EvalAltResult>> {
        reset_ui_style_host.send(ScriptCommand::ResetUiStyle)
    });

    let text_effect_host = host.clone();
    engine.register_fn("set_text_effect", move |effect: DialogueTextEffectSpec| -> Result<(), Box<EvalAltResult>> {
        text_effect_host.send(ScriptCommand::SetTextEffect(effect))
    });

    let reset_text_effect_host = host.clone();
    engine.register_fn("reset_text_effect", move || -> Result<(), Box<EvalAltResult>> {
        reset_text_effect_host.send(ScriptCommand::ResetTextEffect)
    });

    let character_names_host = host.clone();
    engine.register_fn("character_names", move || -> Array {
        character_names_host
            .character_names()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let character_exists_host = host.clone();
    engine.register_fn("character_exists", move |name: ImmutableString| -> bool {
        character_exists_host.character_exists(&name)
    });

    let load_text_host = host.clone();
    engine.register_fn(
        "load_text",
        move |path: ImmutableString| -> Result<String, Box<EvalAltResult>> {
            load_text_host.read_text(&path)
        },
    );

    let load_bytes_host = host.clone();
    engine.register_fn(
        "load_bytes",
        move |path: ImmutableString| -> Result<Blob, Box<EvalAltResult>> {
            load_bytes_host.read_bytes(&path)
        },
    );

    let exists_host = host.clone();
    engine.register_fn("exists", move |path: ImmutableString| -> bool {
        let path = exists_host.resolve_path(&path);
        exists_host.vfs.exists(&path)
    });

    let save_exists_host = host.clone();
    engine.register_fn("save_exists", move |slot: ImmutableString| -> bool {
        save_exists_host.save_exists(&slot).unwrap_or(false)
    });

    let save_host = host.clone();
    engine.register_fn(
        "save",
        move |slot: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            save_host.save_game(&slot, None)
        },
    );

    let save_resume_host = host.clone();
    engine.register_fn(
        "save",
        move |slot: ImmutableString, resume_script: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let resume_script = save_resume_host.resolve_path(&resume_script);
            save_resume_host.save_game(&slot, Some(resume_script))
        },
    );

    let load_save_host = host.clone();
    engine.register_fn(
        "load",
        move |slot: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let resume_script = load_save_host.load_game(&slot)?;
            Err(jump_script_signal(resume_script))
        },
    );

    let load_script_host = host.clone();
    engine.register_fn(
        "load_script",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let resolved = load_script_host.resolve_path(&path);
            Err(jump_script_signal(resolved))
        },
    );

    let jump_script_host = host.clone();
    engine.register_fn(
        "jump_script",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let resolved = jump_script_host.resolve_path(&path);
            Err(jump_script_signal(resolved))
        },
    );

    let call_script_host = host.clone();
    engine.register_fn(
        "call_script",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            let resolved = call_script_host.resolve_path(&path);
            Err(call_script_signal(resolved))
        },
    );

    engine.register_fn("return_script", move || -> Result<(), Box<EvalAltResult>> {
        Err(return_script_signal())
    });

    let return_to_title_host = host.clone();
    engine.register_fn("return_to_title", move || -> Result<(), Box<EvalAltResult>> {
        return_to_title_host.send(ScriptCommand::ReturnToTitle)?;
        Err(return_to_title_signal())
    });

    let current_script_host = host.clone();
    engine.register_fn("current_script", move || -> String {
        current_script_host.current_script_path()
    });
}

fn register_typed_data_api(engine: &mut RhaiEngine) {
    engine
        .register_type_with_name::<UiStylePatch>("UiStylePatch")
        .register_type_with_name::<DialogueTextEffectSpec>("TextEffectSpec")
        .register_type_with_name::<ChoiceOption>("ChoiceOption")
        .register_type_with_name::<ScreenSpec>("ScreenSpec")
        .register_type_with_name::<ScreenNode>("ScreenNode")
        .register_type_with_name::<CharacterAnimationKeyframeInput>("CharacterKeyframe")
        .register_type_with_name::<ScriptEffectOptions>("EffectOptions")
        .register_fn("ui_style", UiStylePatch::default)
        .register_fn("text_effect", text_effect_spec)
        .register_fn("choice_option", choice_option_string)
        .register_fn("choice_option", choice_option_dynamic)
        .register_fn("screen_spec", screen_spec_from_children)
        .register_fn("screen_panel", screen_panel_spec)
        .register_fn("set_panel", |screen: &mut ScreenSpec, value: bool| screen.panel = value)
        .register_fn("set_title", |screen: &mut ScreenSpec, title: ImmutableString| screen.title = Some(title.to_string()))
        .register_fn("set_width", |screen: &mut ScreenSpec, width: FLOAT| screen.width = Some(width as f32))
        .register_fn("set_padding", |screen: &mut ScreenSpec, padding: FLOAT| screen.padding = padding as f32)
        .register_fn("set_gap", |screen: &mut ScreenSpec, gap: FLOAT| screen.gap = gap as f32)
        .register_fn("set_background", |screen: &mut ScreenSpec, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            screen.background = Some(array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("set_border", |screen: &mut ScreenSpec, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            screen.border = Some(array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("set_background_image", |screen: &mut ScreenSpec, path: ImmutableString| {
            screen.background_image = Some(path.to_string());
        })
        .register_fn("set_overlay", |screen: &mut ScreenSpec, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            screen.overlay = Some(array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("text_node", text_node)
        .register_fn("button_node", button_node_string)
        .register_fn("button_node", button_node_dynamic)
        .register_fn("image_node", image_node)
        .register_fn("bar_node", bar_node)
        .register_fn("spacer_node", spacer_node)
        .register_fn("vbox_node", vbox_node)
        .register_fn("hbox_node", hbox_node)
        .register_fn("frame_node", frame_node)
        .register_fn("set_text", |node: &mut ScreenNode, text: ImmutableString| set_node_text(node, text.to_string()))
        .register_fn("set_size", |node: &mut ScreenNode, size: FLOAT| set_node_size(node, size as f32))
        .register_fn("set_color", |node: &mut ScreenNode, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            set_node_color(node, array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("set_background", |node: &mut ScreenNode, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            set_node_background(node, array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("set_hovered_background", |node: &mut ScreenNode, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            if let ScreenNode::Button(button) = node {
                button.hovered_background = Some(array_to_rgba(rgba)?);
            }
            Ok(())
        })
        .register_fn("set_pressed_background", |node: &mut ScreenNode, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            if let ScreenNode::Button(button) = node {
                button.pressed_background = Some(array_to_rgba(rgba)?);
            }
            Ok(())
        })
        .register_fn("set_border", |node: &mut ScreenNode, rgba: Array| -> Result<(), Box<EvalAltResult>> {
            set_node_border(node, array_to_rgba(rgba)?);
            Ok(())
        })
        .register_fn("set_width", |node: &mut ScreenNode, width: FLOAT| set_node_layout(node, |layout| layout.width = Some(width as f32)))
        .register_fn("set_height", |node: &mut ScreenNode, height: FLOAT| set_node_layout(node, |layout| layout.height = Some(height as f32)))
        .register_fn("set_width_percent", |node: &mut ScreenNode, width: FLOAT| set_node_layout(node, |layout| layout.width_percent = Some(width as f32)))
        .register_fn("set_height_percent", |node: &mut ScreenNode, height: FLOAT| set_node_layout(node, |layout| layout.height_percent = Some(height as f32)))
        .register_fn("set_left", |node: &mut ScreenNode, left: FLOAT| set_node_layout(node, |layout| layout.left = Some(left as f32)))
        .register_fn("set_right", |node: &mut ScreenNode, right: FLOAT| set_node_layout(node, |layout| layout.right = Some(right as f32)))
        .register_fn("set_top", |node: &mut ScreenNode, top: FLOAT| set_node_layout(node, |layout| layout.top = Some(top as f32)))
        .register_fn("set_bottom", |node: &mut ScreenNode, bottom: FLOAT| set_node_layout(node, |layout| layout.bottom = Some(bottom as f32)))
        .register_fn("set_align", |node: &mut ScreenNode, align: FLOAT| set_node_align(node, align as f32))
        .register_fn("set_gap", |node: &mut ScreenNode, gap: FLOAT| set_container_gap(node, gap as f32))
        .register_fn("set_justify", |node: &mut ScreenNode, justify: ImmutableString| set_container_justify(node, justify.to_string()))
        .register_fn("set_align_items", |node: &mut ScreenNode, align: ImmutableString| set_container_align_items(node, align.to_string()))
        .register_fn("set_padding", |node: &mut ScreenNode, padding: FLOAT| set_node_padding(node, padding as f32))
        .register_fn("set_button_padding", |node: &mut ScreenNode, x: FLOAT, y: FLOAT| set_button_padding(node, x as f32, y as f32))
        .register_fn("set_button_border_width", |node: &mut ScreenNode, width: FLOAT| set_button_border_width(node, width as f32))
        .register_fn("set_button_radius", |node: &mut ScreenNode, radius: FLOAT| set_button_radius(node, radius as f32))
        .register_fn("style_color", style_color)
        .register_fn("style_number", style_number)
        .register_fn("style_bool", style_bool)
        .register_fn("keyframe", keyframe)
        .register_fn("keyframe_xy", keyframe_xy)
        .register_fn("keyframe_delta", keyframe_delta)
        .register_fn("effect_options", effect_options)
        .register_fn("effect_path", effect_path)
        .register_fn("effect_number", effect_number)
        .register_fn("effect_bool", effect_bool)
        .register_fn("effect_vec4", effect_vec4);
}

fn text_effect_spec(mode: ImmutableString) -> DialogueTextEffectSpec {
    DialogueTextEffectSpec {
        mode: Some(mode.to_string()),
        ..Default::default()
    }
}

fn choice_option_string(text: ImmutableString, value: ImmutableString) -> ChoiceOption {
    ChoiceOption {
        text: text.to_string(),
        value: StoredValue::String(value.to_string()),
    }
}

fn choice_option_dynamic(text: ImmutableString, value: Dynamic) -> Result<ChoiceOption, Box<EvalAltResult>> {
    Ok(ChoiceOption {
        text: text.to_string(),
        value: dynamic_to_stored_value(value)?,
    })
}

fn screen_spec_from_children(children: Array) -> Result<ScreenSpec, Box<EvalAltResult>> {
    Ok(ScreenSpec {
        children: screen_nodes_from_array(children)?,
        ..default_screen_spec()
    })
}

fn screen_panel_spec(width: FLOAT, background_image: ImmutableString, children: Array) -> Result<ScreenSpec, Box<EvalAltResult>> {
    Ok(ScreenSpec {
        width: Some(width as f32),
        background_image: Some(background_image.to_string()),
        children: screen_nodes_from_array(children)?,
        ..default_screen_spec()
    })
}

fn default_screen_spec() -> ScreenSpec {
    ScreenSpec {
        title: None,
        panel: true,
        width: None,
        background_image: None,
        xalign: 0.5,
        yalign: 0.5,
        padding: 24.0,
        gap: 16.0,
        overlay: None,
        background: None,
        border: None,
        children: Vec::new(),
    }
}

fn screen_nodes_from_array(children: Array) -> Result<Vec<ScreenNode>, Box<EvalAltResult>> {
    children
        .into_iter()
        .map(|value| {
            value
                .try_cast::<ScreenNode>()
                .ok_or_else(|| runtime_error("screen children must be ScreenNode values from *_node(...)"))
        })
        .collect()
}

fn text_node(text: ImmutableString, size: FLOAT) -> ScreenNode {
    ScreenNode::Text(TextNode {
        text: text.to_string(),
        size: size as f32,
        color: None,
        align: None,
        layout: ScreenLayout::default(),
    })
}

fn button_node_string(text: ImmutableString, value: ImmutableString) -> ScreenNode {
    ScreenNode::Button(ButtonNode {
        text: text.to_string(),
        value: StoredValue::String(value.to_string()),
        size: 28.0,
        color: None,
        background: None,
        border: None,
        hovered_background: None,
        pressed_background: None,
        align: None,
        padding_x: None,
        padding_y: None,
        border_width: None,
        radius: None,
        layout: ScreenLayout::default(),
    })
}

fn button_node_dynamic(text: ImmutableString, value: Dynamic) -> Result<ScreenNode, Box<EvalAltResult>> {
    let mut node = button_node_string(text, "".into());
    if let ScreenNode::Button(button) = &mut node {
        button.value = dynamic_to_stored_value(value)?;
    }
    Ok(node)
}

fn image_node(path: ImmutableString) -> ScreenNode {
    ScreenNode::Image(ScreenImageNode {
        path: path.to_string(),
        layout: ScreenLayout::default(),
    })
}

fn bar_node(value: FLOAT, width: FLOAT, height: FLOAT) -> ScreenNode {
    ScreenNode::Bar(BarNode {
        value: value as f32,
        min: 0.0,
        max: 1.0,
        width: width as f32,
        height: height as f32,
        background: None,
        fill: None,
        border: None,
    })
}

fn spacer_node(height: FLOAT) -> ScreenNode {
    ScreenNode::Spacer(SpacerNode {
        width: 0.0,
        height: height as f32,
    })
}

fn vbox_node(children: Array) -> Result<ScreenNode, Box<EvalAltResult>> {
    Ok(ScreenNode::Column(container_node(children)?))
}

fn hbox_node(children: Array) -> Result<ScreenNode, Box<EvalAltResult>> {
    Ok(ScreenNode::Row(container_node(children)?))
}

fn frame_node(children: Array) -> Result<ScreenNode, Box<EvalAltResult>> {
    vbox_node(children)
}

fn container_node(children: Array) -> Result<ContainerNode, Box<EvalAltResult>> {
    Ok(ContainerNode {
        gap: 12.0,
        padding: 0.0,
        background: None,
        border: None,
        justify: None,
        align_items: None,
        layout: ScreenLayout::default(),
        children: screen_nodes_from_array(children)?,
    })
}

fn set_node_text(node: &mut ScreenNode, text: String) {
    match node {
        ScreenNode::Text(text_node) => text_node.text = text,
        ScreenNode::Button(button) => button.text = text,
        _ => {}
    }
}

fn set_node_size(node: &mut ScreenNode, size: f32) {
    match node {
        ScreenNode::Text(text) => text.size = size,
        ScreenNode::Button(button) => button.size = size,
        _ => {}
    }
}

fn set_node_color(node: &mut ScreenNode, color: [f32; 4]) {
    match node {
        ScreenNode::Text(text) => text.color = Some(color),
        ScreenNode::Button(button) => button.color = Some(color),
        ScreenNode::Bar(bar) => bar.fill = Some(color),
        _ => {}
    }
}

fn set_node_background(node: &mut ScreenNode, color: [f32; 4]) {
    match node {
        ScreenNode::Button(button) => button.background = Some(color),
        ScreenNode::Bar(bar) => bar.background = Some(color),
        ScreenNode::Row(container) | ScreenNode::Column(container) => container.background = Some(color),
        _ => {}
    }
}

fn set_node_border(node: &mut ScreenNode, color: [f32; 4]) {
    match node {
        ScreenNode::Button(button) => button.border = Some(color),
        ScreenNode::Bar(bar) => bar.border = Some(color),
        ScreenNode::Row(container) | ScreenNode::Column(container) => container.border = Some(color),
        _ => {}
    }
}

fn set_node_layout<F>(node: &mut ScreenNode, update: F)
where
    F: FnOnce(&mut ScreenLayout),
{
    match node {
        ScreenNode::Text(text) => update(&mut text.layout),
        ScreenNode::Button(button) => update(&mut button.layout),
        ScreenNode::Image(image) => update(&mut image.layout),
        ScreenNode::Row(container) | ScreenNode::Column(container) => update(&mut container.layout),
        _ => {}
    }
}

fn set_node_align(node: &mut ScreenNode, align: f32) {
    match node {
        ScreenNode::Text(text) => text.align = Some(align),
        ScreenNode::Button(button) => button.align = Some(align),
        _ => {}
    }
}

fn set_container_gap(node: &mut ScreenNode, gap: f32) {
    if let ScreenNode::Row(container) | ScreenNode::Column(container) = node {
        container.gap = gap;
    }
}

fn set_container_justify(node: &mut ScreenNode, justify: String) {
    if let ScreenNode::Row(container) | ScreenNode::Column(container) = node {
        container.justify = Some(justify);
    }
}

fn set_container_align_items(node: &mut ScreenNode, align: String) {
    if let ScreenNode::Row(container) | ScreenNode::Column(container) = node {
        container.align_items = Some(align);
    }
}

fn set_node_padding(node: &mut ScreenNode, padding: f32) {
    if let ScreenNode::Row(container) | ScreenNode::Column(container) = node {
        container.padding = padding;
    }
}

fn set_button_padding(node: &mut ScreenNode, x: f32, y: f32) {
    if let ScreenNode::Button(button) = node {
        button.padding_x = Some(x);
        button.padding_y = Some(y);
    }
}

fn set_button_border_width(node: &mut ScreenNode, width: f32) {
    if let ScreenNode::Button(button) = node {
        button.border_width = Some(width);
    }
}

fn set_button_radius(node: &mut ScreenNode, radius: f32) {
    if let ScreenNode::Button(button) = node {
        button.radius = Some(radius);
    }
}

fn style_color(style: &mut UiStylePatch, key: ImmutableString, rgba: Array) -> Result<(), Box<EvalAltResult>> {
    let color = array_to_rgba(rgba)?;
    match key.as_str() {
        "dialogue_bg" => style.dialogue_bg = Some(color),
        "dialogue_border" => style.dialogue_border = Some(color),
        "speaker_color" => style.speaker_color = Some(color),
        "line_color" => style.line_color = Some(color),
        "hint_color" => style.hint_color = Some(color),
        "choice_panel_bg" => style.choice_panel_bg = Some(color),
        "choice_prompt_color" => style.choice_prompt_color = Some(color),
        "choice_button_bg" => style.choice_button_bg = Some(color),
        "choice_button_hovered" => style.choice_button_hovered = Some(color),
        "choice_button_pressed" => style.choice_button_pressed = Some(color),
        "choice_button_border" => style.choice_button_border = Some(color),
        "choice_text_color" => style.choice_text_color = Some(color),
        "quick_menu_bg" => style.quick_menu_bg = Some(color),
        "quick_button_bg" => style.quick_button_bg = Some(color),
        "quick_button_hovered" => style.quick_button_hovered = Some(color),
        "quick_button_pressed" => style.quick_button_pressed = Some(color),
        "quick_button_border" => style.quick_button_border = Some(color),
        "quick_text_color" => style.quick_text_color = Some(color),
        other => return Err(runtime_error(format!("unknown ui style color `{other}`"))),
    }
    Ok(())
}

fn style_number(style: &mut UiStylePatch, key: ImmutableString, value: FLOAT) -> Result<(), Box<EvalAltResult>> {
    let value = value as f32;
    match key.as_str() {
        "dialogue_left" => style.dialogue_left = Some(value),
        "dialogue_right" => style.dialogue_right = Some(value),
        "dialogue_bottom" => style.dialogue_bottom = Some(value),
        "dialogue_min_height" => style.dialogue_min_height = Some(value),
        "dialogue_padding_x" => style.dialogue_padding_x = Some(value),
        "dialogue_padding_y" => style.dialogue_padding_y = Some(value),
        "dialogue_radius" => style.dialogue_radius = Some(value),
        "speaker_size" => style.speaker_size = Some(value),
        "line_size" => style.line_size = Some(value),
        "hint_size" => style.hint_size = Some(value),
        "choice_bottom" => style.choice_bottom = Some(value),
        "choice_panel_width" => style.choice_panel_width = Some(value),
        "choice_padding" => style.choice_padding = Some(value),
        "choice_gap" => style.choice_gap = Some(value),
        "choice_prompt_size" => style.choice_prompt_size = Some(value),
        "choice_button_size" => style.choice_button_size = Some(value),
        "quick_menu_bottom" => style.quick_menu_bottom = Some(value),
        "quick_menu_gap" => style.quick_menu_gap = Some(value),
        "quick_button_size" => style.quick_button_size = Some(value),
        other => return Err(runtime_error(format!("unknown ui style number `{other}`"))),
    }
    Ok(())
}

fn style_bool(style: &mut UiStylePatch, key: ImmutableString, value: bool) -> Result<(), Box<EvalAltResult>> {
    match key.as_str() {
        "hint_visible" => style.hint_visible = Some(value),
        "choice_center_text" => style.choice_center_text = Some(value),
        "choice_show_indices" => style.choice_show_indices = Some(value),
        other => return Err(runtime_error(format!("unknown ui style bool `{other}`"))),
    }
    Ok(())
}

fn keyframe(time: FLOAT) -> CharacterAnimationKeyframeInput {
    CharacterAnimationKeyframeInput {
        time: time as f32,
        x: None,
        y: None,
        dx: None,
        dy: None,
        ease: None,
    }
}

fn keyframe_xy(time: FLOAT, x: FLOAT, y: FLOAT, ease: ImmutableString) -> CharacterAnimationKeyframeInput {
    CharacterAnimationKeyframeInput {
        time: time as f32,
        x: Some(x as f32),
        y: Some(y as f32),
        dx: None,
        dy: None,
        ease: Some(ease.to_string()),
    }
}

fn keyframe_delta(time: FLOAT, dx: FLOAT, dy: FLOAT, ease: ImmutableString) -> CharacterAnimationKeyframeInput {
    CharacterAnimationKeyframeInput {
        time: time as f32,
        x: None,
        y: None,
        dx: Some(dx as f32),
        dy: Some(dy as f32),
        ease: Some(ease.to_string()),
    }
}

fn effect_options(ms: i64) -> Result<ScriptEffectOptions, Box<EvalAltResult>> {
    Ok(ScriptEffectOptions {
        duration: duration_from_millis(ms)?,
        ..Default::default()
    })
}

fn effect_path(options: &mut ScriptEffectOptions, key: ImmutableString, path: ImmutableString) -> Result<(), Box<EvalAltResult>> {
    match key.as_str() {
        "from" => options.from_path = Some(path.to_string()),
        "to" => options.to_path = Some(path.to_string()),
        "rule" => options.rule_path = Some(path.to_string()),
        "tex0" => options.aux0_path = Some(path.to_string()),
        "tex1" => options.aux1_path = Some(path.to_string()),
        other => return Err(runtime_error(format!("unknown effect path `{other}`"))),
    }
    Ok(())
}

fn effect_number(options: &mut ScriptEffectOptions, key: ImmutableString, value: FLOAT) -> Result<(), Box<EvalAltResult>> {
    match key.as_str() {
        "mode" => options.mode = value as f32,
        other => return Err(runtime_error(format!("unknown effect number `{other}`"))),
    }
    Ok(())
}

fn effect_bool(options: &mut ScriptEffectOptions, key: ImmutableString, value: bool) -> Result<(), Box<EvalAltResult>> {
    match key.as_str() {
        "commit_to_bg" => options.commit_to_bg = value,
        other => return Err(runtime_error(format!("unknown effect bool `{other}`"))),
    }
    Ok(())
}

fn effect_vec4(options: &mut ScriptEffectOptions, key: ImmutableString, value: Array) -> Result<(), Box<EvalAltResult>> {
    let value = array_to_vec4(value)?;
    match key.as_str() {
        "p0" => options.p0 = value,
        "p1" => options.p1 = value,
        "p2" => options.p2 = value,
        "p3" => options.p3 = value,
        other => return Err(runtime_error(format!("unknown effect vec4 `{other}`"))),
    }
    Ok(())
}

fn array_to_rgba(array: Array) -> Result<[f32; 4], Box<EvalAltResult>> {
    if array.len() != 3 && array.len() != 4 {
        return Err(runtime_error("rgba arrays must contain three or four numbers"));
    }
    let mut values = [0.0, 0.0, 0.0, 1.0];
    for (index, value) in array.into_iter().enumerate() {
        values[index] = dynamic_to_f32(value)?;
    }
    Ok(values)
}

fn array_to_vec4(array: Array) -> Result<Vec4, Box<EvalAltResult>> {
    if array.len() != 4 {
        return Err(runtime_error("vec4 arrays must contain four numbers"));
    }
    let mut values = [0.0; 4];
    for (index, value) in array.into_iter().enumerate() {
        values[index] = dynamic_to_f32(value)?;
    }
    Ok(Vec4::new(values[0], values[1], values[2], values[3]))
}

fn resolve_custom_effect_options(
    host: &ScriptHost,
    options: ScriptEffectOptions,
) -> Result<CustomEffectOptions, Box<EvalAltResult>> {
    let current_background = host.current_background_path();
    let from_path = options
        .from_path
        .or_else(|| current_background.clone())
        .ok_or_else(|| runtime_error("effect options require `from` or an existing background"))?;
    let to_path = options.to_path.unwrap_or_else(|| from_path.clone());
    let rule_path = options.rule_path.unwrap_or_else(|| from_path.clone());
    let aux0_path = options.aux0_path.unwrap_or_else(|| from_path.clone());
    let aux1_path = options.aux1_path.unwrap_or_else(|| from_path.clone());

    Ok(CustomEffectOptions {
        from_path: host.resolve_path(&from_path),
        to_path: host.resolve_path(&to_path),
        rule_path: host.resolve_path(&rule_path),
        aux0_path: host.resolve_path(&aux0_path),
        aux1_path: host.resolve_path(&aux1_path),
        duration: options.duration,
        mode: options.mode,
        p0: options.p0,
        p1: options.p1,
        p2: options.p2,
        p3: options.p3,
        commit_to_bg: options.commit_to_bg,
    })
}

fn resolve_screen_spec_paths(host: &ScriptHost, screen: &mut ScreenSpec) {
    if let Some(path) = screen.background_image.as_mut() {
        *path = host.resolve_path(path);
    }
    for child in &mut screen.children {
        resolve_screen_node_paths(host, child);
    }
}

fn resolve_screen_node_paths(host: &ScriptHost, node: &mut ScreenNode) {
    match node {
        ScreenNode::Image(image) => image.path = host.resolve_path(&image.path),
        ScreenNode::Row(container) | ScreenNode::Column(container) => {
            for child in &mut container.children {
                resolve_screen_node_paths(host, child);
            }
        }
        _ => {}
    }
}

fn extract_script_flow_action(error: &EvalAltResult) -> Option<ScriptFlowAction> {
    match error {
        EvalAltResult::ErrorRuntime(value, _) => extract_signal_string(value),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => extract_script_flow_action(inner),
        _ => None,
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
    let chunks = parse_inline_dialogue_chunks(&text)?;
    if chunks.iter().all(|chunk| matches!(chunk, InlineDialogueChunk::Text(_))) {
        if host.is_batch_mode() {
            let _ = run_blocking_or_collected(host, "say", |animation_id, done| ScriptCommand::Say {
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
        return Err(runtime_error("inline dialogue commands are not supported inside seq/par"));
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

fn parse_inline_dialogue_chunks(text: &str) -> Result<Vec<InlineDialogueChunk>, Box<EvalAltResult>> {
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

fn parse_character_animation_keyframes(
    keyframes: Array,
    current: Vec2,
) -> Result<Vec<ResolvedCharacterKeyframe>, Box<EvalAltResult>> {
    if keyframes.is_empty() {
        return Err(runtime_error("animate requires at least one keyframe"));
    }

    let inputs = keyframes
        .into_iter()
        .map(|value| {
            value
                .try_cast::<CharacterAnimationKeyframeInput>()
                .ok_or_else(|| runtime_error("animate keyframes must be CharacterKeyframe values from keyframe(...)"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut resolved = Vec::with_capacity(inputs.len());
    let mut previous_time = 0.0f32;
    let mut cursor = current;

    for input in inputs {
        if input.time < 0.0 {
            return Err(runtime_error("animate keyframe time cannot be negative"));
        }
        if input.time < previous_time {
            return Err(runtime_error("animate keyframes must be sorted by time ascending"));
        }

        let position = Vec2::new(
            input.x.unwrap_or(cursor.x + input.dx.unwrap_or(0.0)),
            input.y.unwrap_or(cursor.y + input.dy.unwrap_or(0.0)),
        );
        let ease = parse_character_ease(input.ease.as_deref().unwrap_or("linear"))?;

        resolved.push(ResolvedCharacterKeyframe {
            time: input.time,
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

fn dynamic_to_f32(value: Dynamic) -> Result<f32, Box<EvalAltResult>> {
    if value.is::<FLOAT>() {
        return Ok(value.cast::<FLOAT>() as f32);
    }
    if value.is::<INT>() {
        return Ok(value.cast::<INT>() as f32);
    }

    Err(runtime_error("expected a numeric value"))
}

fn parse_choice_option(option: Dynamic) -> Result<ChoiceOption, Box<EvalAltResult>> {
    if option.is::<ChoiceOption>() {
        return Ok(option.cast::<ChoiceOption>());
    }

    if option.is_string() {
        let text = option.cast::<ImmutableString>().to_string();
        return Ok(ChoiceOption {
            value: StoredValue::String(text.clone()),
            text,
        });
    }

    Err(runtime_error(
        "choice option must be a string or ChoiceOption from choice_option(...) ",
    ))
}

fn parse_animation_ids(ids: Array) -> Result<Vec<String>, Box<EvalAltResult>> {
    ids.into_iter()
        .map(|value| {
            if value.is_string() {
                Ok(value.cast::<ImmutableString>().to_string())
            } else {
                Err(runtime_error("animation handle array must contain only strings"))
            }
        })
        .collect()
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
        let response = done_rx
            .recv()
            .map_err(|err| runtime_error(format!("engine stopped while waiting for command: {err}")));
        host.set_inline_current_handle(None);
        match response? {
            ScriptResponse::Continue => Ok(Dynamic::UNIT),
            ScriptResponse::Choice(_) => Err(runtime_error("engine returned unexpected choice response")),
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
        return Ok(StoredValue::String(value.cast::<ImmutableString>().to_string()));
    }

    Err(runtime_error(
        "only bool, int, float and string values are supported here",
    ))
}

fn stored_value_to_dynamic(value: StoredValue) -> Dynamic {
    match value {
        StoredValue::Bool(value) => value.into(),
        StoredValue::Int(value) => value.into(),
        StoredValue::Float(value) => value.into(),
        StoredValue::String(value) => value.into(),
    }
}

fn sanitize_slot_name(slot: &str) -> Result<String, Box<EvalAltResult>> {
    let sanitized = slot
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();

    if sanitized.is_empty() {
        Err(runtime_error("save slot must contain at least one valid character"))
    } else {
        Ok(sanitized)
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
