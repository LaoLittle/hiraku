use hiraku_script::hson::HsonMap;

use super::{VfsError, invalid_settings_data};

#[derive(Debug, Default)]
pub(super) struct BootSection {
    pub(super) startup: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct SettingsFile {
    pub(super) startup: Option<String>,
    pub(super) fonts: FontsSettings,
    pub(super) backgrounds_dir: Option<String>,
    pub(super) soundeffects_dir: Option<String>,
    pub(super) bgm_dir: Option<String>,
    pub(super) voice_dir: Option<String>,
    pub(super) characters_dir: Option<String>,
    pub(super) textures_dir: Option<String>,
    pub(super) res_root: Option<String>,
    pub(super) boot: BootSection,
}

#[derive(Debug, Default)]
pub(super) struct FontsSettings {
    pub(super) path: Option<String>,
}

pub(super) fn settings_from_data(mut data: HsonMap, path: &str) -> Result<SettingsFile, VfsError> {
    let startup = take_data_string(&mut data, "startup", path)?;
    let backgrounds_dir = take_data_string(&mut data, "backgroundsDir", path)?;
    let soundeffects_dir = take_data_string(&mut data, "soundeffectsDir", path)?;
    let bgm_dir = take_data_string(&mut data, "bgmDir", path)?;
    let voice_dir = take_data_string(&mut data, "voiceDir", path)?;
    let characters_dir = take_data_string(&mut data, "charactersDir", path)?;
    let textures_dir = take_data_string(&mut data, "texturesDir", path)?;
    let res_root = take_data_string(&mut data, "resRoot", path)?;

    let fonts = if let Some(mut fonts) = take_data_map(&mut data, "fonts", path)? {
        let path_value = take_data_string(&mut fonts, "path", path)?;
        ensure_empty_data_map(fonts, path, "fonts")?;
        FontsSettings { path: path_value }
    } else {
        FontsSettings::default()
    };

    let boot = if let Some(mut boot) = take_data_map(&mut data, "boot", path)? {
        let startup = take_data_string(&mut boot, "startup", path)?;
        ensure_empty_data_map(boot, path, "boot")?;
        BootSection { startup }
    } else {
        BootSection::default()
    };

    ensure_empty_data_map(data, path, "settings")?;
    Ok(SettingsFile {
        startup,
        fonts,
        backgrounds_dir,
        soundeffects_dir,
        bgm_dir,
        voice_dir,
        characters_dir,
        textures_dir,
        res_root,
        boot,
    })
}

pub(super) fn take_data_string(
    data: &mut HsonMap,
    key: &str,
    path: &str,
) -> Result<Option<String>, VfsError> {
    data.remove(key)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| invalid_settings_data(path, format!("`{key}` must be a string")))
        })
        .transpose()
}

fn take_data_map(data: &mut HsonMap, key: &str, path: &str) -> Result<Option<HsonMap>, VfsError> {
    data.remove(key)
        .map(|value| {
            value
                .as_map()
                .cloned()
                .ok_or_else(|| invalid_settings_data(path, format!("`{key}` must be a map")))
        })
        .transpose()
}

pub(super) fn ensure_empty_data_map(
    data: HsonMap,
    path: &str,
    section: &str,
) -> Result<(), VfsError> {
    if data.is_empty() {
        return Ok(());
    }
    let keys = data.keys().cloned().collect::<Vec<_>>().join(", ");
    Err(invalid_settings_data(
        path,
        format!("unknown {section} setting(s): {keys}"),
    ))
}
