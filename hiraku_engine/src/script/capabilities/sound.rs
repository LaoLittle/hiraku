//! Common statement-scoped audio settings, with explicit playback semantics at
//! the factory: bgm loops/exclusively replaces music; sfx is one-shot/concurrent.
use super::{CharacterContext, StoryEffect};
use hiraku_script::native::{NativeError, NativeRegistry};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "AudioPlayback", handle_type = 2)]
struct AudioHandle(u64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum Channel {
    Music,
    Sfx,
    Voice,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingSound {
    channel: Channel,
    path: String,
    volume: f32,
    fade_in_ms: Option<u64>,
    channel_name: Option<String>,
    looped: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum PendingAudio {
    Playback(PendingSound),
    StopMusic { fade_ms: u64 },
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SoundState {
    next: u64,
    pending: BTreeMap<u64, PendingAudio>,
}

impl SoundState {
    fn begin(&mut self, channel: Channel, path: String) -> Result<AudioHandle, NativeError> {
        if path.trim().is_empty() {
            return Err(NativeError::message("audio key must not be empty"));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("audio handles exhausted"))?;
        self.pending.insert(
            self.next,
            PendingAudio::Playback(PendingSound {
                channel,
                path,
                volume: 1.0,
                fade_in_ms: None,
                channel_name: None,
                looped: false,
            }),
        );
        Ok(AudioHandle(self.next))
    }

    fn pending(&mut self, id: u64) -> Result<&mut PendingSound, NativeError> {
        let pending = self.pending.get_mut(&id).ok_or_else(|| {
            NativeError::message("audio builder was committed; start a new bgm()/sfx() statement")
        })?;
        match pending {
            PendingAudio::Playback(sound) => Ok(sound),
            PendingAudio::StopMusic { .. } => Err(NativeError::message(
                "playback modifiers cannot modify stopBgm",
            )),
        }
    }

    pub(super) fn commit(&mut self, effects: &mut Vec<StoryEffect>) {
        for (_, sound) in std::mem::take(&mut self.pending) {
            let sound = match sound {
                PendingAudio::Playback(sound) => sound,
                PendingAudio::StopMusic { fade_ms } => {
                    effects.push(StoryEffect::StopBgm { fade_ms });
                    continue;
                }
            };
            let PendingSound {
                channel,
                path,
                volume,
                fade_in_ms,
                channel_name,
                looped,
            } = sound;
            effects.push(match channel {
                Channel::Music => StoryEffect::PlayBgm {
                    path,
                    volume,
                    fade_in_ms,
                },
                Channel::Sfx if channel_name.is_some() => StoryEffect::PlaySfxChannel {
                    channel: channel_name.expect("named channel matched"),
                    path,
                    volume,
                    fade_in_ms,
                    looped,
                },
                Channel::Sfx => StoryEffect::PlaySfx {
                    path,
                    volume,
                    fade_in_ms,
                },
                Channel::Voice => StoryEffect::PlayVoice { path, volume },
            });
        }
    }
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    api::register_hks(registry).expect("audio API registration must be internally consistent");
    channel_api::register_hks(registry).expect("audio channel API registration must be consistent");
}

#[hiraku_script::hks_module("audio")]
mod channel_api {
    use super::*;
    #[hks(name = "stop")]
    fn stop(
        context: &mut CharacterContext,
        channel: String,
        milliseconds: Option<f64>,
    ) -> Result<(), NativeError> {
        if channel.trim().is_empty() {
            return Err(NativeError::message("audio channel must not be empty"));
        }
        let duration =
            Duration::try_from_secs_f64(milliseconds.unwrap_or(0.0) / 1000.0).map_err(|_| {
                NativeError::message("audio stop duration must be finite and non-negative")
            })?;
        let fade_ms = u64::try_from(duration.as_millis())
            .map_err(|_| NativeError::message("audio stop duration is too large"))?;
        context
            .commands
            .push(StoryEffect::StopSfxChannel { channel, fade_ms });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::capabilities::{StoryWait, compile_story_bytecode};
    use crate::script::{StoryRuntime, StoryRuntimeEvent};

    #[test]
    fn music_stop_can_be_awaited_and_restored() {
        let code = compile_story_bytecode("music.hks", "stopBgm(3000).await()\n\"Alice\"")
            .expect("typed stop builder");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect { effect, .. }) =
            runtime.step().expect("stop effect")
        else {
            panic!("expected awaitable music stop")
        };
        assert_eq!(effect, StoryEffect::StopBgm { fade_ms: 3000 });
        assert!(runtime.step().expect("fade still active").is_none());
        let snapshot = runtime.snapshot().expect("save during fade");
        let mut restored = StoryRuntime::restore(code, snapshot).expect("restore");
        let Some(StoryRuntimeEvent::TaskEffect { task, effect }) =
            restored.step().expect("restored stop")
        else {
            panic!("expected restored stop wait")
        };
        restored
            .complete_task_effect(task, &effect)
            .expect("fade completes");
        assert!(matches!(
            restored.step().expect("dialogue"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
    }

    #[test]
    fn pending_audio_commits_once_in_source_order() {
        let mut state = SoundState::default();
        state
            .begin(Channel::Music, "music/theme".into())
            .expect("valid music key");
        let sound = state
            .begin(Channel::Sfx, "sound/bell".into())
            .expect("valid sound key");
        state.pending(sound.0).expect("pending sound").volume = 0.5;
        let mut effects = Vec::new();
        state.commit(&mut effects);
        state.commit(&mut effects);
        assert_eq!(
            effects,
            vec![
                StoryEffect::PlayBgm {
                    path: "music/theme".into(),
                    volume: 1.0,
                    fade_in_ms: None
                },
                StoryEffect::PlaySfx {
                    path: "sound/bell".into(),
                    volume: 0.5,
                    fade_in_ms: None
                },
            ]
        );
        assert!(state.pending(sound.0).is_err());
        assert!(state.begin(Channel::Sfx, " ".into()).is_err());
    }

    #[test]
    fn named_loop_and_stop_are_nonblocking_ordered_effects() {
        let bytecode = compile_story_bytecode(
            "audio.hks",
            r#"
            sfx("sound/wind").channel("ambient").looped().fadeIn(200)
            audio.stop("ambient", 1000)
            "Alice hears silence."
        "#,
        )
        .expect("named channel API must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("valid runtime");
        assert!(matches!(runtime.step().expect("start sound"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::PlaySfxChannel {
                channel, looped: true, fade_in_ms: Some(200), ..
            })) if channel == "ambient"));
        assert!(matches!(runtime.step().expect("stop sound"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::StopSfxChannel {
                channel, fade_ms: 1000,
            })) if channel == "ambient"));
        assert!(matches!(
            runtime.step().expect("next dialogue"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
    }

    #[test]
    fn fluent_sound_does_not_block_dialogue() {
        let bytecode = compile_story_bytecode(
            "audio.hks",
            r#"
            sfx("sound/bell").volume(0.5).fadeIn(200)
            "Alice heard a bell."
        "#,
        )
        .expect("audio builders must type-check");
        let mut runtime = StoryRuntime::new(bytecode).expect("valid runtime");
        assert_eq!(
            runtime.step().expect("sound event"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::PlaySfx {
                path: "sound/bell".into(),
                volume: 0.5,
                fade_in_ms: Some(200)
            }))
        );
        assert!(matches!(
            runtime.step().expect("dialogue event"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Say { .. }))
        ));
        assert_eq!(
            runtime.step().expect("dialogue wait"),
            Some(StoryRuntimeEvent::Wait(StoryWait::DialogueAdvance))
        );
    }

    #[test]
    fn parallel_sounds_start_before_either_completes() {
        let bytecode = compile_story_bytecode(
            "audio.hks",
            r#"
            par { sfx("sound/first"); sfx("sound/second") }
            "Bob is listening."
        "#,
        )
        .expect("parallel sounds must compile");
        let mut runtime = StoryRuntime::new(bytecode).expect("valid runtime");
        runtime.step().expect("dialogue event");
        runtime.step().expect("dialogue wait");
        let mut tasks = Vec::new();
        for expected in ["sound/first", "sound/second"] {
            match runtime.step().expect("concurrent sound event") {
                Some(StoryRuntimeEvent::TaskEffect {
                    task,
                    effect: StoryEffect::PlaySfx { path, .. },
                }) => {
                    assert_eq!(path, expected);
                    tasks.push(task);
                }
                event => panic!("expected sound, got {event:?}"),
            }
        }
        assert_eq!(tasks[0], tasks[1]);
        for task in tasks {
            runtime.resume_task(task).expect("sound completion");
        }
        assert_eq!(runtime.step().expect("idle while dialogue waits"), None);
    }
}

#[hiraku_script::hks_module]
mod api {
    use super::*;

    #[hks(name = "stopBgm")]
    fn stop_bgm(
        context: &mut CharacterContext,
        milliseconds: Option<f64>,
    ) -> Result<AudioHandle, NativeError> {
        let duration =
            Duration::try_from_secs_f64(milliseconds.unwrap_or(0.0) / 1000.0).map_err(|_| {
                NativeError::message("music fade duration must be finite and non-negative")
            })?;
        let fade_ms = u64::try_from(duration.as_millis())
            .map_err(|_| NativeError::message("music fade duration exceeds supported range"))?;
        let state = &mut context.sound;
        state.next = state
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("audio handles exhausted"))?;
        state
            .pending
            .insert(state.next, PendingAudio::StopMusic { fade_ms });
        Ok(AudioHandle(state.next))
    }

    #[hks(name = "bgm")]
    fn bgm(context: &mut CharacterContext, path: String) -> Result<AudioHandle, NativeError> {
        context.sound.begin(Channel::Music, path)
    }

    #[hks(name = "sfx")]
    fn sfx(context: &mut CharacterContext, path: String) -> Result<AudioHandle, NativeError> {
        context.sound.begin(Channel::Sfx, path)
    }

    /// Assign an independent, replaceable sound channel. Looping requires a
    /// named owner so a later statement can stop it without affecting others.
    #[hks(name = "channel", receiver)]
    fn channel(
        context: &mut CharacterContext,
        AudioHandle(id): AudioHandle,
        name: String,
    ) -> Result<AudioHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("audio channel must not be empty"));
        }
        let sound = context.sound.pending(id)?;
        if sound.channel != Channel::Sfx {
            return Err(NativeError::message("channel is supported on sfx playback"));
        }
        sound.channel_name = Some(name);
        Ok(AudioHandle(id))
    }

    #[hks(name = "looped", receiver)]
    fn looped(
        context: &mut CharacterContext,
        AudioHandle(id): AudioHandle,
    ) -> Result<AudioHandle, NativeError> {
        let sound = context.sound.pending(id)?;
        if sound.channel_name.is_none() {
            return Err(NativeError::message(
                "looped requires sfx(...).channel(name)",
            ));
        }
        sound.looped = true;
        Ok(AudioHandle(id))
    }

    #[hks(name = "voice")]
    fn voice(context: &mut CharacterContext, path: String) -> Result<AudioHandle, NativeError> {
        context.sound.begin(Channel::Voice, path)
    }

    #[hks(name = "await", selector = "AudioPlayback", receiver)]
    fn await_audio(
        context: &mut CharacterContext,
        AudioHandle(id): AudioHandle,
    ) -> Result<(), NativeError> {
        if !context.sound.pending.contains_key(&id) {
            return Err(NativeError::message("audio builder has already committed"));
        }
        context.await_effects = true;
        Ok(())
    }

    #[hks(name = "volume", receiver)]
    fn volume(
        context: &mut CharacterContext,
        AudioHandle(id): AudioHandle,
        volume: f64,
    ) -> Result<AudioHandle, NativeError> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(NativeError::message("audio volume must be between 0 and 1"));
        }
        context.sound.pending(id)?.volume = volume as f32;
        Ok(AudioHandle(id))
    }

    #[hks(name = "fadeIn", receiver)]
    fn fade_in(
        context: &mut CharacterContext,
        AudioHandle(id): AudioHandle,
        milliseconds: f64,
    ) -> Result<AudioHandle, NativeError> {
        if context.sound.pending(id)?.channel == Channel::Voice {
            return Err(NativeError::message("voice does not support fadeIn"));
        }
        let duration = Duration::try_from_secs_f64(milliseconds / 1000.0).map_err(|_| {
            NativeError::message("audio fade duration must be finite and non-negative")
        })?;
        let millis = u64::try_from(duration.as_millis())
            .map_err(|_| NativeError::message("audio fade duration exceeds supported range"))?;
        context.sound.pending(id)?.fade_in_ms = (millis > 0).then_some(millis);
        Ok(AudioHandle(id))
    }
}
