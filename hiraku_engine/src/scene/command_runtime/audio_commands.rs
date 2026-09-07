use super::*;

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
        AudioCommand::PlaySfx {
            path,
            volume,
            fade_in,
            animation_id,
        } => {
            let target_volume = apply_volume_setting(volume, user_settings.sfx_volume * user_settings.master_volume);
            let start_volume = if fade_in.is_some() {
                0.0
            } else {
                target_volume
            };
            let mut entity = commands.spawn((
                super::super::audio_runtime::SfxChannel { volume },
                super::super::audio_runtime::SfxCompletion { animation_id },
                AudioPlayer::new(asset_server.load(path)),
                PlaybackSettings::ONCE.with_volume(Volume::Linear(start_volume)),
            ));
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
            let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume * user_settings.master_volume);
            if let Some(previous) = stage.bgm.take() {
                commands.entity(previous).try_despawn();
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
                        AudioPlayer::new(loop_audio),
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
        AudioCommand::StopBgm => {
            if let Some(previous) = stage.bgm.take() {
                commands.entity(previous).try_despawn();
            }
            shared_state.0.bgm = None;
        }
        AudioCommand::PlayVoice {
            path,
            volume,
            mode,
            animation_id,
        } => {
            let playback_volume = apply_volume_setting(volume, user_settings.voice_volume * user_settings.master_volume);
            if mode == VoicePlaybackMode::Exclusive {
                finish_active_voice(commands, animations, voice_state);
            }
            let voice = commands
                .spawn((
                    VoiceChannel {
                        path: path.clone(),
                        volume,
                    },
                    AudioPlayer::new(asset_server.load(path)),
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
