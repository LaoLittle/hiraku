use super::*;

mod ingress;

pub use ingress::bridge_story_events;
pub(super) use ingress::evaluate_ui_at;
#[cfg(test)]
pub(crate) use ingress::resolve_ui_component_path;

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
    pub ui_style: ResMut<'w, UiStyle>,
    pub dialogue_state: ResMut<'w, DialogueState>,
    pub dialogue_history: ResMut<'w, DialogueHistoryState>,
    pub choice_state: ResMut<'w, ChoiceState>,
    pub screen_state: ResMut<'w, ScreenUiState>,
    pub overlay_state: ResMut<'w, OverlayUiState>,
    pub choice_ui_roots: Query<'w, 's, Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    pub dialogue_root:
        Query<'w, 's, &'static mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    pub dialogue_root_node: Query<'w, 's, &'static mut Node, With<DialogueRoot>>,
    pub dialogue_background: Query<'w, 's, &'static mut BackgroundColor, With<DialogueRoot>>,
    pub dialogue_border: Query<'w, 's, &'static mut BorderColor, With<DialogueRoot>>,
    pub speaker_text: Query<'w, 's, &'static mut Text, (With<SpeakerText>, Without<LineText>)>,
    pub line_text: Query<'w, 's, &'static mut Text, (With<LineText>, Without<SpeakerText>)>,
    pub line_text_entity: Query<'w, 's, Entity, (With<LineText>, Without<SpeakerText>)>,
    pub speaker_font: Query<'w, 's, &'static mut TextFont, (With<SpeakerText>, Without<LineText>)>,
    pub line_font: Query<'w, 's, &'static mut TextFont, (With<LineText>, Without<SpeakerText>)>,
    pub hint_font: Query<
        'w,
        's,
        &'static mut TextFont,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
    pub hint_visibility:
        Query<'w, 's, &'static mut Visibility, (With<HintText>, Without<DialogueRoot>)>,
    pub speaker_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<SpeakerText>, Without<LineText>, Without<HintText>),
    >,
    pub line_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<LineText>, Without<SpeakerText>, Without<HintText>),
    >,
    pub hint_color: Query<
        'w,
        's,
        &'static mut TextColor,
        (With<HintText>, Without<SpeakerText>, Without<LineText>),
    >,
}

#[derive(SystemParam)]
pub struct ScriptExecutionCommandContext<'w> {
    pub waits: ResMut<'w, PendingWaits>,
    pub pending_cancels: ResMut<'w, PendingAnimationCancels>,
    pub pending_commands: ResMut<'w, PendingScriptCommands>,
    pub runtime: ResMut<'w, ScriptRuntimeState>,
    pub active_batches: ResMut<'w, ActiveScriptBatches>,
    pub animations: ResMut<'w, AnimationState>,
}

