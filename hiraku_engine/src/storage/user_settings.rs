use bevy::prelude::Resource;
use hiraku_script::hson;
use hiraku_storage::{ByteStorage, PlatformStorage};
use serde::{Deserialize, Serialize};

use crate::vfs::workspace_base_path;

use super::StorageError;

const USER_SETTINGS_PATH: &str = "hiraku.hson";
const USER_SETTINGS_KEY: &str = "hiraku";

#[derive(Clone, Debug, Resource, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default = "default_volume")]
    pub bgm_volume: f32,
    #[serde(default = "default_volume")]
    pub voice_volume: f32,
    #[serde(default = "default_volume")]
    pub sfx_volume: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserSettingsFile {
    #[serde(rename = "bgmVolume", default = "default_volume_f64")]
    bgm_volume: f64,
    #[serde(rename = "voiceVolume", default = "default_volume_f64")]
    voice_volume: f64,
    #[serde(rename = "sfxVolume", default = "default_volume_f64")]
    sfx_volume: f64,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            bgm_volume: 1.0,
            voice_volume: 1.0,
            sfx_volume: 1.0,
        }
    }
}

pub fn read_user_settings() -> Result<UserSettings, StorageError> {
    let Some(payload) = settings_storage().read(USER_SETTINGS_KEY)? else {
        return Ok(UserSettings::default());
    };
    let payload = String::from_utf8(payload)
        .map_err(|error| StorageError::HsonData(format!("settings are not UTF-8: {error}")))?;
    let settings = hson::from_str::<UserSettingsFile>(&payload).map_err(|error| {
        StorageError::HsonData(error.render_with_options(
            USER_SETTINGS_PATH,
            &payload,
            hiraku_script::RenderOptions::terminal(),
        ))
    })?;
    Ok(UserSettings {
        bgm_volume: settings.bgm_volume as f32,
        voice_volume: settings.voice_volume as f32,
        sfx_volume: settings.sfx_volume as f32,
    })
}

pub fn write_user_settings(settings: &UserSettings) -> Result<(), StorageError> {
    let data = UserSettingsFile {
        bgm_volume: settings.bgm_volume.into(),
        voice_volume: settings.voice_volume.into(),
        sfx_volume: settings.sfx_volume.into(),
    };
    let payload =
        hson::to_string(&data).map_err(|error| StorageError::HsonData(error.to_string()))?;
    settings_storage().write(USER_SETTINGS_KEY, payload.as_bytes())?;
    Ok(())
}

fn settings_storage() -> PlatformStorage {
    PlatformStorage::new(
        workspace_base_path().join("config"),
        "hiraku.config",
        "hson",
    )
}

fn default_volume() -> f32 {
    1.0
}

fn default_volume_f64() -> f64 {
    1.0
}
