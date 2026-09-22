use std::time::Duration;

use bevy::math::{Vec2, Vec3};

use crate::script::navigation::NavigationRequest;
use crate::ui::ScreenSpec;

use super::{CameraEffectScope, CameraProjectionMode, ScriptRequestId};

/// An ordered, engine-facing command produced by the story runtime.
///
/// The envelope deliberately remains a single enum so commands from different
/// domains retain their source order. Each payload is domain-specific, keeping
/// audio, UI, dialogue, and scene APIs from growing into one flat protocol.
#[derive(Debug)]
pub enum ScriptCommand {
    Runtime(RuntimeCommand),
    Stage(StageCommand),
    Dialogue(DialogueCommand),
    Camera(CameraCommand),
    Settings(SettingsCommand),
    Ui(UiCommand),
    Character(CharacterCommand),
    Animation(AnimationCommand),
    Audio(AudioCommand),
    Video(VideoCommand),
}

#[derive(Debug)]
pub enum RuntimeCommand {
    SaveSlot(String),
    Log(String),
    Navigate(NavigationRequest),
    Exit,
}

#[derive(Debug)]
pub enum StageCommand {
    Spatial(crate::stage::runtime::StageCommand),
    SetActorDepth {
        id: String,
        depth: f32,
    },
    Clip(crate::scene::clipping::ClipCommand),
    AwaitCurtain {
        done: ScriptRequestId,
    },
    Picture(crate::scene::pictures::PictureCommand),
    SetCurtain {
        color: [u8; 3],
        opacity: f32,
        fade: Option<Duration>,
        mask: Option<String>,
        softness: f32,
    },
    SetBackground {
        path: String,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum DialogueCommand {
    Speed(f32),
    Say {
        speaker: String,
        text: String,
        animation_id: Option<String>,
    },
    Continue {
        text: String,
        animation_id: Option<String>,
    },
    AwaitAdvance {
        done: ScriptRequestId,
    },
    Clear,
}

#[derive(Debug)]
pub enum CameraCommand {
    Shake {
        amplitude: Vec2,
        interval: f32,
        duration: Duration,
        animation_id: Option<String>,
    },
    Set {
        blur_intensity: Option<f32>,
        zoom: Option<f32>,
        zoom_view_space: bool,
        offset: Option<Vec3>,
        rotation: Option<Vec3>,
        projection: Option<CameraProjectionMode>,
        scope: CameraEffectScope,
        duration: Duration,
        ease: CharacterEase,
        animation_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum SettingsCommand {
    Preference(crate::storage::PreferenceChange),
    AutoDialogue(bool),
    FastForward(bool),
    Adjust { name: String, delta: f32 },
    Set { name: String, value: f32 },
}

#[derive(Debug)]
pub enum UiCommand {
    ShowScreen {
        screen: ScreenSpec,
        done: Option<ScriptRequestId>,
        push: bool,
    },
    ShowOverlay {
        name: String,
        screen: ScreenSpec,
        lifetime: Option<f32>,
    },
    HideOverlay {
        name: String,
    },
}

#[derive(Debug)]
pub enum CharacterCommand {
    StopMotion {
        actor_id: String,
    },
    Motion {
        actor_id: String,
        revision: u64,
        transition: super::actor_motion::ActorOffset,
        animation_id: Option<String>,
    },
    Hide {
        actor_id: Option<String>,
        fade_ms: u64,
    },
    Show {
        rotation: f32,
        placement_animation: Option<super::animation::AnimationSpec>,
        actor_id: String,
        character_name: String,
        expressions: Vec<String>,
        position: Vec2,
        scale: f32,
        focused: bool,
        fade: Option<Duration>,
        animation_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum AnimationCommand {
    Scene {
        effect: crate::scene::effect_wait::SceneEffect,
        done: ScriptRequestId,
    },
    Delay {
        duration: Duration,
        done: ScriptRequestId,
    },
    Wait {
        ids: Vec<String>,
        done: ScriptRequestId,
    },
}

#[derive(Debug)]
pub enum AudioCommand {
    StopSfxChannel {
        channel: String,
        fade: Duration,
    },
    PlaySfx {
        channel: Option<String>,
        looped: bool,
        path: String,
        volume: f32,
        fade_in: Option<Duration>,
        animation_id: Option<String>,
    },
    PlayBgm {
        path: String,
        prelude: Option<String>,
        volume: f32,
        fade_in: Option<Duration>,
        animation_id: Option<String>,
    },
    StopBgm {
        fade: Duration,
        animation_id: Option<String>,
    },
    PlayVoice {
        path: String,
        volume: f32,
        mode: VoicePlaybackMode,
        animation_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum VideoCommand {
    Play {
        path: String,
        layout: hiraku_video::AlphaLayout,
        done: Option<ScriptRequestId>,
        fade_out: Duration,
    },
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePlaybackMode {
    Exclusive,
    Concurrent,
}

#[derive(Debug, Clone)]
pub struct ResolvedCharacterKeyframe {
    pub time: f32,
    pub position: Vec2,
    pub ease: CharacterEase,
}

pub type CharacterEase = crate::script::animation::Easing;
