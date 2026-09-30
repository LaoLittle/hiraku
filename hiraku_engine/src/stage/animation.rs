//! Model animation uses the story clock, never wall time. Bevy still evaluates
//! and skins the model; paused players are sampled at the saved story position.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageAnimation {
    pub clip: String,
    pub duration: f32,
    /// Opacity samples keyed by glTF material name. Values are absolute alpha.
    #[serde(default)]
    pub opacity: BTreeMap<String, Vec<(f32, f32)>>,
}

impl StageAnimation {
    pub(super) fn valid(&self) -> bool {
        !self.clip.trim().is_empty()
            && self.duration.is_finite()
            && self.duration > 0.0
            && self.opacity.iter().all(|(name, keys)| {
                !name.trim().is_empty()
                    && !keys.is_empty()
                    && keys.iter().all(|&(time, value)| {
                        time.is_finite()
                            && (0.0..=self.duration).contains(&time)
                            && value.is_finite()
                            && (0.0..=1.0).contains(&value)
                    })
                    && keys.windows(2).all(|pair| pair[0].0 < pair[1].0)
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagePlayback {
    pub id: u64,
    pub name: String,
    pub elapsed: f32,
    pub looping: bool,
    pub completed: bool,
}

impl StagePlayback {
    pub(super) fn new(id: u64, name: String, looping: bool) -> Self {
        Self {
            id,
            name,
            elapsed: 0.0,
            looping,
            completed: false,
        }
    }
    pub(super) fn valid(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.elapsed.is_finite()
            && self.elapsed >= 0.0
            && !(self.looping && self.completed)
    }
    fn advance(&mut self, delta: f32, duration: f32) {
        if self.looping {
            self.elapsed = (self.elapsed + delta).rem_euclid(duration);
        } else {
            self.elapsed = (self.elapsed + delta).min(duration);
            self.completed = self.elapsed >= duration;
        }
    }
}

#[derive(Component)]
pub(super) struct StagePlayer {
    playback: u64,
    node: bevy::animation::graph::AnimationNodeIndex,
}

fn opacity(keys: &[(f32, f32)], time: f32) -> f32 {
    let end = keys.partition_point(|key| key.0 <= time);
    if end == 0 {
        return keys[0].1;
    }
    if end == keys.len() {
        return keys[end - 1].1;
    }
    let (a, b) = (keys[end - 1], keys[end]);
    a.1 + (b.1 - a.1) * ((time - a.0) / (b.0 - a.0))
}

pub(super) fn sync(
    mut commands: Commands,
    mut shared: ResMut<crate::state::SceneSharedState>,
    mut runtime: ResMut<super::runtime::StageRuntime>,
    mut graphs: Option<ResMut<Assets<AnimationGraph>>>,
    mut players: Query<(Entity, &mut AnimationPlayer, Option<&mut StagePlayer>)>,
    surfaces: Query<
        (
            Entity,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<super::shading::GammaStageMaterial>>,
            &bevy::gltf::GltfMaterialName,
        ),
        With<super::runtime::StageSurface>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gamma_materials: ResMut<Assets<super::shading::GammaStageMaterial>>,
    parents: Query<&ChildOf>,
    time: crate::scene::playback::StoryTime,
    mut redraw: super::redraw::StageRedraw,
) {
    let Some(state) = shared.0.spatial_stage.as_mut() else {
        return;
    };
    if !runtime.ready(state) || runtime.error.is_some() {
        return;
    }
    let Some(playback) = state.animation.as_mut() else {
        return;
    };
    let Some(root) = runtime.root else { return };
    let Some(definition) = runtime.definition.as_ref() else {
        return;
    };
    let Some((index, (_, animation))) = definition
        .animations
        .iter()
        .enumerate()
        .find(|(_, (name, _))| *name == &playback.name)
    else {
        runtime.error = Some(format!("unknown stage animation `{}`", playback.name));
        return;
    };
    let Some(clip) = definition.animation_handles.get(index).cloned() else {
        runtime.error = Some("stage animation clips must be loaded through AssetServer".into());
        return;
    };
    let duration = animation.duration;
    if playback.elapsed > duration {
        runtime.error = Some(format!(
            "saved stage animation `{}` is past its duration",
            playback.name
        ));
        return;
    }
    let mut installed = false;
    let mut found = false;
    for (entity, mut player, bound) in &mut players {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            continue;
        }
        found = true;
        if let Some(bound) = bound.filter(|bound| bound.playback == playback.id) {
            player
                .play(bound.node)
                .pause()
                .set_seek_time(playback.elapsed);
        } else {
            let Some(graphs) = graphs.as_mut() else {
                runtime.error = Some("stage animation requires Bevy's AnimationPlugin".into());
                return;
            };
            let (graph, node) = AnimationGraph::from_clip(clip.clone());
            let graph = graphs.add(graph);
            player.stop_all();
            player.play(node).pause().set_seek_time(playback.elapsed);
            commands.entity(entity).insert((
                AnimationGraphHandle(graph),
                StagePlayer {
                    playback: playback.id,
                    node,
                },
            ));
            installed = true;
        }
    }
    if !found {
        runtime.error = Some(format!(
            "stage animation `{}` has no animation player",
            playback.name
        ));
        return;
    }
    if !playback.completed || installed {
        redraw.request();
        // Do not consume a frame while the deferred graph is being installed.
        if !installed {
            playback.advance(time.delta_secs(), duration);
            for (entity, mut player, bound) in &mut players {
                if parents
                    .iter_ancestors(entity)
                    .any(|ancestor| ancestor == root)
                {
                    if let Some(bound) = bound.filter(|bound| bound.playback == playback.id) {
                        player
                            .play(bound.node)
                            .pause()
                            .set_seek_time(playback.elapsed);
                    }
                }
            }
        }
    }
    for (entity, standard, gamma, name) in &surfaces {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            continue;
        }
        if let Some(keys) = animation.opacity.get(&name.0) {
            let value = opacity(keys, playback.elapsed);
            if let Some(handle) = standard {
                if let Some(mut material) = materials.get_mut(handle) {
                    if material.base_color.alpha() != value {
                        material.base_color.set_alpha(value);
                    }
                }
            }
            if let Some(handle) = gamma {
                if let Some(mut material) = gamma_materials.get_mut(handle) {
                    if material.base.base_color.alpha() != value {
                        material.base.base_color.set_alpha(value);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_pauses_loops_and_finishes_on_story_time() {
        let mut playback = StagePlayback::new(1, "opening".into(), false);
        playback.advance(0.25, 1.0);
        playback.advance(0.0, 1.0);
        assert_eq!(playback.elapsed, 0.25);
        playback.advance(2.0, 1.0);
        assert!(playback.completed);
        assert_eq!(playback.elapsed, 1.0);
        let mut playback = StagePlayback::new(2, "idle".into(), true);
        playback.advance(2.25, 1.0);
        assert_eq!(playback.elapsed, 0.25);
        assert!(!playback.completed);
    }
    #[test]
    fn opacity_clamps_endpoints_and_interpolates() {
        let keys = [(0.0, 1.0), (0.5, 0.0)];
        assert_eq!(opacity(&keys, 0.25), 0.5);
        assert_eq!(opacity(&keys, 1.0), 0.0);
    }

    #[test]
    fn playback_replacement_and_snapshot_preserve_identity_and_position() {
        use super::super::runtime::{StageCommand, StageSnapshot};
        let mut stage = None;
        StageSnapshot::apply(
            &mut stage,
            StageCommand::Open {
                id: 1,
                path: "room.stage.hson".into(),
            },
        )
        .expect("open");
        StageSnapshot::apply(
            &mut stage,
            StageCommand::Play {
                id: 1,
                playback: 2,
                name: "idle".into(),
                looping: true,
            },
        )
        .expect("play");
        stage
            .as_mut()
            .expect("stage")
            .animation
            .as_mut()
            .expect("animation")
            .advance(0.25, 1.0);
        let bytes = hiraku_script::bhson::to_vec(&stage).expect("snapshot");
        let mut restored: Option<StageSnapshot> =
            hiraku_script::bhson::from_slice(&bytes).expect("restore");
        let playback = restored
            .as_ref()
            .expect("stage")
            .animation
            .as_ref()
            .expect("animation");
        assert_eq!(playback.id, 2);
        assert_eq!(playback.elapsed, 0.25);
        assert!(playback.looping);
        StageSnapshot::apply(
            &mut restored,
            StageCommand::Play {
                id: 1,
                playback: 3,
                name: "closing".into(),
                looping: false,
            },
        )
        .expect("replace");
        let playback = restored
            .as_ref()
            .expect("stage")
            .animation
            .as_ref()
            .expect("animation");
        assert_eq!(playback.id, 3);
        assert_eq!(playback.elapsed, 0.0);
        restored
            .as_ref()
            .expect("stage")
            .validate()
            .expect("valid snapshot");
    }

    #[test]
    fn opacity_requires_sorted_finite_samples_in_clip_domain() {
        let mut animation = StageAnimation {
            clip: "room.glb#Animation0".into(),
            duration: 1.0,
            opacity: BTreeMap::from([("screen".into(), vec![(0.0, 1.0), (1.0, 0.0)])]),
        };
        assert!(animation.valid());
        animation.opacity.get_mut("screen").expect("curve")[1].0 = 0.0;
        assert!(!animation.valid());
        animation.opacity.get_mut("screen").expect("curve")[1] = (1.0, f32::NAN);
        assert!(!animation.valid());
    }
}
