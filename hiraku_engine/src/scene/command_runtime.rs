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
}

#[derive(SystemParam)]
pub struct RenderAssetCommandContext<'w> {
    pub images: Res<'w, Assets<Image>>,
    pub texture_atlases: Res<'w, TextureAtlasCatalog>,
    pub characters: Res<'w, CharacterCatalog>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub alpha_mask_materials: ResMut<'w, Assets<AlphaMaskMaterial>>,
    pub multiply_materials: ResMut<'w, Assets<MultiplyMaterial>>,
    pub world_sprite_materials: ResMut<'w, Assets<WorldSpriteMaterial>>,
}

#[derive(SystemParam)]
pub struct SceneCommandContext<'w, 's> {
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
    pub voice_state: ResMut<'w, VoiceState>,
    pub pending_characters: ResMut<'w, PendingCharacterShows>,
    pub waits: ResMut<'w, PendingWaits>,
}

pub fn process_script_commands(ctx: SceneCommandContext) {
    let ui = ctx.ui;
    let execution = ctx.execution;
    let render_assets = ctx.render_assets;
    let mut commands = ctx.commands;
    let mut app_exit = ctx.app_exit;
    let asset_server = ctx.asset_server;
    let images = render_assets.images;
    let texture_atlases = render_assets.texture_atlases;
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
    let mut pending_script_commands = execution.pending_commands;
    let mut script_runtime = execution.runtime;
    let mut dialogue_state = ui.dialogue_state;
    let mut dialogue_history = ui.dialogue_history;
    let mut choice_state = ui.choice_state;
    let mut screen_state = ui.screen_state;
    let mut overlay_state = ui.overlay_state;
    let mut animations = execution.animations;
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

    while let Some(queued) = pending_script_commands.dispatch_next() {
        let command = queued.command;
        if screen_state.active_root.is_some()
            && screen_state.waiting.is_none()
            && should_clear_stale_screen_before_command(&command)
        {
            clear_screen_ui(&mut commands, &mut screen_state);
        }

        match command {
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
                let image = asset_server.load(path.clone());
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
            ScriptCommand::Camera(CameraCommand::Set {
                blur_intensity,
                zoom,
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
            ScriptCommand::Settings(SettingsCommand::Adjust { name, delta }) => {
                let volume = match name.as_str() {
                    "bgmVolume" => &mut user_settings.bgm_volume,
                    "voiceVolume" => &mut user_settings.voice_volume,
                    "sfxVolume" => &mut user_settings.sfx_volume,
                    _ => {
                        warn!("unsupported user setting `{name}`");
                        continue;
                    }
                };
                *volume = adjusted_volume(*volume, delta);
                if let Err(error) = write_user_settings(user_settings.as_ref()) {
                    warn!("failed to write user settings: {error}");
                }
            }
            ScriptCommand::Ui(command) => dispatch_ui_command(
                command,
                &mut commands,
                &asset_server,
                &images,
                &texture_atlases,
                &ui_fonts,
                &ui_style,
                &mut screen_state,
                &mut overlay_state,
            ),
            ScriptCommand::Character(CharacterCommand::Show {
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
                queue_character_show(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
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
                );
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
                let prepared = vfs
                    .0
                    .read_text(&target)
                    .map_err(|error| error.to_string())
                    .and_then(|source| compile_story_bytecode(&target, &source))
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
                        dialogue_history.visible = false;
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
        commands.enqueue(ScriptCommand::Audio(AudioCommand::StopBgm));
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
            ScriptCommand::Audio(AudioCommand::StopBgm)
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
