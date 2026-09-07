use bevy::prelude::Window;
use hiraku_engine::UserSettings;

// Browser layout/fullscreen belongs to the page and its trusted gesture handler.
pub(crate) const DISPLAY_PREFERENCES_AVAILABLE: bool = false;

pub(crate) fn apply_display_preferences(_window: &mut Window, _settings: &UserSettings) {}
