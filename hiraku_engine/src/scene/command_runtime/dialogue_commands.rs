use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_dialogue_command(
    command: DialogueCommand,
    commands: &mut Commands,
    dialogue_state: &mut DialogueState,
    dialogue_history: &mut DialogueHistoryState,
    shared_state: &mut SceneSharedState,
    animations: &mut AnimationState,
    dialogue_root: &mut Query<&mut Visibility, (With<DialogueRoot>, Without<HintText>)>,
    speaker_text: &mut Query<&mut Text, (With<SpeakerText>, Without<LineText>)>,
    line_text: &mut Query<&mut Text, (With<LineText>, Without<SpeakerText>)>,
    line_text_entity: &Query<Entity, (With<LineText>, Without<SpeakerText>)>,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
) {
    match command {
        DialogueCommand::Say {
            speaker,
            text,
            animation_id,
        } => {
            if let Some(waiting) = dialogue_state.waiting.take() {
                complete_dialogue_wait(commands, animations, waiting);
            }
            if let Ok(mut visibility) = dialogue_root.single_mut() {
                *visibility = Visibility::Visible;
            }
            if let Ok(mut speaker_node) = speaker_text.single_mut() {
                **speaker_node = speaker.clone();
            }
            if let Ok(line_root) = line_text_entity.single() {
                set_dialogue_line_text(
                    commands,
                    dialogue_state,
                    line_root,
                    line_text,
                    ui_fonts,
                    ui_style,
                    &text,
                    0,
                    None,
                );
            } else {
                set_dialogue_model_reveal(dialogue_state, &text, 0, None);
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
        DialogueCommand::Continue { text, animation_id } => {
            if let Some(waiting) = dialogue_state.waiting.take() {
                complete_dialogue_wait(commands, animations, waiting);
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
                        commands,
                        dialogue_state,
                        line_root,
                        line_text,
                        ui_fonts,
                        ui_style,
                        &text,
                        0,
                        None,
                    );
                } else {
                    set_dialogue_model_reveal(dialogue_state, &text, 0, None);
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
                        commands,
                        dialogue_state,
                        line_root,
                        ui_fonts,
                        ui_style,
                        &text,
                        None,
                    );
                } else {
                    append_dialogue_model_reveal(
                        dialogue_state,
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
        DialogueCommand::AwaitAdvance { done } => {
            dialogue_state.waiting = Some(PendingDialogueAdvance {
                animation_id: None,
                request: Some(done),
            });
        }
        DialogueCommand::Clear => {
            if let Some(waiting) = dialogue_state.waiting.take() {
                complete_dialogue_wait(commands, animations, waiting);
            }
            clear_dialogue_spans(commands, dialogue_state);
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
    }
}
