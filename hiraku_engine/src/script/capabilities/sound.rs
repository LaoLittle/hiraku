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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PendingSound {
    channel: Channel,
    path: String,
    volume: f32,
    fade_in_ms: Option<u64>,
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SoundState {
    next: u64,
    pending: BTreeMap<u64, PendingSound>,
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
            PendingSound {
                channel,
                path,
                volume: 1.0,
                fade_in_ms: None,
            },
        );
        Ok(AudioHandle(self.next))
    }

    fn pending(&mut self, id: u64) -> Result<&mut PendingSound, NativeError> {
        self.pending.get_mut(&id).ok_or_else(|| {
            NativeError::message("audio builder was committed; start a new bgm()/sfx() statement")
        })
    }

    pub(super) fn commit(&mut self, effects: &mut Vec<StoryEffect>) {
        for (_, sound) in std::mem::take(&mut self.pending) {
            let PendingSound {
                channel,
                path,
                volume,
                fade_in_ms,
            } = sound;
            effects.push(match channel {
                Channel::Music => StoryEffect::PlayBgm {
                    path,
                    volume,
                    fade_in_ms,
                },
                Channel::Sfx => StoryEffect::PlaySfx {
                    path,
                    volume,
                    fade_in_ms,
                },
            });
        }
    }
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    api::register_hks(registry).expect("audio API registration must be internally consistent");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::capabilities::{StoryWait, compile_story_bytecode};
    use crate::script::{StoryRuntime, StoryRuntimeEvent};

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

    #[hks(name = "bgm")]
    fn bgm(context: &mut CharacterContext, path: String) -> Result<AudioHandle, NativeError> {
        context.sound.begin(Channel::Music, path)
    }

    #[hks(name = "sfx")]
    fn sfx(context: &mut CharacterContext, path: String) -> Result<AudioHandle, NativeError> {
        context.sound.begin(Channel::Sfx, path)
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
        let duration = Duration::try_from_secs_f64(milliseconds / 1000.0).map_err(|_| {
            NativeError::message("audio fade duration must be finite and non-negative")
        })?;
        let millis = u64::try_from(duration.as_millis())
            .map_err(|_| NativeError::message("audio fade duration exceeds supported range"))?;
        context.sound.pending(id)?.fade_in_ms = (millis > 0).then_some(millis);
        Ok(AudioHandle(id))
    }
}
