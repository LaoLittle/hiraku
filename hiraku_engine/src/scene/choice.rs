use super::*;

#[derive(Resource, Default)]
pub struct ChoiceState {
    pub waiting: Option<ScriptRequestId>,
    pub options: Vec<ChoiceOption>,
}

#[derive(Component)]
pub struct ChoiceUi;

#[derive(Component)]
pub struct ChoiceButton {
    pub index: usize,
}

pub fn handle_choice_buttons(
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    ui_style: Res<UiStyle>,
    mut clicks: MessageReader<Pointer<Click>>,
    mut interaction_query: Query<
        (&PickingInteraction, &mut BackgroundColor, &ChoiceButton),
        Changed<PickingInteraction>,
    >,
    button_query: Query<&ChoiceButton>,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    for (interaction, mut color, _) in &mut interaction_query {
        match *interaction {
            PickingInteraction::Pressed => *color = ui_style.choice_button_pressed.into(),
            PickingInteraction::Hovered => *color = ui_style.choice_button_hovered.into(),
            PickingInteraction::None => *color = ui_style.choice_button_bg.into(),
        }
    }
    for click in clicks.read() {
        if click.button != PointerButton::Primary {
            continue;
        }
        let Ok(button) = button_query.get(click.entity) else {
            continue;
        };
        resolve_choice(&mut commands, &mut choice_state, &choice_ui, button.index);
    }
}

pub fn handle_choice_action_input(
    mut actions: MessageReader<crate::input::HirakuActionInput>,
    mut commands: Commands,
    mut choice_state: ResMut<ChoiceState>,
    choice_ui: Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    if choice_state.waiting.is_none() {
        return;
    }
    let selected = actions.read().find_map(|action| match action.0 {
        crate::input::HirakuAction::Choice(index) => Some(index),
        _ => None,
    });
    if let Some(index) = selected {
        resolve_choice(&mut commands, &mut choice_state, &choice_ui, index);
    }
}

pub(super) fn spawn_choice_ui(
    commands: &mut Commands,
    ui_fonts: &UiFonts,
    ui_style: &UiStyle,
    prompt: &str,
    options: &[ChoiceOption],
) {
    let root = commands
        .spawn((
            ChoiceUi,
            Node {
                position_type: PositionType::Absolute,
                left: if ui_style.choice_panel_width > 0.0 {
                    Val::Auto
                } else {
                    px(24.0)
                },
                right: if ui_style.choice_panel_width > 0.0 {
                    Val::Auto
                } else {
                    px(24.0)
                },
                bottom: px(ui_style.choice_bottom),
                width: if ui_style.choice_panel_width > 0.0 {
                    px(ui_style.choice_panel_width)
                } else {
                    percent(100.0)
                },
                max_width: percent(92.0),
                padding: UiRect::all(px(ui_style.choice_padding)),
                flex_direction: FlexDirection::Column,
                row_gap: px(ui_style.choice_gap),
                justify_self: JustifySelf::Center,
                align_self: AlignSelf::Center,
                ..default()
            },
            BackgroundColor(ui_style.choice_panel_bg),
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        if !prompt.is_empty() {
            parent.spawn((
                ChoiceUi,
                Text::new(prompt),
                ui_text_font(ui_fonts, ui_style.choice_prompt_size),
                TextColor(ui_style.choice_prompt_color),
            ));
        }

        for (index, option) in options.iter().enumerate() {
            parent
                .spawn((
                    ChoiceUi,
                    ChoiceButton { index },
                    Button,
                    Node {
                        width: percent(100.0),
                        border: UiRect::all(px(1.0)),
                        padding: UiRect::axes(px(18.0), px(14.0)),
                        justify_content: if ui_style.choice_center_text {
                            JustifyContent::Center
                        } else {
                            JustifyContent::FlexStart
                        },
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(12.0)),
                        ..default()
                    },
                    BackgroundColor(ui_style.choice_button_bg),
                    BorderColor::all(ui_style.choice_button_border),
                ))
                .with_children(|button| {
                    let label = if ui_style.choice_show_indices {
                        format!("{}. {}", index + 1, option.text)
                    } else {
                        option.text.clone()
                    };
                    button.spawn((
                        ChoiceUi,
                        Text::new(label),
                        ui_text_font(ui_fonts, ui_style.choice_button_size),
                        TextColor(ui_style.choice_text_color),
                    ));
                });
        }
    });
}

pub(super) fn clear_choice_ui(
    commands: &mut Commands,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
) {
    for entity in choice_ui.iter() {
        commands.entity(entity).try_despawn();
    }
}

fn resolve_choice(
    commands: &mut Commands,
    choice_state: &mut ChoiceState,
    choice_ui: &Query<Entity, (With<ChoiceUi>, Without<ChildOf>)>,
    index: usize,
) {
    let Some(selected) = choice_state.options.get(index).cloned() else {
        return;
    };
    let Some(done) = choice_state.waiting.take() else {
        return;
    };
    clear_choice_ui(commands, choice_ui);
    choice_state.options.clear();
    commands.write_message(ScriptResponseMessage {
        request: done,
        response: ScriptResponse::Choice(selected.value),
    });
}
