//! Saveable named world-space clips, shared by pictures and composed actors.
use std::collections::BTreeMap;

use bevy::math::Vec2;
use hiraku_sprite3d::ClipRect;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipRegion {
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub rotation: f32,
}

impl ClipRegion {
    pub fn rect(&self) -> Result<ClipRect, &'static str> {
        ClipRect::new(
            Vec2::from(self.center),
            Vec2::from(self.size),
            self.rotation,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClipCommand {
    Define { name: String, region: ClipRegion },
    Remove { name: String },
    Actor { id: String, region: Option<String> },
    Picture { id: String, region: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipState {
    regions: BTreeMap<String, ClipRegion>,
    actors: BTreeMap<String, String>,
    pictures: BTreeMap<String, String>,
}

impl ClipState {
    pub fn apply(&mut self, command: ClipCommand) -> Result<(), String> {
        match command {
            ClipCommand::Define { name, region } => {
                if name.trim().is_empty() {
                    return Err("clip name must not be empty".into());
                }
                region.rect().map_err(str::to_owned)?;
                self.regions.insert(name, region);
            }
            ClipCommand::Remove { name } => {
                if self.regions.remove(&name).is_none() {
                    return Err(format!("clip `{name}` is not defined"));
                }
                self.actors.retain(|_, region| region != &name);
                self.pictures.retain(|_, region| region != &name);
            }
            ClipCommand::Actor { id, region } => {
                Self::attach(&self.regions, &mut self.actors, id, region)?
            }
            ClipCommand::Picture { id, region } => {
                Self::attach(&self.regions, &mut self.pictures, id, region)?
            }
        }
        Ok(())
    }

    fn attach(
        regions: &BTreeMap<String, ClipRegion>,
        targets: &mut BTreeMap<String, String>,
        id: String,
        region: Option<String>,
    ) -> Result<(), String> {
        if id.trim().is_empty() {
            return Err("clip target must not be empty".into());
        }
        if let Some(region) = region {
            if !regions.contains_key(&region) {
                return Err(format!("clip `{region}` is not defined"));
            }
            targets.insert(id, region);
        } else {
            targets.remove(&id);
        }
        Ok(())
    }

    fn resolve(&self, region: Option<&String>) -> Option<ClipRect> {
        self.regions.get(region?)?.rect().ok()
    }
    pub fn actor(&self, id: &str) -> Option<ClipRect> {
        self.resolve(self.actors.get(id))
    }
    pub fn picture(&self, id: &str) -> Option<ClipRect> {
        self.resolve(self.pictures.get(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_region_survives_snapshot_and_detaches_on_remove() {
        let mut state = ClipState::default();
        state
            .apply(ClipCommand::Define {
                name: "window".into(),
                region: ClipRegion {
                    center: [10.0, 20.0],
                    size: [40.0, 80.0],
                    rotation: 30.0,
                },
            })
            .expect("region");
        state
            .apply(ClipCommand::Actor {
                id: "alice".into(),
                region: Some("window".into()),
            })
            .expect("actor");
        state
            .apply(ClipCommand::Picture {
                id: "room".into(),
                region: Some("window".into()),
            })
            .expect("picture");
        assert_eq!(state.actor("alice"), state.picture("room"));
        let bytes = hiraku_script::hson::to_vec(&state).expect("serialize");
        let mut restored: ClipState = hiraku_script::hson::from_slice(&bytes).expect("restore");
        assert_eq!(state, restored);
        assert!(
            restored
                .apply(ClipCommand::Actor {
                    id: "bob".into(),
                    region: Some("missing".into())
                })
                .is_err()
        );
        restored
            .apply(ClipCommand::Remove {
                name: "window".into(),
            })
            .expect("remove");
        assert!(restored.actor("alice").is_none());
        assert!(restored.picture("room").is_none());
    }
}