#[derive(SystemParam)]
pub struct RenderAssetCommandContext<'w> {
    pub images: Res<'w, Assets<Image>>,
    pub texture_atlases: Res<'w, TextureAtlasCatalog>,
    pub characters: Res<'w, CharacterCatalog>,
    pub transition_mesh: Res<'w, RuleTransitionMesh>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub alpha_mask_materials: ResMut<'w, Assets<AlphaMaskMaterial>>,
    pub multiply_materials: ResMut<'w, Assets<MultiplyMaterial>>,
    pub world_sprite_materials: ResMut<'w, Assets<WorldSpriteMaterial>>,
    pub custom_effect_materials: ResMut<'w, Assets<CustomScreenEffectMaterial>>,
    pub rule_materials: ResMut<'w, Assets<RuleTransitionMaterial>>,
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
    let mut ui_style = ui.ui_style;
    let mut frontend = ctx.frontend;
    let mut stage = ctx.stage;
    let mut waits = execution.waits;
    let mut pending_cancels = execution.pending_cancels;
    let mut camera_state = ctx.camera_state;
    let mut camera_tweens = ctx.camera_tweens;
    let mut pending_script_commands = execution.pending_commands;
    let mut script_runtime = execution.runtime;
    let mut active_batches = execution.active_batches;
    let mut dialogue_state = ui.dialogue_state;
    let mut dialogue_history = ui.dialogue_history;
    let mut choice_state = ui.choice_state;
    let mut screen_state = ui.screen_state;
    let mut overlay_state = ui.overlay_state;
    let mut animations = execution.animations;
    let mut voice_state = ctx.voice_state;
    let mut pending_characters = ctx.pending_characters;
    let transition_mesh = render_assets.transition_mesh;
    let mut meshes = render_assets.meshes;
    let mut alpha_mask_materials = render_assets.alpha_mask_materials;
    let mut multiply_materials = render_assets.multiply_materials;
    let mut world_sprite_materials = render_assets.world_sprite_materials;
    let mut custom_effect_materials = render_assets.custom_effect_materials;
    let mut rule_materials = render_assets.rule_materials;
    let choice_ui_roots = ui.choice_ui_roots;
    let mut dialogue_root = ui.dialogue_root;
    let mut dialogue_root_node = ui.dialogue_root_node;
    let mut dialogue_background = ui.dialogue_background;
    let mut dialogue_border = ui.dialogue_border;
    let mut speaker_text = ui.speaker_text;
    let mut line_text = ui.line_text;
    let line_text_entity = ui.line_text_entity;
    let mut speaker_font = ui.speaker_font;
    let mut line_font = ui.line_font;
    let mut hint_font = ui.hint_font;
    let mut hint_visibility = ui.hint_visibility;
    let mut speaker_color = ui.speaker_color;
    let mut line_color = ui.line_color;
    let mut hint_color = ui.hint_color;

    while let Some(queued) = pending_script_commands.dispatch_next() {
        let command = queued.command;
        if screen_state.active_root.is_some()
            && screen_state.waiting.is_none()
            && should_clear_stale_screen_before_command(&command)
        {
            clear_screen_ui(&mut commands, &mut screen_state);
        }

        match command {
            ScriptCommand::Log(message) => info!("[hks] {message}"),
            ScriptCommand::SetBackground {
                path,
                fade,
                animation_id,
            } => {
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
            ScriptCommand::RuleTransitionBg {
                path,
                rule_path,
                duration,
                vague,
                animation_id,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }
                if let Some(transition) = stage.transition.take() {
                    commands.entity(transition).try_despawn();
                }

                let Some(previous_background) = stage.background else {
                    let image = asset_server.load(path.clone());
                    let sprite = WorldSprite::from_image(image);
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
                    stage.background = Some(entity);
                    shared_state.0.background = Some(ImageLayerSnapshot { path });
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let Some(previous_path) = shared_state
                    .0
                    .background
                    .as_ref()
                    .map(|background| background.path.clone())
                else {
                    let image = asset_server.load(path.clone());
                    let sprite = WorldSprite::from_image(image);
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
                    stage.background = Some(entity);
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                    continue;
                };

                let target_image = asset_server.load(path.clone());
                let previous_image = asset_server.load(previous_path);
                let rule_image = asset_server.load(rule_path);
                let material = rule_materials.add(RuleTransitionMaterial {
                    from_texture: previous_image,
                    to_texture: target_image.clone(),
                    rule_texture: rule_image,
                    progress: 0.0,
                    vague,
                });
                let transition_entity = commands
                    .spawn((
                        Mesh3d(transition_mesh.0.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_BACKGROUND + 1.0),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        RuleTransitionPlayer {
                            material,
                            target_path: path,
                            target_image,
                            previous_background,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                        },
                    ))
                    .id();
                stage.transition = Some(transition_entity);
            }
            ScriptCommand::PlayCustomEffect {
                options,
                animation_id,
            } => {
                if let Some(effect) = stage.screen_effect.take() {
                    commands.entity(effect).try_despawn();
                }

                let source_image = asset_server.load(options.from_path.clone());
                let target_image = asset_server.load(options.to_path.clone());
                let rule_image = asset_server.load(options.rule_path.clone());
                let aux0_image = asset_server.load(options.aux0_path.clone());
                let aux1_image = asset_server.load(options.aux1_path.clone());

                let previous_background = if options.commit_to_bg {
                    stage.background.take()
                } else {
                    stage.background
                };

                let material = custom_effect_materials.add(CustomScreenEffectMaterial {
                    source_texture: source_image,
                    target_texture: target_image.clone(),
                    rule_texture: rule_image,
                    aux0_texture: aux0_image,
                    aux1_texture: aux1_image,
                    progress: 0.0,
                    duration: options.duration.as_secs_f32(),
                    time: 0.0,
                    mode: options.mode,
                    p0: options.p0,
                    p1: options.p1,
                    p2: options.p2,
                    p3: options.p3,
                });

                let effect_entity = commands
                    .spawn((
                        Mesh3d(transition_mesh.0.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, STAGE_Z_OVERLAY - 0.5),
                            scale: Vec3::new(6000.0, 6000.0, 1.0),
                            ..default()
                        },
                        CustomScreenEffectPlayer {
                            material,
                            timer: Timer::new(options.duration, TimerMode::Once),
                            target_path: options.commit_to_bg.then_some(options.to_path),
                            target_image: options.commit_to_bg.then_some(target_image),
                            previous_background,
                            animation_id,
                        },
                    ))
                    .id();
                stage.screen_effect = Some(effect_entity);
            }
            ScriptCommand::ShowSprite {
                id,
                path,
                rect,
                position,
                layer,
                scale,
                fade,
                animation_id,
            } => {
                let handle = asset_server.load(path.clone());
                let entity = if let Some(entity) = stage.sprites.get(&id).copied() {
                    let mut sprite = WorldSprite::from_image(handle)
                        .with_rect(rect.map(source_rect_from_corners));
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    commands.entity(entity).insert((
                        SpriteActor {
                            id: id.clone(),
                            path: path.clone(),
                        },
                        sprite,
                        Transform {
                            translation: Vec3::new(position.x, position.y, STAGE_Z_SPRITE + layer),
                            scale: Vec3::splat(scale),
                            ..default()
                        },
                    ));
                    entity
                } else {
                    let mut sprite = WorldSprite::from_image(handle)
                        .with_rect(rect.map(source_rect_from_corners));
                    if fade.is_some() {
                        sprite.color = sprite.color.with_alpha(0.0);
                    }
                    let render = world_sprite_render_components(
                        &sprite,
                        &mut meshes,
                        &mut world_sprite_materials,
                    );
                    let entity = commands
                        .spawn((
                            SpriteActor {
                                id: id.clone(),
                                path: path.clone(),
                            },
                            sprite,
                            render,
                            Transform {
                                translation: Vec3::new(
                                    position.x,
                                    position.y,
                                    STAGE_Z_SPRITE + layer,
                                ),
                                scale: Vec3::splat(scale),
                                ..default()
                            },
                        ))
                        .id();
                    stage.sprites.insert(id.clone(), entity);
                    entity
                };

                if let Some(duration) = fade {
                    commands.entity(entity).insert(VisualTween {
                        from_alpha: Some(0.0),
                        to_alpha: Some(1.0),
                        from_translation: None,
                        to_translation: None,
                        from_scale: None,
                        to_scale: None,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                        despawn_on_finish: false,
                    });
                } else {
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::HideSprite {
                id,
                fade,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.remove(&id) {
                    if let Some(duration) = fade {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(1.0),
                            to_alpha: Some(0.0),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: true,
                        });
                    } else {
                        commands.entity(entity).try_despawn();
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::SetOverlay {
                alpha,
                fade,
                animation_id,
            } => {
                if let Some(overlay) = stage.overlay {
                    if let Some(duration) = fade {
                        let current_alpha = shared_state.0.overlay_alpha;
                        commands.entity(overlay).insert(VisualTween {
                            from_alpha: Some(current_alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        commands.entity(overlay).insert(WorldSprite::from_color(
                            Color::BLACK.with_alpha(alpha),
                            Vec2::new(6000.0, 6000.0),
                        ));
                        if let Some(animation_id) = animation_id {
                            animations.completed.insert(animation_id);
                        }
                    }
                    shared_state.0.overlay_alpha = alpha;
                }
            }
            ScriptCommand::Say {
                speaker,
                text,
                animation_id,
            } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_fonts,
                        &ui_style,
                        &text,
                        0,
                        None,
                    );
                } else {
                    set_dialogue_model_reveal(&mut dialogue_state, &text, 0, None);
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id,
                    request: None,
                });
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
                if let Some(dialogue) = shared_state.0.dialogue.clone() {
                    dialogue_history.entries.push(dialogue);
                }
            }
            ScriptCommand::ContinueDialogue { text, animation_id } => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                if shared_state.0.dialogue.is_none() {
                    warn!("dialogue continuation has no UI buffer; treating it as narration");
                    if let Ok(mut visibility) = dialogue_root.single_mut() {
                        *visibility = Visibility::Visible;
                    }
                    if let Ok(mut speaker_node) = speaker_text.single_mut() {
                        **speaker_node = String::new();
                    }
                    if let Ok(line_root) = line_text_entity.single() {
                        set_dialogue_line_text(
                            &mut commands,
                            &mut dialogue_state,
                            line_root,
                            &mut line_text,
                            &ui_fonts,
                            &ui_style,
                            &text,
                            0,
                            None,
                        );
                    } else {
                        set_dialogue_model_reveal(&mut dialogue_state, &text, 0, None);
                    }
                    shared_state.0.dialogue = Some(DialogueSnapshot {
                        speaker: String::new(),
                        text,
                    });
                } else {
                    let previous_chars = shared_state
                        .0
                        .dialogue
                        .as_ref()
                        .map(|dialogue| dialogue.text.chars().count())
                        .unwrap_or_default();
                    if let Ok(line_root) = line_text_entity.single() {
                        append_dialogue_line_text(
                            &mut commands,
                            &mut dialogue_state,
                            line_root,
                            &ui_fonts,
                            &ui_style,
                            &text,
                            None,
                        );
                    } else {
                        append_dialogue_model_reveal(
                            &mut dialogue_state,
                            previous_chars,
                            text.chars().count(),
                            None,
                        );
                    }
                    if let Some(dialogue) = shared_state.0.dialogue.as_mut() {
                        dialogue.text.push_str(&text);
                    }
                    if let (Some(current), Some(last)) = (
                        shared_state.0.dialogue.as_ref(),
                        dialogue_history.entries.last_mut(),
                    ) {
                        *last = current.clone();
                    }
                }
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id,
                    request: None,
                });
            }
            ScriptCommand::AwaitDialogueAdvance { done } => {
                dialogue_state.waiting = Some(PendingDialogueAdvance {
                    animation_id: None,
                    request: Some(done),
                });
            }
            ScriptCommand::SetDialogue {
                speaker,
                text,
                reveal_from,
                animation_id,
            } => {
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Visible;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = speaker.clone();
                }
                if let Ok(line_root) = line_text_entity.single() {
                    set_dialogue_line_text(
                        &mut commands,
                        &mut dialogue_state,
                        line_root,
                        &mut line_text,
                        &ui_fonts,
                        &ui_style,
                        &text,
                        reveal_from.unwrap_or_else(|| text.chars().count()),
                        animation_id,
                    );
                } else {
                    set_dialogue_model_reveal(
                        &mut dialogue_state,
                        &text,
                        reveal_from.unwrap_or_else(|| text.chars().count()),
                        animation_id,
                    );
                }
                shared_state.0.dialogue = Some(DialogueSnapshot { speaker, text });
            }
            ScriptCommand::ClearDialogue => {
                if let Some(waiting) = dialogue_state.waiting.take() {
                    complete_dialogue_wait(&mut commands, &mut animations, waiting);
                }
                clear_dialogue_spans(&mut commands, &mut dialogue_state);
                if let Ok(mut visibility) = dialogue_root.single_mut() {
                    *visibility = Visibility::Hidden;
                }
                if let Ok(mut speaker_node) = speaker_text.single_mut() {
                    **speaker_node = String::new();
                }
                if let Ok(mut line_node) = line_text.single_mut() {
                    **line_node = String::new();
                }
                shared_state.0.dialogue = None;
            }
            ScriptCommand::SetTextEffect(effect) => {
                apply_text_effect_spec(&mut dialogue_state.effect, effect);
                shared_state.0.text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::ResetTextEffect => {
                dialogue_state.effect = DialogueTextEffect::default();
                shared_state.0.text_effect = text_effect_snapshot(&dialogue_state.effect);
            }
            ScriptCommand::SetCamera {
                blur_intensity,
                zoom,
                offset,
                rotation,
                projection,
                scope,
                duration,
                ease,
                animation_id,
            } => {
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
            ScriptCommand::ApplyUserSettings(settings) => *user_settings = settings,
            ScriptCommand::AdjustUserSetting { name, delta } => {
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
            ScriptCommand::ApplyUiStyle(style_patch) => {
                apply_ui_style_patch(&mut ui_style, style_patch);
                refresh_dialogue_ui_style(
                    &ui_fonts,
                    &ui_style,
                    &mut dialogue_root_node,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_font,
                    &mut line_font,
                    &mut hint_font,
                    &mut hint_visibility,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ResetUiStyle => {
                *ui_style = UiStyle::default();
                refresh_dialogue_ui_style(
                    &ui_fonts,
                    &ui_style,
                    &mut dialogue_root_node,
                    &mut dialogue_background,
                    &mut dialogue_border,
                    &mut speaker_font,
                    &mut line_font,
                    &mut hint_font,
                    &mut hint_visibility,
                    &mut speaker_color,
                    &mut line_color,
                    &mut hint_color,
                );
            }
            ScriptCommand::ShowScreen { screen, done } => {
                let spawned = spawn_screen_ui(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &ui_fonts,
                    &ui_style,
                    &screen,
                );
                let root = spawned.root;
                let previous = screen_state.active_root.take();
                let images_ready = screen_images_ready(&images, &spawned.image_handles);
                if previous.is_none() && images_ready {
                    commands
                        .entity(root)
                        .insert((Visibility::Inherited, GlobalZIndex(SCREEN_MODAL_ACTIVE_Z)));
                    screen_state.active_root = Some(root);
                    screen_state.waiting = done;
                } else {
                    commands
                        .entity(root)
                        .insert((Visibility::Hidden, GlobalZIndex(SCREEN_MODAL_PENDING_Z)));
                    screen_state.pending_root = Some(crate::ui::PendingScreenRoot {
                        entity: root,
                        previous,
                        wait_images: spawned.image_handles,
                        ready_frames_remaining: SCREEN_READY_FRAMES,
                        done,
                    });
                    screen_state.waiting = None;
                }
            }
            ScriptCommand::WaitForScreenChoice { done } => {
                if screen_state.active_root.is_some() && screen_state.pending_root.is_none() {
                    screen_state.waiting = Some(done);
                } else {
                    commands.write_message(ScriptResponseMessage {
                        request: done,
                        response: ScriptResponse::Continue,
                    });
                }
            }
            ScriptCommand::ShowOverlay { name, screen } => {
                if let Some(root) = overlay_state.roots.remove(&name) {
                    commands.entity(root).try_despawn();
                }
                let spawned = spawn_screen_ui(
                    &mut commands,
                    &asset_server,
                    &texture_atlases,
                    &ui_fonts,
                    &ui_style,
                    &screen,
                );
                commands
                    .entity(spawned.root)
                    .insert((Visibility::Inherited, GlobalZIndex(SCREEN_ACTIVE_Z + 10)));
                overlay_state.roots.insert(name, spawned.root);
            }
            ScriptCommand::HideOverlay { name } => {
                if let Some(root) = overlay_state.roots.remove(&name) {
                    commands.entity(root).try_despawn();
                }
            }
            ScriptCommand::Choose {
                prompt,
                options,
                done,
            } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                spawn_choice_ui(&mut commands, &ui_fonts, &ui_style, &prompt, &options);
                choice_state.waiting = Some(done);
                choice_state.options = options;
            }
            ScriptCommand::ShowCharacter {
                actor_id,
                character_name,
                expressions,
                position,
                scale,
                focused,
                fade,
                animation_id,
            } => {
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
            ScriptCommand::HideCharacter { actor_id } => {
                despawn_character_actor(
                    &mut commands,
                    &mut stage,
                    &mut pending_characters,
                    &actor_id,
                );
                stage.character_positions.remove(&actor_id);
            }
            ScriptCommand::JumpCharacter {
                actor_id,
                height,
                duration,
                animation_id,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Jump { height },
                    duration,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::ShakeCharacter {
                actor_id,
                amplitude,
                duration,
                animation_id,
            } => {
                apply_character_motion(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    CharacterMotionKind::Shake { amplitude },
                    duration,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::AnimateCharacter {
                actor_id,
                keyframes,
                animation_id,
            } => {
                apply_character_timeline(
                    &mut commands,
                    &mut stage,
                    &shared_state,
                    &actor_id,
                    keyframes,
                    animation_id,
                    &mut animations,
                );
            }
            ScriptCommand::RestoreSnapshot { snapshot } => {
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                // clear_overlay_ui(&mut commands, &mut overlay_state);
                commands.insert_resource(CameraShakeState::default());
                commands.insert_resource(AnimationState::default());
                pending_characters.items.clear();
                camera_state.blur_intensity = snapshot.camera.blur;
                camera_state.zoom = snapshot.camera.zoom.max(0.01);
                camera_state.offset = Vec3::from_array(snapshot.camera.offset);
                camera_state.rotation = Vec3::from_array(snapshot.camera.rotation);
                camera_state.projection = if snapshot.camera.projection == "perspective" {
                    crate::script::CameraProjectionMode::Perspective
                } else {
                    crate::script::CameraProjectionMode::Orthographic
                };
                camera_state.effect_scope = if snapshot.camera.scope == "canvas" {
                    crate::script::CameraEffectScope::Canvas
                } else {
                    crate::script::CameraEffectScope::World
                };
                *camera_tweens = CameraTweenState::default();
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
                    snapshot.clone(),
                );
                shared_state.0 = snapshot;
            }
            ScriptCommand::MoveSprite {
                id,
                position,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::new(sprite.x, sprite.y, STAGE_Z_SPRITE + sprite.layer);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: Some(from),
                            to_translation: Some(Vec3::new(
                                position.x,
                                position.y,
                                STAGE_Z_SPRITE + sprite.layer,
                            )),
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during move");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for move_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::ScaleSprite {
                id,
                scale,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        let from = Vec3::splat(sprite.scale);
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: None,
                            to_alpha: None,
                            from_translation: None,
                            to_translation: None,
                            from_scale: Some(from),
                            to_scale: Some(Vec3::splat(scale)),
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during scale");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for scale_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::FadeSprite {
                id,
                alpha,
                duration,
                animation_id,
            } => {
                if let Some(entity) = stage.sprites.get(&id).copied() {
                    let snapshot = &shared_state.0;
                    if let Some(sprite) = snapshot.sprites.iter().find(|sprite| sprite.id == id) {
                        commands.entity(entity).insert(VisualTween {
                            from_alpha: Some(sprite.alpha),
                            to_alpha: Some(alpha),
                            from_translation: None,
                            to_translation: None,
                            from_scale: None,
                            to_scale: None,
                            timer: Timer::new(duration, TimerMode::Once),
                            animation_id,
                            despawn_on_finish: false,
                        });
                    } else {
                        warn!("sprite `{id}` missing snapshot during fade");
                        complete_missing_animation(&mut animations, animation_id);
                    }
                } else {
                    warn!("sprite `{id}` not found for fade_sprite");
                    complete_missing_animation(&mut animations, animation_id);
                }
            }
            ScriptCommand::Wait {
                duration,
                animation_id,
                done,
            } => {
                waits.items.push(PendingWait {
                    timer: Timer::new(duration, TimerMode::Once),
                    animation_id,
                    done,
                });
            }
            ScriptCommand::WaitAnimations { ids, done } => {
                if ids.iter().all(|id| animations.completed.contains(id)) {
                    commands.write_message(ScriptResponseMessage {
                        request: done,
                        response: ScriptResponse::Continue,
                    });
                } else {
                    animations.waits.push(PendingAnimationWait { ids, done });
                }
            }
            ScriptCommand::Shake {
                duration,
                amplitude,
                animation_id,
            } => {
                commands.insert_resource(CameraShakeState {
                    active: Some(CameraShake {
                        timer: Timer::new(duration, TimerMode::Once),
                        amplitude,
                        animation_id,
                    }),
                });
            }
            ScriptCommand::PlayBgm {
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
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                }
                stage.bgm = Some(bgm);
                shared_state.0.bgm = Some(AudioSnapshot { path, volume });
            }
            ScriptCommand::SetBgmVolume { volume } => {
                let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
                if let Some(bgm) = stage.bgm {
                    if let Some(snapshot) = shared_state.0.bgm.as_ref() {
                        commands.entity(bgm).insert(BgmChannel {
                            path: snapshot.path.clone(),
                            volume,
                        });
                    }
                    commands.entity(bgm).insert(BgmFade {
                        from: playback_volume,
                        to: playback_volume,
                        timer: Timer::new(std::time::Duration::ZERO, TimerMode::Once),
                        animation_id: None,
                    });
                }
                if let Some(snapshot) = shared_state.0.bgm.as_mut() {
                    snapshot.volume = volume;
                }
            }
            ScriptCommand::FadeBgm {
                volume,
                duration,
                animation_id,
            } => {
                let playback_volume = apply_volume_setting(volume, user_settings.bgm_volume);
                let from = shared_state
                    .0
                    .bgm
                    .as_ref()
                    .map(|bgm| bgm.volume)
                    .map(|volume| apply_volume_setting(volume, user_settings.bgm_volume))
                    .unwrap_or(playback_volume);
                if let Some(bgm) = stage.bgm {
                    if let Some(snapshot) = shared_state.0.bgm.as_ref() {
                        commands.entity(bgm).insert(BgmChannel {
                            path: snapshot.path.clone(),
                            volume,
                        });
                    }
                    commands.entity(bgm).insert(BgmFade {
                        from,
                        to: playback_volume,
                        timer: Timer::new(duration, TimerMode::Once),
                        animation_id,
                    });
                    if let Some(snapshot) = shared_state.0.bgm.as_mut() {
                        snapshot.volume = volume;
                    }
                } else {
                    if let Some(animation_id) = animation_id {
                        animations.completed.insert(animation_id);
                    }
                }
            }
            ScriptCommand::StopBgm => {
                if let Some(previous) = stage.bgm.take() {
                    commands.entity(previous).try_despawn();
                }
                shared_state.0.bgm = None;
            }
            ScriptCommand::PlayVoice {
                path,
                volume,
                mode,
                animation_id,
            } => {
                let playback_volume = apply_volume_setting(volume, user_settings.voice_volume);
                if mode == VoicePlaybackMode::Exclusive {
                    finish_active_voice(&mut commands, &mut animations, &mut voice_state);
                }
                let voice = commands
                    .spawn((
                        VoiceChannel {
                            path: path.clone(),
                            volume,
                        },
                        AudioPlayer::new(asset_server.load(path.clone())),
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
            ScriptCommand::StopVoice => {
                finish_all_voices(&mut commands, &mut animations, &mut voice_state);
            }
            ScriptCommand::PlaySfx { path, volume } => {
                let playback_volume = apply_volume_setting(volume, user_settings.sfx_volume);
                commands.spawn((
                    SfxChannel { volume },
                    AudioPlayer::new(asset_server.load(path)),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(playback_volume)),
                ));
            }
            ScriptCommand::SubmitBatch { mode, items } => match mode {
                BatchSubmitMode::Parallel => {
                    for item in items {
                        pending_script_commands.enqueue(*item.command);
                    }
                }
                BatchSubmitMode::Sequence => {
                    let mut remaining = items.into_iter().collect::<VecDeque<_>>();
                    let Some(first) = remaining.pop_front() else {
                        continue;
                    };
                    let current_handle = first.handle.clone();
                    pending_script_commands.enqueue(*first.command);
                    active_batches.items.push(ActiveScriptBatch {
                        remaining,
                        current_handle,
                    });
                }
            },
            ScriptCommand::CancelAnimations { ids } => {
                pending_cancels.ids.extend(ids);
            }
            ScriptCommand::Exit => {
                app_exit.write(AppExit::Success);
            }
            ScriptCommand::ReturnToTitle => {
                finish_all_voices(&mut commands, &mut animations, &mut voice_state);
                clear_choice_ui(&mut commands, &choice_ui_roots);
                clear_screen_ui(&mut commands, &mut screen_state);
                clear_overlay_ui(&mut commands, &mut overlay_state);
                script_runtime.mounted_ui_overlays.clear();
                script_runtime.ui_registry.clear();
                pending_characters.items.clear();
                shared_state.0 = SceneSnapshot::default();
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
                    SceneSnapshot::default(),
                );
                frontend.runtime_started = true;
                frontend.notice = None;
                if !frontend.startup_script.is_empty() {
                    let startup = frontend.startup_script.clone();
                    let story = vfs
                        .0
                        .read_text(&startup)
                        .map_err(|error| error.to_string())
                        .and_then(|source| compile_story_bytecode(&startup, &source))
                        .and_then(|bytecode| {
                            StoryRuntime::new(bytecode).map_err(|error| error.to_string())
                        });
                    match story {
                        Ok(mut story) => {
                            story.set_globals(crate::script::capabilities::engine_globals(
                                &user_settings,
                            ));
                            script_runtime.story = Some(story);
                            script_runtime.current_script = Some(startup);
                            script_runtime.story_events.clear();
                            script_runtime.pending_ui_screen = None;
                            script_runtime.wait_request = None;
                            script_runtime.response_inbox.clear();
                            script_runtime.task_requests.clear();
                        }
                        Err(error) => crate::script::emit_script_diagnostic(
                            "failed to return to HKS title:",
                            &error,
                        ),
                    }
                }
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

        assert_eq!(commands.enqueue(ScriptCommand::Log("first".into())), 0);
        assert_eq!(commands.enqueue(ScriptCommand::Log("second".into())), 1);
        let first = commands
            .dispatch_next()
            .expect("the first queued command must be available");
        assert_eq!(first.sequence, 0);

        commands.clear();
        assert_eq!(commands.enqueue(ScriptCommand::Log("third".into())), 2);
        let third = commands
            .dispatch_next()
            .expect("the post-clear command must be available");
        assert_eq!(third.sequence, 2);
        assert!(matches!(third.command, ScriptCommand::Log(message) if message == "third"));
    }
}
