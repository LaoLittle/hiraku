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
        AudioCommand::PlayBgm {
            path,
            prelude,
            volume,
            fade_in,
            animation_id,
        } => {
            let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
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
                commands.entity(bgm).insert(BgmFade {
                    from: start_volume,
                    to: playback_volume,
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
            let playback_volume = apply_volume_setting(volume, user_settings.voice_volume);
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
