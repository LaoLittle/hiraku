//! Optional physical keyboard/IME adapter for a host window. Embedded engines
//! can instead send HirakuTextInput directly from their own input source.
use bevy::{
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
    window::Ime,
};
use hiraku_engine::input::{HirakuTextFocus, HirakuTextInput};

pub struct HirakuTextInputPlugin;

impl Plugin for HirakuTextInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, bridge_text_input);
    }
}

fn bridge_text_input(
    focus: Res<HirakuTextFocus>,
    keys: Res<ButtonInput<KeyCode>>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut ime: MessageReader<Ime>,
    mut windows: Query<&mut Window>,
    mut output: MessageWriter<HirakuTextInput>,
    mut composing: Local<bool>,
) {
    let active = focus.0.is_some();
    for mut window in &mut windows {
        if window.focused && window.ime_enabled != active {
            window.ime_enabled = active;
        }
    }
    for event in ime.read() {
        match event {
            Ime::Preedit { value, .. } if active => {
                *composing = !value.is_empty();
                output.write(HirakuTextInput::Preedit(value.clone()));
            }
            Ime::Commit { value, .. } if active => {
                *composing = false;
                output.write(HirakuTextInput::Insert(value.clone()));
            }
            Ime::Disabled { .. } => *composing = false,
            _ => {}
        }
    }
    if !active {
        *composing = false;
    }
    let shortcut = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    for event in keyboard.read() {
        if !active || event.state != ButtonState::Pressed || *composing {
            continue;
        }
        let command = match event.key_code {
            KeyCode::KeyA if shortcut => Some(HirakuTextInput::SelectAll),
            _ if shortcut => None,
            KeyCode::Backspace => Some(HirakuTextInput::Backspace),
            KeyCode::Delete => Some(HirakuTextInput::Delete),
            KeyCode::ArrowLeft => Some(HirakuTextInput::Left),
            KeyCode::ArrowRight => Some(HirakuTextInput::Right),
            KeyCode::Home => Some(HirakuTextInput::Home),
            KeyCode::End => Some(HirakuTextInput::End),
            KeyCode::Enter | KeyCode::NumpadEnter => Some(HirakuTextInput::Submit),
            KeyCode::Escape => Some(HirakuTextInput::Cancel),
            _ => event
                .text
                .as_ref()
                .map(|text| HirakuTextInput::Insert(text.to_string())),
        };
        if let Some(command) = command {
            output.write(command);
        }
    }
}
