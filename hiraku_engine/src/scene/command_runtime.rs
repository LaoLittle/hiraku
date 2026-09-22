use super::*;

mod audio_commands;
mod dialogue_commands;
mod ingress;
mod ui_commands;

use crate::script::navigation::{NavigationKind, NavigationReset};
use audio_commands::dispatch_audio_command;
use dialogue_commands::dispatch_dialogue_command;
pub use ingress::drive_story_runtime;
pub(super) use ingress::evaluate_ui_at;
pub(super) use ingress::evaluate_ui_at_with_arguments;
#[cfg(test)]
pub(crate) use ingress::resolve_ui_component_path;
use ui_commands::dispatch_ui_command;

#[derive(Debug)]
pub struct SequencedScriptCommand {
    pub sequence: u64,
    pub command: ScriptCommand,
}

#[derive(Resource, Default)]
pub struct PendingScriptCommands {
    next_sequence: u64,
    last_dispatched_sequence: Option<u64>,
    items: VecDeque<SequencedScriptCommand>,
}

impl PendingScriptCommands {
    pub fn enqueue(&mut self, command: ScriptCommand) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("script command sequence exhausted");
        self.items
            .push_back(SequencedScriptCommand { sequence, command });
        sequence
    }

    fn dispatch_next(&mut self) -> Option<SequencedScriptCommand> {
        let queued = self.items.pop_front()?;
        debug_assert!(
            self.last_dispatched_sequence
                .is_none_or(|previous| queued.sequence > previous),
            "script commands must be dispatched in sequence order"
        );
        self.last_dispatched_sequence = Some(queued.sequence);
        Some(queued)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[derive(SystemParam)]
pub struct UiCommandContext<'w, 's> {
    pub ui_fonts: Res<'w, UiFonts>,
    pub ui_style: Res<'w, UiStyle>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub dialogue_history: ResMut<'w, DialogueHistoryState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root:
        Query<'w, 's, &'static mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub line_text_entity: Query<'w, 's, Entity, (With<LineText>, Without<SpeakerText>)>,
}

#[derive(SystemParam)]
pub struct ScriptExecutionCommandContext<'w> {
    pub pending_commands: ResMut<'w, PendingScriptCommands>,
    pub runtime: ResMut<'w, ScriptRuntimeState>,
    pub animations: ResMut<'w, AnimationState>,
    pub video_player: ResMut<'w, VideoPlayer>,
    pub movie_waits: ResMut<'w, PendingMovieWaits>,
}

#[derive(SystemParam)]
pub struct RenderAssetCommandContext<'w> {
    pub canvas: Res<'w, crate::HirakuCanvas>,
    pub preview: ResMut<'w, super::save_preview::SavePreview>,
    pub images: Res<'w, Assets<Image>>,
    pub characters: Res<'w, CharacterCatalog>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub alpha_mask_materials: ResMut<'w, Assets<AlphaMaskMaterial>>,
    pub multiply_materials: ResMut<'w, Assets<MultiplyMaterial>>,
    pub world_sprite_materials: ResMut<'w, Assets<WorldSpriteMaterial>>,
}

#[derive(SystemParam)]
pub struct SceneCommandContext<'w, 's> {
    pub dependencies: ResMut<'w, crate::dependencies::ScriptDependencies>,
    pub commands: Commands<'w, 's>,
    pub app_exit: MessageWriter<'w, AppExit>,
    pub asset_server: Res<'w, AssetServer>,
    pub render_assets: RenderAssetCommandContext<'w>,
    pub vfs: Res<'w, VfsResource>,
    pub shared_state: ResMut<'w, SceneSharedState>,
    pub user_settings: ResMut<'w, UserSettings>,
    pub ui: UiCommandContext<'w, 's>,
    pub frontend: ResMut<'w, FrontendState>,
    pub stage: ResMut<'w, StageState>,
    pub execution: ScriptExecutionCommandContext<'w>,
    pub camera_state: ResMut<'w, CameraState>,
    pub camera_tweens: ResMut<'w, CameraTweenState>,
    pub camera_shake: ResMut<'w, CameraShakeState>,
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub waits: ResMut<'w, PendingWaits>,
}

