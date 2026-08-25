use std::fs;

use bevy::prelude::Resource;
use hiraku_script::hson;
use serde::{Deserialize, Serialize};

use crate::vfs::workspace_base_path;

use super::StorageError;

const USER_SETTINGS_PATH: &str = "config/hiraku.hson";

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
    #[cfg(target_arch = "wasm32")]
    return Ok(UserSettings::default());

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = workspace_base_path().join(USER_SETTINGS_PATH);
        match fs::read_to_string(path) {
            Ok(payload) => {
                let settings = hson::from_str::<UserSettingsFile>(&payload).map_err(|error| {
                    StorageError::HsonData(error.render(USER_SETTINGS_PATH, &payload))
                })?;
                Ok(UserSettings {
                    bgm_volume: settings.bgm_volume as f32,
                    voice_volume: settings.voice_volume as f32,
                    sfx_volume: settings.sfx_volume as f32,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(UserSettings::default())
            }
            Err(error) => Err(StorageError::Io(error)),
        }
    }
}

pub fn write_user_settings(settings: &UserSettings) -> Result<(), StorageError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = settings;
        return Ok(());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = workspace_base_path().join(USER_SETTINGS_PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = UserSettingsFile {
            bgm_volume: settings.bgm_volume.into(),
            voice_volume: settings.voice_volume.into(),
            sfx_volume: settings.sfx_volume.into(),
        };
        let payload =
            hson::to_string(&data).map_err(|error| StorageError::HsonData(error.to_string()))?;
        fs::write(path, payload)?;
        Ok(())
    }
}

fn default_volume() -> f32 {
    1.0
}

fn default_volume_f64() -> f64 {
    1.0
}
