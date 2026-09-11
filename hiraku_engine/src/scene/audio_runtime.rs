use super::*;

#[derive(Component, Clone)]
pub(crate) struct BgmChannel {
    pub path: String,
    pub volume: f32,
}

/// Holds both file handles until the continuous prelude/loop source can be built.
#[derive(Component)]
pub(crate) struct BgmPrelude {
    pub prelude_audio: Handle<AudioSource>,
    pub loop_audio: Handle<AudioSource>,
    pub start_volume: f32,
}

#[derive(Component)]
pub(crate) struct AudioFade {
    pub from: f32,
    pub to: f32,
    pub timer: Timer,
    pub animation_id: Option<String>,
}

#[derive(Component)]
#[expect(dead_code, reason = "voice metadata is kept for future UI/debug hooks")]
pub(crate) struct VoiceChannel {
    pub path: String,
    pub volume: f32,
}

#[derive(Component)]
pub(crate) struct SfxChannel {
    pub volume: f32,
}

#[derive(Component)]
pub(crate) struct SfxCompletion {
    pub animation_id: Option<String>,
}

/// Playback owns its ECS entity. There are no threads/channels or blocking
/// waits here; task joins observe completion through the animation tracker.
pub fn poll_sfx_playback(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut animations: ResMut<AnimationState>,
    sounds: Query<(Entity, &AudioPlayer, Option<&AudioSink>, &SfxCompletion)>,
) {
    if !sounds.is_empty() {
        redraw.request();
    }
    for (entity, player, sink, completion) in &sounds {
        let failed = matches!(
            asset_server.get_load_state(player.0.id()),
            Some(bevy::asset::LoadState::Failed(_))
        );
        if failed || sink.is_some_and(|sink| sink.empty()) {
            if let Some(id) = &completion.animation_id {
                animations.completed.insert(id.clone());
            }
            commands.entity(entity).try_despawn();
        }
    }
}

pub fn animate_audio_fades(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    time: crate::scene::playback::StoryTime,
    settings: Res<UserSettings>,
    mut animations: ResMut<AnimationState>,
    mut sources: Query<(
        Entity,
        Option<&mut AudioSink>,
        &mut AudioFade,
        Option<&BgmChannel>,
        Option<&VoiceChannel>,
        Option<&SfxChannel>,
    )>,
) {
    if !sources.is_empty() {
        redraw.request();
    }
    for (entity, sink, mut fade, bgm, voice, sfx) in &mut sources {
        let Some(mut sink) = sink else {
            // Asset loading is asynchronous. The fade starts with audible playback, not while
            // the source is still waiting to be decoded.
            continue;
        };
        fade.timer.tick(time.delta());
        let fraction = tween_fraction(&fade.timer);
        let volume = fade.from + (fade.to - fade.from) * fraction;
        let channel = if bgm.is_some() {
            settings.bgm_volume
        } else if voice.is_some() {
            settings.voice_volume
        } else if sfx.is_some() {
            settings.sfx_volume
        } else {
            1.0
        };
        sink.set_volume(Volume::Linear(apply_volume_setting(
            volume,
            channel * settings.master_volume,
        )));

        if fade.timer.is_finished() {
            if let Some(animation_id) = fade.animation_id.take() {
                animations.completed.insert(animation_id);
            }
            commands.entity(entity).try_remove::<AudioFade>();
        }
    }
}

/// Waits until both files are cached, then starts one decoder with no ECS boundary switch.
pub fn prepare_bgm_preludes(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    file_audio: Res<Assets<AudioSource>>,
    mut prelude_loop_audio: ResMut<Assets<PreludeLoopAudio>>,
    preludes: Query<(Entity, &BgmPrelude)>,
) {
    if !preludes.is_empty() {
        redraw.request();
    }
    for (entity, prelude) in &preludes {
        let Some(prelude_source) = file_audio.get(&prelude.prelude_audio) else {
            continue;
        };
        let Some(loop_source) = file_audio.get(&prelude.loop_audio) else {
            continue;
        };

        let audio = prelude_loop_audio.add(PreludeLoopAudio::new(
            prelude_source.clone(),
            loop_source.clone(),
        ));
        commands
            .entity(entity)
            .insert((
                AudioPlayer(audio),
                PlaybackSettings::ONCE.with_volume(Volume::Linear(prelude.start_volume)),
            ))
            .try_remove::<BgmPrelude>();
    }
}

