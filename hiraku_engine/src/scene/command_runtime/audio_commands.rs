use super::super::audio_runtime::{NamedSfxChannel, SfxChannel, SfxCompletion, StopSoundAfterFade};
use super::*;
use std::time::Duration;

fn stop_sfx_channel(world: &mut World, channel: &str, duration: Duration) {
    let entities: Vec<_> = world
        .query::<(Entity, &NamedSfxChannel)>()
        .iter(world)
        .filter_map(|(entity, name)| (name.0 == channel).then_some(entity))
        .collect();
    for entity in entities {
        if !duration.is_zero() && world.get::<AudioSink>(entity).is_some() {
            let volume = world
                .get::<AudioFade>(entity)
                .map(|fade| fade.from + (fade.to - fade.from) * tween_fraction(&fade.timer))
                .unwrap_or_else(|| {
                    world
                        .get::<SfxChannel>(entity)
                        .map_or(1.0, |sound| sound.volume)
                });
            world.entity_mut(entity).insert((
                AudioFade {
                    from: volume,
                    to: 0.0,
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id: None,
                },
                StopSoundAfterFade,
            ));
        } else {
            // Includes cancellation before AssetServer has produced a sink.
            let completion = world
                .get::<SfxCompletion>(entity)
                .and_then(|value| value.animation_id.clone());
            if let Some(id) = completion {
                world.resource_mut::<AnimationState>().completed.insert(id);
            }
            world.despawn(entity);
        }
    }
}

fn retire_bgm(commands: &mut Commands, entity: Entity) {
    commands.queue(move |world: &mut World| {
        let completion = world
            .get::<AudioFade>(entity)
            .and_then(|fade| fade.animation_id.clone());
        if let Some(id) = completion {
            world.resource_mut::<AnimationState>().completed.insert(id);
        }
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    });
}

