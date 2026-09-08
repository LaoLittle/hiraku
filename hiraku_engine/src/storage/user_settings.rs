use bevy::prelude::Resource;
use hiraku_script::hson;
use hiraku_storage::BufferedStorage as PlatformStorage;
use serde::{Deserialize, Serialize};

use super::StorageError;
use crate::vfs::workspace_base_path;

const USER_SETTINGS_PATH: &str = "hiraku.hson";
const USER_SETTINGS_KEY: &str = "hiraku";

/// User preferences are independent of story saves. Hosts consume display
/// preferences; dialogue and audio systems consume their own fields.
#[derive(Clone, Debug, Resource, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserSettings {
    /// Host capability, never persisted or controlled by a story save.
    #[serde(skip)]
    pub display_available: bool,
    pub master_volume: f32,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    /// Multiplier on the script-authored characters-per-second.
    pub text_speed: f32,
    /// Seconds after reveal and voice completion before automatic advancement.
    pub auto_delay: f32,
    pub fullscreen: bool,
    pub window_width: u32,
    pub window_height: u32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            display_available: false,
            master_volume: 1.0,
            bgm_volume: 1.0,
            voice_volume: 1.0,
            sfx_volume: 1.0,
            text_speed: 1.0,
            auto_delay: 2.0,
            fullscreen: false,
            window_width: 1600,
            window_height: 900,
        }
    }
}

/// Typed requests shared by UI and other embedding clients.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PreferenceChange {
    MasterVolume(f32),
    TextSpeed(f32),
    AutoDelay(f32),
    Fullscreen(bool),
    Resolution { width: u32, height: u32 },
}

impl UserSettings {
    pub fn apply(&mut self, change: &PreferenceChange) -> Result<(), String> {
        match *change {
            PreferenceChange::MasterVolume(value) => self.master_volume = bounded(value, 0.0, 1.0)?,
            PreferenceChange::TextSpeed(value) => self.text_speed = bounded(value, 0.1, 3.0)?,
            PreferenceChange::AutoDelay(value) => self.auto_delay = bounded(value, 0.0, 10.0)?,
            PreferenceChange::Fullscreen(value) => self.fullscreen = value,
            PreferenceChange::Resolution { width, height } => {
                if !(640..=7680).contains(&width) || !(360..=4320).contains(&height) {
                    return Err("window resolution must be between 640x360 and 7680x4320".into());
                }
                self.window_width = width;
                self.window_height = height;
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        for value in [
            self.master_volume,
            self.bgm_volume,
            self.voice_volume,
            self.sfx_volume,
        ] {
            bounded(value, 0.0, 1.0)?;
        }
        bounded(self.text_speed, 0.1, 3.0)?;
        bounded(self.auto_delay, 0.0, 10.0)?;
        let mut checked = self.clone();
        checked.apply(&PreferenceChange::Resolution {
            width: self.window_width,
            height: self.window_height,
        })
    }
}

fn bounded(value: f32, min: f32, max: f32) -> Result<f32, String> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "setting must be finite and between {min} and {max}"
        ))
    }
}

pub fn read_user_settings() -> Result<UserSettings, StorageError> {
    let Some(payload) = settings_storage().read(USER_SETTINGS_KEY)? else {
        return Ok(UserSettings::default());
    };
    let payload = String::from_utf8(payload)
        .map_err(|error| StorageError::HsonData(format!("settings are not UTF-8: {error}")))?;
    let settings = hson::from_str::<UserSettings>(&payload).map_err(|error| {
        StorageError::HsonData(error.render_with_options(
            USER_SETTINGS_PATH,
            &payload,
            hiraku_script::RenderOptions::terminal(),
        ))
    })?;
    settings.validate().map_err(StorageError::HsonData)?;
    Ok(settings)
}

pub fn write_user_settings(settings: &UserSettings) -> Result<(), StorageError> {
    settings.validate().map_err(StorageError::HsonData)?;
    let payload =
        hson::to_string(settings).map_err(|error| StorageError::HsonData(error.to_string()))?;
    settings_storage().enqueue_write(USER_SETTINGS_KEY, payload.as_bytes())?;
    Ok(())
}

pub(super) fn settings_storage() -> PlatformStorage {
    PlatformStorage::new(
        workspace_base_path().join("config"),
        "hiraku.config",
        "hson",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_roundtrip_without_a_filesystem() {
        let mut settings = UserSettings::default();
        settings
            .apply(&PreferenceChange::TextSpeed(1.5))
            .expect("valid speed");
        settings
            .apply(&PreferenceChange::MasterVolume(0.5))
            .expect("valid volume");
        let encoded = hson::to_string(&settings).expect("serialize preferences");
        assert!(encoded.contains("textSpeed"));
        let decoded: UserSettings = hson::from_str(&encoded).expect("deserialize preferences");
        assert_eq!(decoded.text_speed, 1.5);
        assert_eq!(decoded.master_volume, 0.5);
        decoded.validate().expect("valid persisted preferences");
    }
    #[test]
    fn invalid_preferences_do_not_mutate_existing_values() {
        let mut settings = UserSettings::default();
        assert!(
            settings
                .apply(&PreferenceChange::TextSpeed(f32::NAN))
                .is_err()
        );
        assert!(settings.apply(&PreferenceChange::AutoDelay(-1.0)).is_err());
        assert_eq!(settings.text_speed, 1.0);
        assert_eq!(settings.auto_delay, 2.0);
    }
}