pub fn reconcile_restored_bgm(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    audio: Res<AudioCatalog>,
    user_settings: Res<UserSettings>,
    mut stage: ResMut<StageState>,
) {
    let Some(snapshot) = stage.pending_bgm_restore.take() else {
        return;
    };
    let playback_volume = apply_volume_setting(
        snapshot.volume,
        user_settings.bgm_volume * user_settings.master_volume,
    );
    let loop_audio = asset_server.load(snapshot.path.clone());
    let entity = if let Some(prelude) = audio
        .resolve_music_path(&snapshot.path)
        .and_then(|definition| definition.prelude.as_ref())
    {
        commands
            .spawn((
                BgmChannel {
                    path: snapshot.path,
                    volume: snapshot.volume,
                },
                BgmPrelude {
                    prelude_audio: asset_server.load(prelude.clone()),
                    loop_audio,
                    start_volume: playback_volume,
                },
            ))
            .id()
    } else {
        commands
            .spawn((
                BgmChannel {
                    path: snapshot.path,
                    volume: snapshot.volume,
                },
                AudioPlayer::new(loop_audio),
                PlaybackSettings::LOOP.with_volume(Volume::Linear(playback_volume)),
            ))
            .id()
    };
    stage.bgm = Some(entity);
}

pub fn apply_live_audio_settings(
    user_settings: Res<UserSettings>,
    mut bgms: Query<
        (&mut AudioSink, &BgmChannel),
        (
            Without<VoiceChannel>,
            Without<SfxChannel>,
            Without<AudioFade>,
        ),
    >,
    mut voices: Query<
        (&mut AudioSink, &VoiceChannel),
        (Without<BgmChannel>, Without<SfxChannel>, Without<AudioFade>),
    >,
    mut sfx: Query<
        (&mut AudioSink, &SfxChannel),
        (
            Without<BgmChannel>,
            Without<VoiceChannel>,
            Without<AudioFade>,
        ),
    >,
) {
    if !user_settings.is_changed() {
        return;
    }

    for (mut sink, channel) in &mut bgms {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.bgm_volume * user_settings.master_volume,
        )));
    }
    for (mut sink, channel) in &mut voices {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.voice_volume * user_settings.master_volume,
        )));
    }
    for (mut sink, channel) in &mut sfx {
        sink.set_volume(Volume::Linear(apply_volume_setting(
            channel.volume,
            user_settings.sfx_volume * user_settings.master_volume,
        )));
    }
}

pub(super) fn apply_volume_setting(volume: f32, setting: f32) -> f32 {
    (volume * setting).clamp(0.0, 1.0)
}

pub(super) fn finish_active_voice(
    commands: &mut Commands,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    let Some(active) = voice_state.active.take() else {
        return;
    };
    finish_voice(commands, animations, active);
}

pub(super) fn finish_all_voices(
    commands: &mut Commands,
    animations: &mut AnimationState,
    voice_state: &mut VoiceState,
) {
    finish_active_voice(commands, animations, voice_state);
    let concurrent = std::mem::take(&mut voice_state.concurrent);
    for active in concurrent.into_values() {
        finish_voice(commands, animations, active);
    }
}

pub(super) fn finish_voice(
    commands: &mut Commands,
    animations: &mut AnimationState,
    mut active: ActiveVoice,
) {
    commands.entity(active.entity).try_despawn();
    if let Some(animation_id) = active.animation_id.take() {
        animations.completed.insert(animation_id);
    }
}

pub fn poll_voice_playback(
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    mut animations: ResMut<AnimationState>,
    mut voice_state: ResMut<VoiceState>,
    sinks: Query<&AudioSink>,
) {
    if voice_state.active.is_some() || !voice_state.concurrent.is_empty() {
        redraw.request();
    }
    let exclusive_finished = voice_state
        .active
        .as_ref()
        .is_some_and(|active| sinks.get(active.entity).is_ok_and(|sink| sink.empty()));
    if exclusive_finished {
        finish_active_voice(&mut commands, &mut animations, &mut voice_state);
    }

    let completed = voice_state
        .concurrent
        .keys()
        .copied()
        .filter(|entity| sinks.get(*entity).is_ok_and(|sink| sink.empty()))
        .collect::<Vec<_>>();
    for entity in completed {
        if let Some(active) = voice_state.concurrent.remove(&entity) {
            finish_voice(&mut commands, &mut animations, active);
        }
    }
}
