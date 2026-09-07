use bevy::{
    prelude::Window,
    window::{MonitorSelection, WindowMode},
};
use hiraku_engine::UserSettings;

pub(crate) const DISPLAY_PREFERENCES_AVAILABLE: bool = true;

pub(crate) fn apply_display_preferences(window: &mut Window, settings: &UserSettings) {
    window.mode = if settings.fullscreen {
        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
    } else {
        WindowMode::Windowed
    };
    if !settings.fullscreen {
        window
            .resolution
            .set(settings.window_width as f32, settings.window_height as f32);
    }
}