pub fn process_script_commands(mut redraw: crate::redraw::Redraw, ctx: SceneCommandContext) {
    let mut dependencies = ctx.dependencies;
    let ui = ctx.ui;
    let execution = ctx.execution;
    let mut render_assets = ctx.render_assets;
    let mut commands = ctx.commands;
    let mut app_exit = ctx.app_exit;
    let asset_server = ctx.asset_server;
    let images = render_assets.images;
    let vfs = ctx.vfs;
    let mut shared_state = ctx.shared_state;
    let characters = render_assets.characters;
    let mut user_settings = ctx.user_settings;
    let ui_fonts = ui.ui_fonts;
    let ui_style = ui.ui_style;
    let mut frontend = ctx.frontend;
    let mut stage = ctx.stage;
    let mut camera_state = ctx.camera_state;
    let mut camera_tweens = ctx.camera_tweens;
    let mut camera_shake = ctx.camera_shake;
    let mut pending_script_commands = execution.pending_commands;
    let mut script_runtime = execution.runtime;
    let mut dialogue_state = ui.dialogue_state;
    let mut dialogue_history = ui.dialogue_history;
    let mut choice_state = ui.choice_state;
    let mut screen_state = ui.screen_state;
    let mut overlay_state = ui.overlay_state;
    let mut animations = execution.animations;
    let mut video_player = execution.video_player;
    let mut movie_waits = execution.movie_waits;
    let mut voice_state = ctx.voice_state;
    let mut pending_characters = ctx.pending_characters;
    let mut waits = ctx.waits;
    let mut meshes = render_assets.meshes;
    let mut alpha_mask_materials = render_assets.alpha_mask_materials;
    let mut multiply_materials = render_assets.multiply_materials;
    let mut world_sprite_materials = render_assets.world_sprite_materials;
    let choice_ui_roots = ui.choice_ui_roots;
    let mut dialogue_root = ui.dialogue_root;
    let mut speaker_text = ui.speaker_text;
    let mut line_text = ui.line_text;
    let line_text_entity = ui.line_text_entity;

    while crate::storage::storage_ready() {
        // Inspect without consuming the command: its sequence and the caller
        // remain intact throughout asynchronous preload.
        if let Some(SequencedScriptCommand {
            command: ScriptCommand::Runtime(RuntimeCommand::Navigate(navigation)),
            ..
        }) = pending_script_commands.items.front()
            && navigation.should_preload(script_runtime.story.as_ref())
        {
            let target = vfs.0.resolve_path(
                navigation
                    .origin
                    .as_deref()
                    .or(script_runtime.current_script.as_deref()),
                &navigation.path,
            );
            let mut required = vec![target];
            if navigation.kind == NavigationKind::Call {
                required.extend(script_runtime.current_script.iter().cloned());
                required.extend(script_runtime.call_stack.iter().map(|f| f.script.clone()));
            }
            match dependencies.prepare(&vfs.0, &asset_server, &required) {
                Ok(false) => break,
                Err(error) => {
                    crate::script::emit_script_diagnostic("failed to preload script:", &error);
                    pending_script_commands.clear();
                    dependencies.loading = true;
                    dependencies.error = Some(error);
                    break;
                }
                Ok(true) => (),
            }
        }
        let Some(queued) = pending_script_commands.dispatch_next() else {
            break;
        };
        redraw.request();
        let command = queued.command;
        if screen_state.active_root.is_some()
            && screen_state.waiting.is_none()
            && should_clear_stale_screen_before_command(&command)
        {
            clear_screen_ui(&mut commands, &mut screen_state);
        }

        match command {
            ScriptCommand::Runtime(RuntimeCommand::SaveSlot(slot)) => {
                if let Err(error) = save_runtime_slot(
                    &slot,
                    &script_runtime,
                    &shared_state,
                    &[],
                    &dialogue_history.entries,
                ) {
                    warn!("failed to save slot `{slot}`: {error}");
                }
            }
            ScriptCommand::Stage(StageCommand::Clip(clip)) => {
                if let Err(error) = shared_state.0.clips.apply(clip) {
                    warn!("{error}");
                }
            }
            ScriptCommand::Stage(StageCommand::SetActorDepth { id, depth }) => {
                shared_state.0.actor_depths.insert(id, depth);
            }
            ScriptCommand::Stage(StageCommand::Spatial(mut command)) => {
                if let crate::stage::runtime::StageCommand::Open { path, .. } = &mut command {
                    *path = vfs
                        .0
                        .resolve_path(script_runtime.current_script.as_deref(), path);
                }
                if let Err(error) = crate::stage::runtime::StageSnapshot::apply(
                    &mut shared_state.0.spatial_stage,
                    command,
                ) {
                    crate::script::emit_script_diagnostic("stage command failed", &error);
                    script_runtime.story = None;
                    return;
                }
            }
            ScriptCommand::Stage(StageCommand::Picture(picture)) => {
                if let Err(error) =
                    pictures::apply_picture_command(&mut shared_state.0.pictures, picture)
                {
                    warn!("{error}");
                }
            }
            ScriptCommand::Runtime(RuntimeCommand::Log(message)) => info!("[hks] {message}"),
            ScriptCommand::Stage(StageCommand::SetBackground {
                path,
                fade,
                animation_id,
            }) => {
                let current_background = shared_state
                    .0
                    .background
                    .as_ref()
                    .map(|background| background.path.clone());
                if fade.is_none() && current_background.as_deref() == Some(path.as_str()) {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                }

                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }
                let image = crate::texture::load_static_image(&asset_server, path.clone());
                let mut sprite = WorldSprite::from_image(image);
                let background = if let Some(duration) = fade {
                    sprite.color = sprite.color.with_alpha(0.0);
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                            VisualTween {
                                from_alpha: Some(0.0),
                                to_alpha: Some(1.0),
                                from_translation: None,
                                to_translation: None,
                                from_scale: None,
                                to_scale: None,
                                timer: Timer::new(duration, TimerMode::Once),
                                animation_id,
                                despawn_on_finish: false,
                            },
                        ))
                        .id()
                } else {
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            BackgroundLayer { path: path.clone() },
                            sprite,
                            render,
                            Transform::from_xyz(0.0, 0.0, STAGE_Z_BACKGROUND),
                        ))
                        .id();
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    entity
                };

                if let Some(previous) = stage.background.replace(background) {
                    if let Some(duration) = fade {
                        commands.entity(previous).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id: None,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(previous).try_despawn();
                    }
                }

                shared_state.0.background = Some(ImageLayerSnapshot { path });
            }
            ScriptCommand::Dialogue(command) => dispatch_dialogue_command(
                command,
                &mut commands,
                &mut dialogue_state,
                &mut dialogue_history,
                &mut shared_state,
                &mut animations,
                &mut dialogue_root,
                &mut speaker_text,
                &mut line_text,
                &line_text_entity,
                &ui_fonts,
                &ui_style,
            ),
            ScriptCommand::Camera(CameraCommand::Shake {
                amplitude,
                interval,
                duration,
                animation_id,
            }) => {
                if let Some(previous) = camera_shake.active.take() {
                    complete_missing_animation(&mut animations, previous.animation_id);
                }
                if duration.is_zero() {
                    complete_missing_animation(&mut animations, animation_id);
                } else {
                    camera_shake.active = Some(crate::render::camera::CameraShake {
                        amplitude,
                        interval,
                        timer: Timer::new(duration, TimerMode::Once),
                        seed: queued.sequence,
                        animation_id,
                    });
                }
            }
            ScriptCommand::Camera(CameraCommand::Set {
                blur_intensity,
                zoom,
                zoom_view_space,
                offset,
                rotation,
                projection,
                scope,
                duration,
                ease,
                animation_id,
            }) => {
                if let Some(blur) = blur_intensity {
                    shared_state.0.camera.blur = blur;
                }
                if let Some(zoom) = zoom {
                    shared_state.0.camera.zoom = zoom;
                }
                if let Some(offset) = offset {
                    shared_state.0.camera.offset = offset.to_array();
                }
                if let Some(rotation) = rotation {
                    shared_state.0.camera.rotation = rotation.to_array();
                }
                if let Some(projection) = projection {
                    shared_state.0.camera.projection = match projection {
                        crate::script::CameraProjectionMode::Orthographic => "orthographic",
                        crate::script::CameraProjectionMode::Perspective => "perspective",
                    }
                    .to_string();
                }
                shared_state.0.camera.scope = match scope {
                    crate::script::CameraEffectScope::World => "world",
                    crate::script::CameraEffectScope::Canvas => "canvas",
                }
                .to_string();
                start_camera_tween(
                    &mut camera_state,
                    &mut camera_tweens,
                    blur_intensity,
                    zoom,
                    zoom_view_space,
                    offset,
                    rotation,
                    projection,
                    scope,
                    duration,
                    ease,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::Settings(command) => {
                match &command {
                    SettingsCommand::Preference(change) => {
                        if let Err(error) = user_settings.apply(change) {
                            warn!("invalid preference: {error}");
                        } else if let Err(error) = write_user_settings(&user_settings) {
                            warn!("failed to write preferences: {error}");
                        }
                        continue;
                    }
                    SettingsCommand::AutoDialogue(enabled) => {
                        if *enabled {
                            dialogue_state.fast_forward_enabled = false;
                            dialogue_state.fast_forward_held = false;
                        }
                        dialogue_state.auto_enabled = *enabled;
                        dialogue_state.auto_elapsed = 0.0;
                        continue;
                    }
                    SettingsCommand::FastForward(enabled) => {
                        dialogue_state.fast_forward_enabled = *enabled;
                        dialogue_state.fast_forward_elapsed = 0.0;
                        if *enabled {
                            dialogue_state.auto_enabled = false;
                        }
                        continue;
                    }
                    _ => {}
                }
                let name = match &command {
                    SettingsCommand::Adjust { name, .. } | SettingsCommand::Set { name, .. } => {
                        name
                    }
                    _ => unreachable!("non-volume settings handled above"),
                };
                let volume = match name.as_str() {
                    "bgmVolume" => &mut user_settings.bgm_volume,
                    "voiceVolume" => &mut user_settings.voice_volume,
                    "sfxVolume" => &mut user_settings.sfx_volume,
                    _ => {
                        warn!("unsupported user setting `{name}`");
                        continue;
                    }
                };
                *volume = match command {
                    SettingsCommand::Adjust { delta, .. } => adjusted_volume(*volume, delta),
                    SettingsCommand::Set { value, .. } => value.clamp(0.0, 1.0),
                    _ => unreachable!("non-volume settings handled above"),
                };
                if let Err(error) = write_user_settings(user_settings.as_ref()) {
                    warn!("failed to write user settings: {error}");
                }
            }
            ScriptCommand::Ui(command) => dispatch_ui_command(
                command,
                &mut commands,
                &asset_server,
                &images,
                &ui_fonts,
                &ui_style,
                &mut screen_state,
                &mut overlay_state,
                &render_assets.canvas,
                &mut render_assets.preview,
            ),
            ScriptCommand::Character(CharacterCommand::StopMotion { actor_id }) => {
                if let Some(motion) = shared_state.0.actor_motions.get_mut(&actor_id) {
                    motion.stop();
                    complete_missing_animation(&mut animations, motion.animation_id.take());
                }
            }
            ScriptCommand::Character(CharacterCommand::Motion {
                actor_id,
                revision,
                transition,
                animation_id,
            }) => {
                let restoring = stage
                    .pending_character_restore
                    .iter()
                    .any(|part| part.id.starts_with(&format!("character::{actor_id}::")));
                let hidden = !stage.character_active_parts.contains_key(&actor_id) && !restoring;
                actor_motion::start(
                    &mut shared_state.0.actor_motions,
                    &mut animations,
                    actor_id.clone(),
                    revision,
                    transition,
                    animation_id,
                );
                if hidden && let Some(motion) = shared_state.0.actor_motions.get_mut(&actor_id) {
                    actor_motion::finish(motion, &mut animations);
                }
            }
            ScriptCommand::Character(CharacterCommand::Hide { actor_id, fade_ms }) => {
                for (id, motion) in &mut shared_state.0.actor_motions {
                    if actor_id.as_ref().is_none_or(|actor| actor == id) {
                        actor_motion::finish(motion, &mut animations);
                    }
                }
                hide_character_entities(
                    &mut commands,
                    &mut stage,
                    &mut pending_characters,
                    &mut animations,
                    actor_id.as_deref(),
                    fade_ms,
                );
            }
            ScriptCommand::Character(CharacterCommand::Show {
                rotation,
                placement_animation,
                actor_id,
                character_name,
                expressions,
                position,
                scale,
                focused,
                fade,
                animation_id,
            }) => {
                let Some(character) = characters.characters.get(&character_name).cloned() else {
                    warn!("character `{character_name}` not found in catalog");
                    complete_missing_animation(&mut animations, animation_id);
                    continue;
                };
                let parts = match character.parts_for_expressions(&expressions) {
                    Ok(parts) => parts,
                    Err(message) => {
                        warn!("{message}");
                        complete_missing_animation(&mut animations, animation_id);
                        continue;
                    }
                };

                stage.character_positions.insert(actor_id.clone(), position);
                stage.character_rotations.insert(actor_id.clone(), rotation);
                stage
                    .character_catalog_names
                    .insert(actor_id.clone(), character_name);
                queue_character_show(
                    &mut commands,
                    &asset_server,
                    &mut meshes,
                    &mut alpha_mask_materials,
                    &mut multiply_materials,
                    &mut stage,
                    &mut pending_characters,
                    &mut animations,
                    actor_id,
                    parts,
                    position,
                    scale,
                    focused,
                    fade,
                    animation_id,
                    placement_animation,
                );
            }
            ScriptCommand::Stage(StageCommand::SetCurtain {
                color,
                opacity,
                fade,
                mask,
                softness,
            }) => {
                if let Some(overlay) = stage.overlay {
                    let mask = mask.map(|path| crate::render::world_sprite::DissolveMask {
                        image: asset_server
                            .load_builder()
                            .with_settings(|settings: &mut bevy::image::ImageLoaderSettings| {
                                settings.is_srgb = false;
                                settings.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                            })
                            .load(path.clone()),
                        path,
                        softness,
                        canvas_size: Vec2::ONE,
                        reversed: false,
                    });
                    commands
                        .entity(overlay)
                        .remove::<(VisualTween, super::curtain::CurtainFailed)>()
                        .try_insert(super::curtain::PendingCurtain {
                            color,
                            opacity,
                            duration: fade,
                            mask,
                        });
                }
            }
            ScriptCommand::Stage(StageCommand::AwaitCurtain { done }) => {
                commands.spawn(super::curtain::CurtainWait(done));
            }
            ScriptCommand::Animation(AnimationCommand::Delay { duration, done }) => {
                waits.items.push(super::animation_runtime::PendingWait {
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id: None,
                    done,
                });
            }
            ScriptCommand::Animation(AnimationCommand::Scene { effect, done }) => {
                commands.spawn(super::effect_wait::SceneEffectWait { effect, done });
            }
            ScriptCommand::Animation(AnimationCommand::Wait { ids, done }) => {
                if ids.iter().all(|id| animations.completed.contains(id)) {
                    commands.write_message(ScriptResponseMessage {
                        request: done,
                        response: ScriptResponse::Continue,
                    });
                } else {
                    animations.waits.push(PendingAnimationWait { ids, done });
                }
            }
            ScriptCommand::Audio(command) => dispatch_audio_command(
                command,
                &mut commands,
                &asset_server,
                &user_settings,
                &mut stage,
                &mut shared_state,
                &mut animations,
                &mut voice_state,
            ),
            ScriptCommand::Video(command) => {
                dispatch_video_command(command, &asset_server, &mut video_player, &mut movie_waits)
            }
            ScriptCommand::Runtime(RuntimeCommand::Exit) => {
                app_exit.write(AppExit::Success);
            }
            ScriptCommand::Runtime(RuntimeCommand::Navigate(navigation)) => {
                let target = vfs.0.resolve_path(
                    navigation
                        .origin
                        .as_deref()
                        .or(script_runtime.current_script.as_deref()),
                    &navigation.path,
                );
                let cached = script_runtime
                    .story
                    .as_ref()
                    .and_then(|story| story.program_for_path(&target));
                let prepared = cached
                    .map(Ok)
                    .unwrap_or_else(|| {
                        vfs.0
                            .read_text(&target)
                            .map_err(|error| error.to_string())
                            .and_then(|source| {
                                crate::script::compile_story_program(&vfs.0, &target, &source)
                            })
                    })
                    .and_then(|bytecode| {
                        StoryRuntime::new(bytecode).map_err(|error| error.to_string())
                    });
                let mut next_story = match prepared {
                    Ok(story) => story,
                    Err(error) => {
                        crate::script::emit_script_diagnostic(
                            &format!("failed to navigate to HKS script `{target}`:"),
                            &error,
                        );
                        continue;
                    }
                };
                next_story.preload_calls = navigation.should_preload(script_runtime.story.as_ref());

                let mut globals = if navigation.reset == NavigationReset::Session {
                    BTreeMap::new()
                } else {
                    script_runtime
                        .story
                        .as_ref()
                        .map(|story| story.globals().clone())
                        .unwrap_or_default()
                };
                globals.extend(crate::script::capabilities::engine_globals(&user_settings));
                next_story.set_globals(globals);
                if navigation.reset != NavigationReset::Session
                    && let Some(previous) = script_runtime.story.as_ref()
                {
                    next_story.inherit_native_state(
                        previous,
                        navigation.reset == NavigationReset::Presentation,
                    );
                }

                if navigation.kind == NavigationKind::Goto {
                    clear_choice_ui(&mut commands, &choice_ui_roots);
                    clear_screen_ui(&mut commands, &mut screen_state);
                    choice_state.options.clear();
                    choice_state.waiting.take();
                    dialogue_state.waiting.take();
                    screen_state.waiting.take();
                    waits.items.clear();
                    animations.waits.clear();
                    pending_script_commands.clear();
                    script_runtime.pending_ui_screen = None;
                    script_runtime.pending_ui_arguments.clear();
                    script_runtime.wait_request = None;
                    script_runtime.response_inbox.clear();
                    script_runtime.task_requests.clear();
                }

                if navigation.reset != NavigationReset::None {
                    finish_all_voices(&mut commands, &mut animations, &mut voice_state);
                    clear_overlay_ui(&mut commands, &mut overlay_state);
                    script_runtime.mounted_ui_overlays.clear();
                    pending_characters.items.clear();
                    animations.completed.clear();
                    camera_tweens.active = None;
                    camera_shake.active = None;
                    *camera_state = CameraState::default();
                    let empty_scene = SceneSnapshot::default();
                    shared_state.0 = empty_scene.clone();
                    restore_scene_snapshot(
                        &mut commands,
                        &asset_server,
                        &mut stage,
                        &mut dialogue_state,
                        &mut choice_state,
                        &mut dialogue_root,
                        &mut speaker_text,
                        &mut line_text,
                        &user_settings,
                        empty_scene,
                    );
                    if navigation.reset == NavigationReset::Session {
                        dialogue_history.entries.clear();
                    }
                }

                if navigation.kind == NavigationKind::Call {
                    if let (Some(script), Some(caller)) = (
                        script_runtime.current_script.take(),
                        script_runtime.story.take(),
                    ) {
                        script_runtime
                            .call_stack
                            .push(crate::script::ScriptCallFrame {
                                script,
                                story: caller,
                            });
                    }
                } else {
                    script_runtime.call_stack.clear();
                }
                script_runtime.story = Some(next_story);
                script_runtime.current_script = Some(target);
                script_runtime.task_requests.clear();
                frontend.runtime_started = true;
                frontend.notice = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_sequences_remain_monotonic_across_queue_clears() {
        let mut commands = PendingScriptCommands::default();

        assert_eq!(
            commands.enqueue(ScriptCommand::Runtime(RuntimeCommand::Log("first".into()))),
            0
        );
        assert_eq!(
            commands.enqueue(ScriptCommand::Runtime(RuntimeCommand::Log("second".into()))),
            1
        );
        let first = commands
            .dispatch_next()
            .expect("the first queued command must be available");
        assert_eq!(first.sequence, 0);

        commands.clear();
        assert_eq!(
            commands.enqueue(ScriptCommand::Runtime(RuntimeCommand::Log("third".into()))),
            2
        );
        let third = commands
            .dispatch_next()
            .expect("the post-clear command must be available");
        assert_eq!(third.sequence, 2);
        assert!(matches!(
            third.command,
            ScriptCommand::Runtime(RuntimeCommand::Log(message)) if message == "third"
        ));
    }

    #[test]
    fn domain_commands_share_one_deterministic_order() {
        let mut commands = PendingScriptCommands::default();
        commands.enqueue(ScriptCommand::Audio(AudioCommand::StopBgm {
            fade: std::time::Duration::ZERO,
            animation_id: None,
        }));
        commands.enqueue(ScriptCommand::Dialogue(DialogueCommand::Clear));
        commands.enqueue(ScriptCommand::Runtime(RuntimeCommand::Exit));

        let audio = commands
            .dispatch_next()
            .expect("the audio command must remain first");
        let dialogue = commands
            .dispatch_next()
            .expect("the dialogue command must remain second");
        let runtime = commands
            .dispatch_next()
            .expect("the runtime command must remain third");

        assert_eq!(
            (audio.sequence, dialogue.sequence, runtime.sequence),
            (0, 1, 2)
        );
        assert!(matches!(
            audio.command,
            ScriptCommand::Audio(AudioCommand::StopBgm { .. })
        ));
        assert!(matches!(
            dialogue.command,
            ScriptCommand::Dialogue(DialogueCommand::Clear)
        ));
        assert!(matches!(
            runtime.command,
            ScriptCommand::Runtime(RuntimeCommand::Exit)
        ));
    }
}
