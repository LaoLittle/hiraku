use std::collections::BTreeMap;

use bevy::prelude::Resource;
use hiraku_script::hson;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    data::evaluate_hson_map,
    vfs::{HdpVfs, VfsError},
};

#[derive(Clone, Debug, Default, Resource)]
pub struct AudioCatalog {
    music: BTreeMap<String, AudioDefinition>,
    voices: BTreeMap<String, AudioDefinition>,
    sfx: BTreeMap<String, AudioDefinition>,
}

#[derive(Clone, Debug)]
pub struct AudioDefinition {
    pub path: String,
}

impl AudioCatalog {
    pub fn resolve_music(&self, name: &str) -> Option<&AudioDefinition> {
        self.music.get(name)
    }

    pub fn resolve_voice(&self, name: &str) -> Option<&AudioDefinition> {
        self.voices.get(name)
    }

    pub fn resolve_sfx(&self, name: &str) -> Option<&AudioDefinition> {
        self.sfx.get(name)
    }
}

#[derive(Debug, Error)]
pub enum AudioCatalogError {
    #[error("failed to read audio data: {0}")]
    Read(#[from] VfsError),
    #[error("failed to load audio data `{path}`: {message}")]
    Data { path: String, message: String },
}

#[derive(Debug, Deserialize)]
struct AudioFile {
    name: String,
    audio: String,
}

pub fn load_audio_catalog(vfs: &HdpVfs) -> Result<AudioCatalog, AudioCatalogError> {
    let voice_directory = vfs.load_voice_dir_path(None)?;
    Ok(AudioCatalog {
        music: load_channel(vfs, &vfs.load_bgm_dir_path(None)?, ".music.hson")?,
        voices: load_voice_channel(vfs, &voice_directory)?,
        sfx: load_channel(vfs, &vfs.load_soundeffects_dir_path(None)?, ".sfx.hson")?,
    })
}

#[derive(Debug, Deserialize)]
struct CharacterVoiceFile {
    #[serde(rename = "char")]
    _character: String,
    voices: Vec<CharacterVoiceEntry>,
}

#[derive(Debug, Deserialize)]
struct CharacterVoiceEntry {
    name: String,
    file: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum VoiceDescriptor {
    Character(CharacterVoiceFile),
    Single(AudioFile),
}

fn load_voice_channel(
    vfs: &HdpVfs,
    directory: &str,
) -> Result<BTreeMap<String, AudioDefinition>, AudioCatalogError> {
    let mut definitions = BTreeMap::new();
    let mut paths = match vfs.list_files_recursive(directory) {
        Ok(paths) => paths,
        Err(VfsError::NotFound(_)) => return Ok(definitions),
        Err(error) => return Err(error.into()),
    };
    paths.retain(|path| path.ends_with(".voice.hson") || path.ends_with(".voices.hson"));
    paths.sort();

    for descriptor_path in paths {
        let source = vfs.read_text(&descriptor_path)?;
        let descriptor: VoiceDescriptor =
            hson::from_str(&source).map_err(|error| AudioCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
            })?;
        let voices = match descriptor {
            VoiceDescriptor::Character(file) => file.voices,
            VoiceDescriptor::Single(file) => vec![CharacterVoiceEntry {
                name: file.name,
                file: file.audio,
            }],
        };
        for voice in voices {
            let definition = AudioDefinition {
                path: vfs.resolve_path(Some(&descriptor_path), &voice.file),
            };
            if definitions.insert(voice.name.clone(), definition).is_some() {
                return Err(AudioCatalogError::Data {
                    path: descriptor_path.clone(),
                    message: format!("audio `{}` is defined more than once", voice.name),
                });
            }
        }
    }
    Ok(definitions)
}

fn load_channel(
    vfs: &HdpVfs,
    directory: &str,
    extension: &str,
) -> Result<BTreeMap<String, AudioDefinition>, AudioCatalogError> {
    let mut paths = match vfs.list_files_recursive(directory) {
        Ok(paths) => paths,
        Err(VfsError::NotFound(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    paths.retain(|path| path.ends_with(extension));
    paths.sort();

    let mut definitions = BTreeMap::new();
    for descriptor_path in paths {
        let source = vfs.read_text(&descriptor_path)?;
        let data = evaluate_hson_map(&descriptor_path, &source).map_err(|error| {
            AudioCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
            }
        })?;
        let file: AudioFile = hson::from_value(hson::HsonValue::Map(data)).map_err(|error| {
            AudioCatalogError::Data {
                path: descriptor_path.clone(),
                message: error.to_string(),
            }
        })?;
        let definition = AudioDefinition {
            path: vfs.resolve_path(Some(&descriptor_path), &file.audio),
        };
        if definitions.insert(file.name.clone(), definition).is_some() {
            return Err(AudioCatalogError::Data {
                path: descriptor_path,
                message: format!("audio `{}` is defined more than once", file.name),
            });
        }
    }
    Ok(definitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_audio_aliases_from_hks_descriptors() {
        let root = std::env::temp_dir().join(format!("hiraku-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bgm")).unwrap();
        std::fs::create_dir_all(root.join("voice")).unwrap();
        std::fs::create_dir_all(root.join("soundeffects")).unwrap();
        std::fs::write(root.join("settings.hson"), ".{}").unwrap();
        std::fs::write(
            root.join("bgm/title.music.hson"),
            ".{ name: \"title\", audio: \"Title.ogg\" }",
        )
        .unwrap();
        std::fs::write(
            root.join("voice/ema_001.voice.hson"),
            ".{ name: \"ema/001\", audio: \"Ema_001.ogg\" }",
        )
        .unwrap();
        std::fs::write(
            root.join("voice/alice.voice.hson"),
            r#".{
                char: "alice",
                voices: (
                    .{ name: "voice/scene01/hash1", file: "hash1.ogg" },
                    .{ name: "voice/scene01/hash2", file: "hash2.ogg" }
                )
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("soundeffects/click.sfx.hson"),
            ".{ name: \"ui/click\", audio: \"click.wav\" }",
        )
        .unwrap();

        let vfs = HdpVfs::new_with_config(&root, "settings.hson", "startup.story.hks");
        let catalog = load_audio_catalog(&vfs).unwrap();
        assert_eq!(
            catalog.resolve_music("title").unwrap().path,
            "bgm/Title.ogg"
        );
        assert_eq!(
            catalog.resolve_voice("ema/001").unwrap().path,
            "voice/Ema_001.ogg"
        );
        assert_eq!(
            catalog.resolve_voice("voice/scene01/hash1").unwrap().path,
            "voice/hash1.ogg"
        );
        assert_eq!(
            catalog.resolve_sfx("ui/click").unwrap().path,
            "soundeffects/click.wav"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
