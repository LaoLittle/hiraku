use bevy::{prelude::*, window::PrimaryWindow};
use hiraku_engine::UserSettings;

pub(crate) fn apply_preferences(
    mut settings: ResMut<UserSettings>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut applied: Local<Option<(bool, u32, u32)>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    if settings.display_available != crate::platform::DISPLAY_PREFERENCES_AVAILABLE {
        settings.display_available = crate::platform::DISPLAY_PREFERENCES_AVAILABLE;
    }
    let desired = (
        settings.fullscreen,
        settings.window_width,
        settings.window_height,
    );
    if *applied != Some(desired) {
        crate::platform::apply_display_preferences(&mut window, &settings);
        *applied = Some(desired);
    }
}