fn stop_music(world: &mut World, entity: Option<Entity>, duration: Duration, done: Option<String>) {
    if let Some(entity) = entity {
        let previous_fade = world.get::<AudioFade>(entity);
        let previous_done = previous_fade.and_then(|fade| fade.animation_id.clone());
        let volume = previous_fade
            .map(|fade| fade.from + (fade.to - fade.from) * tween_fraction(&fade.timer))
            .unwrap_or_else(|| {
                world
                    .get::<BgmChannel>(entity)
                    .map_or(1.0, |bgm| bgm.volume)
            });
        if let Some(id) = previous_done {
            world.resource_mut::<AnimationState>().completed.insert(id);
        }
        if !duration.is_zero() && world.get::<AudioSink>(entity).is_some() {
            world.entity_mut(entity).insert((
                AudioFade {
                    from: volume,
                    to: 0.0,
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id: done,
                },
                StopSoundAfterFade,
            ));
            return;
        }
        // Cancel loading/prelude preparation too: a stopped track must not
        // become audible later, and an absent sink must not strand its waiter.
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    if let Some(id) = done {
        world.resource_mut::<AnimationState>().completed.insert(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn stopping_loading_music_settles_old_and_new_waiters() {
        let mut world = World::new();
        world.init_resource::<AnimationState>();
        let entity = world
            .spawn(AudioFade {
                from: 0.0,
                to: 0.8,
                timer: Timer::from_seconds(1.0, TimerMode::Once),
                animation_id: Some("fade-in".into()),
            })
            .id();
        stop_music(
            &mut world,
            Some(entity),
            Duration::from_secs(3),
            Some("fade-out".into()),
        );
        assert!(world.get_entity(entity).is_err());
        let completed = &world.resource::<AnimationState>().completed;
        assert!(completed.contains("fade-in"));
        assert!(completed.contains("fade-out"));
        stop_music(
            &mut world,
            None,
            Duration::from_secs(3),
            Some("already-stopped".into()),
        );
        assert!(
            world
                .resource::<AnimationState>()
                .completed
                .contains("already-stopped")
        );
    }
    #[test]
    fn stopping_pending_sound_settles_only_its_named_channel() {
        let mut world = World::new();
        world.init_resource::<AnimationState>();
        let alice = world
            .spawn((
                NamedSfxChannel("alice".into()),
                SfxCompletion {
                    animation_id: Some("alice-playback".into()),
                },
            ))
            .id();
        let bob = world
            .spawn((
                NamedSfxChannel("bob".into()),
                SfxCompletion {
                    animation_id: Some("bob-playback".into()),
                },
            ))
            .id();
        stop_sfx_channel(&mut world, "alice", Duration::from_secs(1));
        assert!(world.get_entity(alice).is_err());
        assert!(world.get_entity(bob).is_ok());
        assert!(
            world
                .resource::<AnimationState>()
                .completed
                .contains("alice-playback")
        );
        assert!(
            !world
                .resource::<AnimationState>()
                .completed
                .contains("bob-playback")
        );
        stop_sfx_channel(&mut world, "alice", Duration::ZERO);
    }
    #[test]
    fn replacing_music_releases_its_fade_wait() {
        let mut world = World::new();
        world.init_resource::<AnimationState>();
        let entity = world
            .spawn(AudioFade {
                from: 0.0,
                to: 1.0,
                timer: Timer::new(Duration::from_secs(1), TimerMode::Once),
                animation_id: Some("fade-alice".into()),
            })
            .id();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        retire_bgm(&mut commands, entity);
        queue.apply(&mut world);
        assert!(world.get_entity(entity).is_err());
        assert!(
            world
                .resource::<AnimationState>()
                .completed
                .contains("fade-alice")
        );
    }
}

pub(super) fn dispatch_audio_command(
    command: AudioCommand,
    commands: &mut Commands,
    asset_server: &AssetServer,
    user_settings: &UserSettings,
    stage: &mut StageState,
    shared_state: &mut SceneSharedState,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    match command {
        AudioCommand::StopSfxChannel { channel, fade } => {
            commands.queue(move |world: &mut World| stop_sfx_channel(world, &channel, fade));
        }
        AudioCommand::PlaySfx {
            channel,
            looped,
            path,
            volume,
            fade_in,
            animation_id,
        } => {
            if let Some(name) = channel.clone() {
                commands
                    .queue(move |world: &mut World| stop_sfx_channel(world, &name, Duration::ZERO));
            }
            let target_volume = apply_volume_setting(
                volume,
                user_settings.sfx_volume * user_settings.master_volume,
            );
            let start_volume = if fade_in.is_some() {
                0.0
            } else {
                target_volume
            };
            let mut entity = commands.spawn((
                super::super::audio_runtime::SfxChannel { volume },
                super::super::audio_runtime::SfxCompletion { animation_id },
                bevy::audio::AudioPlayer::<AudioSource>(asset_server.load(path)),
                (if looped {
                    PlaybackSettings::LOOP
                } else {
                    PlaybackSettings::ONCE
                })
                .with_volume(Volume::Linear(start_volume)),
            ));
            if let Some(channel) = channel {
                entity.insert(NamedSfxChannel(channel));
            }
            if let Some(duration) = fade_in {
                entity.insert(AudioFade {
                    from: 0.0,
                    to: volume,
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id: None,
                });
            }
        }
        AudioCommand::PlayBgm {
            path,
            prelude,
            volume,
            fade_in,
            animation_id,
        } => {
            let playback_volume = apply_volume_setting(
                volume,
                user_settings.bgm_volume * user_settings.master_volume,
            );
            if let Some(previous) = stage.bgm.take() {
                retire_bgm(commands, previous);
            }
            let start_volume = if fade_in.is_some() {
                0.0
            } else {
                playback_volume
            };
            let loop_audio = asset_server.load(path.clone());
            let bgm = if let Some(prelude) = prelude {
                commands
                    .spawn((
                        BgmChannel {
                            path: path.clone(),
                            volume,
                        },
                        BgmPrelude {
                            prelude_audio: asset_server.load(prelude),
                            loop_audio,
                            start_volume,
                        },
                    ))
                    .id()
            } else {
                commands
                    .spawn((
                        BgmChannel {
                            path: path.clone(),
                            volume,
                        },
                        bevy::audio::AudioPlayer::<AudioSource>(loop_audio),
                        PlaybackSettings::LOOP.with_volume(Volume::Linear(start_volume)),
                    ))
                    .id()
            };
            if let Some(fade_in) = fade_in {
                commands.entity(bgm).insert(AudioFade {
                    from: 0.0,
                    to: volume,
                    timer: Timer::new(fade_in, TimerMode::Once),
                    animation_id,
                });
            } else if let Some(animation_id) = animation_id {
                animations.completed.insert(animation_id);
            }
            stage.bgm = Some(bgm);
            shared_state.0.bgm = Some(AudioSnapshot { path, volume });
        }
        AudioCommand::StopBgm { fade, animation_id } => {
            let previous = stage.bgm.take();
            commands
                .queue(move |world: &mut World| stop_music(world, previous, fade, animation_id));
            shared_state.0.bgm = None;
        }
        AudioCommand::PlayVoice {
            path,
            volume,
            mode,
            animation_id,
        } => {
            let playback_volume = apply_volume_setting(
                volume,
                user_settings.voice_volume * user_settings.master_volume,
            );
            if mode == VoicePlaybackMode::Exclusive {
                finish_active_voice(commands, animations, voice_state);
            }
            let voice = commands
                .spawn((
                    VoiceChannel {
                        path: path.clone(),
                        volume,
                    },
                    bevy::audio::AudioPlayer::<AudioSource>(asset_server.load(path)),
                    PlaybackSettings::ONCE.with_volume(Volume::Linear(playback_volume)),
                ))
                .id();
            let active = ActiveVoice {
                entity: voice,
                animation_id,
            };
            match mode {
                VoicePlaybackMode::Exclusive => voice_state.active = Some(active),
                VoicePlaybackMode::Concurrent => {
                    voice_state.concurrent.insert(voice, active);
                }
            }
        }
    }
}
