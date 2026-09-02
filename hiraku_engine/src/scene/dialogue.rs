use super::*;

#[derive(Resource, Default)]
pub struct DialogueState {
    pub waiting: Option<PendingDialogueAdvance>,
    pub span_entities: Vec<Entity>,
    pub reveal: Option<DialogueRevealState>,
    pub effect: DialogueTextEffect,
}

#[derive(Resource, Default)]
pub struct DialogueHistoryState {
    pub entries: Vec<DialogueSnapshot>,
}

pub struct PendingDialogueAdvance {
    pub animation_id: Option<String>,
    pub request: Option<ScriptRequestId>,
}

pub struct DialogueRevealState {
    pub spans: Vec<Entity>,
    pub total_chars: usize,
    pub next_index: usize,
    pub accumulator: f32,
    pub interval: f32,
    pub fade_seconds: f32,
    pub animation_id: Option<String>,
}

#[derive(Clone)]
pub struct DialogueTextEffect {
    pub mode: DialogueTextEffectMode,
    pub cps: f32,
    pub fade_seconds: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogueTextEffectMode {
    Instant,
    TypewriterFade,
}

impl Default for DialogueTextEffect {
    fn default() -> Self {
        Self {
            mode: DialogueTextEffectMode::TypewriterFade,
            cps: 30.0,
            fade_seconds: 0.12,
        }
    }
}

#[derive(Component)]
pub struct DialogueCharSpan {
    pub target_alpha: f32,
    pub age: f32,
    pub revealed: bool,
}

#[derive(Component)]
pub struct SpeakerText;

#[derive(Component)]
pub struct LineText;

#[derive(Component)]
pub struct HintText;

#[derive(Component)]
pub struct DialogueRoot;

/// Lowest UI picking layer. It makes blank canvas clicks available for dialogue advancement
/// without teaching the host application about story state.
#[derive(Component)]
pub struct DialogueAdvanceSurface;

pub fn advance_dialogue_on_input(
    mut actions: MessageReader<crate::input::HirakuActionInput>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<&mut DialogueCharSpan>,
    mut responses: MessageWriter<ScriptResponseMessage>,
    choice_state: Res<ChoiceState>,
    screen_state: Res<ScreenUiState>,
    mut runtime_menu: ResMut<RuntimeMenuState>,
    ui_interactions: Query<
        Option<&PickingInteraction>,
        Or<(
            With<ScreenUiButton>,
            With<ScreenUiImageButton>,
            With<ScreenUiToggle>,
            With<RuntimeMenuButton>,
            With<ChoiceButton>,
            With<PauseMenuRoot>,
        )>,
    >,
    parents: Query<&ChildOf>,
) {
    let action_advance = actions
        .read()
        .any(|action| action.0 == crate::input::HirakuAction::NextDialogue);
    let mut pointer_advance = false;
    for click in clicks.read() {
        if let Some(count) = runtime_menu
            .consumed_pointer_clicks
            .get_mut(&click.pointer_id)
        {
            *count -= 1;
            if *count == 0 {
                runtime_menu
                    .consumed_pointer_clicks
                    .remove(&click.pointer_id);
            }
            continue;
        }
        pointer_advance |= click.pointer_id.is_custom()
            && find_component_ancestor(click.entity, &ui_interactions, &parents).is_none()
    }

    // Always drain both readers above so input produced while a modal is open
    // cannot be replayed after it closes.
    if choice_state.waiting.is_some() || screen_state.active_root.is_some() {
        return;
    }
    let advance = action_advance || pointer_advance;

    if !advance {
        return;
    }

    if action_advance
        && ui_interactions
            .iter()
            .flatten()
            .any(|interaction| !matches!(*interaction, PickingInteraction::None))
    {
        return;
    }

    advance_dialogue(
        &mut dialogue_state,
        &mut animations,
        &mut dialogue_chars,
        &mut responses,
    );
}

pub(super) fn advance_dialogue(
    dialogue_state: &mut DialogueState,
    animations: &mut AnimationState,
    dialogue_chars: &mut Query<&mut DialogueCharSpan>,
    responses: &mut MessageWriter<ScriptResponseMessage>,
) {
    if dialogue_reveal_has_hidden_chars(dialogue_state) {
        reveal_all_dialogue_chars(dialogue_state, dialogue_chars);
        return;
    }

    if let Some(waiting) = dialogue_state.waiting.take() {
        if let Some(animation_id) = waiting.animation_id {
            animations.completed.insert(animation_id);
        }
        if let Some(request) = waiting.request {
            responses.write(ScriptResponseMessage {
                request,
                response: ScriptResponse::Continue,
            });
        }
    }
}

pub fn animate_dialogue_text_reveal(
    time: Res<Time>,
    mut dialogue_state: ResMut<DialogueState>,
    mut animations: ResMut<AnimationState>,
    mut dialogue_chars: Query<(&mut TextColor, &mut DialogueCharSpan)>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    reveal.accumulator += time.delta_secs();
    while reveal.next_index < reveal.total_chars && reveal.accumulator >= reveal.interval {
        reveal.accumulator -= reveal.interval;
        if let Some(entity) = reveal.spans.get(reveal.next_index).copied()
            && let Ok((_, mut span)) = dialogue_chars.get_mut(entity)
        {
            span.revealed = true;
            span.age = 0.0;
        }
        reveal.next_index += 1;
    }

    let mut fully_visible = reveal.next_index >= reveal.total_chars;
    for &entity in &reveal.spans {
        if let Ok((mut color, mut span)) = dialogue_chars.get_mut(entity) {
            if span.revealed {
                span.age = (span.age + time.delta_secs()).min(reveal.fade_seconds);
                let alpha = if reveal.fade_seconds <= f32::EPSILON {
                    span.target_alpha
                } else {
                    span.target_alpha * (span.age / reveal.fade_seconds).clamp(0.0, 1.0)
                };
                color.0.set_alpha(alpha);
                if span.age + f32::EPSILON < reveal.fade_seconds {
                    fully_visible = false;
                }
            } else {
                color.0.set_alpha(0.0);
                fully_visible = false;
            }
        }
    }

    if fully_visible {
        if let Some(animation_id) = reveal.animation_id.take() {
            animations.completed.insert(animation_id);
        }
        dialogue_state.reveal = None;
    }
}

pub(super) fn clear_dialogue_spans(commands: &mut Commands, dialogue_state: &mut DialogueState) {
    for entity in dialogue_state.span_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    dialogue_state.reveal = None;
}

pub(super) fn set_dialogue_model_reveal(
    dialogue_state: &mut DialogueState,
    text: &str,
    visible_prefix_chars: usize,
    animation_id: Option<String>,
) {
    let total_chars = text.chars().count();
    let visible_prefix_chars = visible_prefix_chars.min(total_chars);
    if dialogue_state.effect.mode == DialogueTextEffectMode::Instant
        || visible_prefix_chars >= total_chars
    {
        dialogue_state.reveal = None;
        return;
    }
    dialogue_state.reveal = Some(DialogueRevealState {
        spans: Vec::new(),
        total_chars,
        next_index: visible_prefix_chars,
        accumulator: 0.0,
        interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
        fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
        animation_id,
    });
}

pub(super) fn append_dialogue_model_reveal(
    dialogue_state: &mut DialogueState,
    previous_chars: usize,
    appended_chars: usize,
    animation_id: Option<String>,
) {
    if appended_chars == 0 || dialogue_state.effect.mode == DialogueTextEffectMode::Instant {
        return;
    }
    let total_chars = previous_chars.saturating_add(appended_chars);
    if let Some(reveal) = dialogue_state.reveal.as_mut() {
        reveal.total_chars = total_chars;
        if reveal.animation_id.is_none() {
            reveal.animation_id = animation_id;
        }
    } else {
        dialogue_state.reveal = Some(DialogueRevealState {
            spans: Vec::new(),
            total_chars,
            next_index: previous_chars,
            accumulator: 0.0,
            interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
            fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
            animation_id,
        });
    }
}

pub(super) fn set_dialogue_line_text(
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    line_root: Entity,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    visible_prefix_chars: usize,
    animation_id: Option<String>,
) {
    clear_dialogue_spans(commands, dialogue_state);

    if let Ok(mut line_node) = line_text.single_mut() {
        **line_node = String::new();
    }

    let chars = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    let target_alpha = ui_style.line_color.alpha();
    let reveal_enabled = dialogue_state.effect.mode != DialogueTextEffectMode::Instant;
    let visible_prefix_chars = visible_prefix_chars.min(chars.len());

    for (index, ch) in chars.iter().enumerate() {
        let revealed = !reveal_enabled || index < visible_prefix_chars;
        let initial_alpha = if revealed { target_alpha } else { 0.0 };
        let entity = commands
            .spawn((
                TextSpan::new(ch.clone()),
                ui_text_font(ui_fonts, ui_style.line_size),
                TextColor(ui_style.line_color.with_alpha(initial_alpha)),
                DialogueCharSpan {
                    target_alpha,
                    age: if revealed {
                        dialogue_state.effect.fade_seconds
                    } else {
                        0.0
                    },
                    revealed,
                },
            ))
            .id();
        commands.entity(line_root).add_child(entity);
        dialogue_state.span_entities.push(entity);
    }

    if reveal_enabled && visible_prefix_chars < chars.len() {
        dialogue_state.reveal = Some(DialogueRevealState {
            spans: dialogue_state.span_entities.clone(),
            total_chars: chars.len(),
            next_index: visible_prefix_chars,
            accumulator: 0.0,
            interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
            fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
            animation_id,
        });
    } else {
        dialogue_state.reveal = None;
        let _ = animation_id;
    }
}

pub(super) fn append_dialogue_line_text(
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    line_root: Entity,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    text: &str,
    animation_id: Option<String>,
) {
    let start_index = dialogue_state.span_entities.len();
    let target_alpha = ui_style.line_color.alpha();
    let reveal_enabled = dialogue_state.effect.mode != DialogueTextEffectMode::Instant;

    for ch in text.chars() {
        let entity = commands
            .spawn((
                TextSpan::new(ch.to_string()),
                ui_text_font(ui_fonts, ui_style.line_size),
                TextColor(ui_style.line_color.with_alpha(if reveal_enabled {
                    0.0
                } else {
                    target_alpha
                })),
                DialogueCharSpan {
                    target_alpha,
                    age: if reveal_enabled {
                        0.0
                    } else {
                        dialogue_state.effect.fade_seconds
                    },
                    revealed: !reveal_enabled,
                },
            ))
            .id();
        commands.entity(line_root).add_child(entity);
        dialogue_state.span_entities.push(entity);
    }

    if reveal_enabled && start_index < dialogue_state.span_entities.len() {
        if let Some(reveal) = dialogue_state.reveal.as_mut() {
            reveal.spans = dialogue_state.span_entities.clone();
            if reveal.animation_id.is_none() {
                reveal.animation_id = animation_id;
            }
        } else {
            dialogue_state.reveal = Some(DialogueRevealState {
                spans: dialogue_state.span_entities.clone(),
                total_chars: dialogue_state.span_entities.len(),
                next_index: start_index,
                accumulator: 0.0,
                interval: (1.0 / dialogue_state.effect.cps.max(1.0)).max(0.0),
                fade_seconds: dialogue_state.effect.fade_seconds.max(0.0),
                animation_id,
            });
        }
    }
}

fn dialogue_reveal_has_hidden_chars(dialogue_state: &DialogueState) -> bool {
    dialogue_state
        .reveal
        .as_ref()
        .is_some_and(|reveal| reveal.next_index < reveal.total_chars)
}

fn reveal_all_dialogue_chars(
    dialogue_state: &mut DialogueState,
    dialogue_chars: &mut Query<&mut DialogueCharSpan>,
) {
    let Some(reveal) = dialogue_state.reveal.as_mut() else {
        return;
    };

    for &entity in &reveal.spans {
        if let Ok(mut span) = dialogue_chars.get_mut(entity) {
            span.revealed = true;
            span.age = reveal.fade_seconds;
        }
    }
    reveal.next_index = reveal.total_chars;
    reveal.accumulator = 0.0;
}

pub(super) fn text_effect_snapshot(effect: &DialogueTextEffect) -> TextEffectSnapshot {
    TextEffectSnapshot {
        mode: match effect.mode {
            DialogueTextEffectMode::Instant => "instant".to_string(),
            DialogueTextEffectMode::TypewriterFade => "typewriter_fade".to_string(),
        },
        cps: effect.cps,
        fade_seconds: effect.fade_seconds,
    }
}

pub(super) fn dialogue_text_effect_from_snapshot(
    snapshot: &TextEffectSnapshot,
) -> DialogueTextEffect {
    let mut effect = DialogueTextEffect::default();
    if snapshot.mode == "instant" {
        effect.mode = DialogueTextEffectMode::Instant;
    }
    if snapshot.cps > 0.0 {
        effect.cps = snapshot.cps;
    }
    if snapshot.fade_seconds >= 0.0 {
        effect.fade_seconds = snapshot.fade_seconds;
    }
    effect
}

pub(super) fn complete_dialogue_wait(
    commands: &mut Commands,
    animations: &mut AnimationState,
    waiting: PendingDialogueAdvance,
) {
    complete_missing_animation(animations, waiting.animation_id);
    if let Some(request) = waiting.request {
        commands.write_message(ScriptResponseMessage {
            request,
            response: ScriptResponse::Continue,
        });
    }
}
