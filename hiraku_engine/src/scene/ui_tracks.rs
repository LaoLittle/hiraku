use crate::ui::{UiKeyframe, UiShaderKeyframe, UiTrackReference};
use bevy::{
    asset::{AssetLoader, LoadContext, LoadState, io::Reader},
    prelude::*,
};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) struct UiTrack {
    pub frames: Vec<UiKeyframe>,
    #[serde(default)]
    pub uniforms: Vec<UiShaderKeyframe>,
}

#[derive(Asset, TypePath, Debug, Deserialize)]
pub(crate) struct UiTracks {
    pub tracks: BTreeMap<String, UiTrack>,
}

impl UiTracks {
    pub fn parse(source: &[u8]) -> Result<Self, String> {
        let data: Self =
            hiraku_script::hson::from_slice(source).map_err(|error| error.to_string())?;
        for (name, track) in &data.tracks {
            if name.is_empty() || !super::ui_keyframes::validate(&track.frames) {
                return Err(format!("invalid geometry keyframes in track `{name}`"));
            }
            if !track.uniforms.is_empty()
                && !crate::render::ui_quad::valid_shader_keys(&track.uniforms)
            {
                return Err(format!("invalid uniform keyframes in track `{name}`"));
            }
        }
        Ok(data)
    }
}

pub(crate) fn validate_document(
    vfs: &crate::vfs::HdpVfs,
    nodes: &[crate::ui::ScreenNode],
) -> Result<(), String> {
    fn visit(
        vfs: &crate::vfs::HdpVfs,
        nodes: &[crate::ui::ScreenNode],
        cache: &mut BTreeMap<String, UiTracks>,
    ) -> Result<(), String> {
        use crate::ui::ScreenNode;
        for node in nodes {
            match node {
                ScreenNode::Image(image) => {
                    if let Some(reference) = &image.layout.track {
                        if !cache.contains_key(&reference.path) {
                            let bytes = vfs
                                .read_bytes(&reference.path)
                                .map_err(|error| error.to_string())?;
                            let data = UiTracks::parse(&bytes)
                                .map_err(|error| format!("{}: {error}", reference.path))?;
                            cache.insert(reference.path.clone(), data);
                        }
                        let track = cache[&reference.path]
                            .tracks
                            .get(&reference.name)
                            .ok_or_else(|| {
                                format!(
                                    "{}: track `{}` is not defined",
                                    reference.path, reference.name
                                )
                            })?;
                        if !track.uniforms.is_empty() && image.layout.shader.is_none() {
                            return Err(format!(
                                "{}: track `{}` requires a shader",
                                reference.path, reference.name
                            ));
                        }
                    }
                }
                ScreenNode::Row(container) | ScreenNode::Column(container) => {
                    visit(vfs, &container.children, cache)?
                }
                ScreenNode::Scrollable(container) => visit(vfs, &container.children, cache)?,
                _ => {}
            }
        }
        Ok(())
    }
    visit(vfs, nodes, &mut BTreeMap::new())
}

#[derive(Default, TypePath)]
pub(crate) struct UiTracksLoader;
impl AssetLoader for UiTracksLoader {
    type Asset = UiTracks;
    type Settings = ();
    type Error = std::io::Error;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<UiTracks, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        UiTracks::parse(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
    fn extensions(&self) -> &[&str] {
        &["ui-tracks.hson"]
    }
}

#[derive(Component, Default)]
pub(crate) struct UiTrackAssets {
    pub handles: Vec<Handle<UiTracks>>,
    pub pending: usize,
}
#[derive(Component)]
pub(crate) struct PendingTrack {
    root: Entity,
    handle: Handle<UiTracks>,
    name: String,
}

pub(crate) fn mount(
    commands: &mut Commands,
    assets: &AssetServer,
    root: Entity,
    entity: Entity,
    reference: &UiTrackReference,
) {
    let handle = assets.load::<UiTracks>(reference.path.clone());
    commands.entity(entity).insert((
        PendingTrack {
            root,
            handle: handle.clone(),
            name: reference.name.clone(),
        },
        super::ui_keyframes::Opacity(0.0),
    ));
    commands.queue(move |world: &mut World| {
        if let Ok(mut root) = world.get_entity_mut(root) {
            if let Some(mut deps) = root.get_mut::<UiTrackAssets>() {
                deps.pending += 1;
                if !deps.handles.contains(&handle) {
                    deps.handles.push(handle);
                }
            } else {
                root.insert(UiTrackAssets {
                    handles: vec![handle],
                    pending: 1,
                });
            }
        }
    });
}

pub(crate) fn attach(
    mut commands: Commands,
    assets: Res<Assets<UiTracks>>,
    server: Res<AssetServer>,
    pending: Query<(
        Entity,
        &PendingTrack,
        Option<&crate::render::ui_quad::UiShaderSource>,
    )>,
    mut roots: Query<(Entity, &mut UiTrackAssets)>,
    mut redraw: crate::redraw::Redraw,
) {
    if !pending.is_empty() {
        redraw.request();
    }
    for (root, mut deps) in &mut roots {
        deps.pending = pending
            .iter()
            .filter(|(_, track, _)| track.root == root)
            .count();
    }
    for (entity, pending, shader) in &pending {
        let track = assets
            .get(&pending.handle)
            .map(|data| data.tracks.get(&pending.name));
        match track {
            Some(Some(track)) => {
                if !track.uniforms.is_empty() && shader.is_none() {
                    eprintln!(
                        "UI track `{}` has shader uniforms but its image has no shader",
                        pending.name
                    );
                } else {
                    let frames = super::ui_keyframes::UiKeyframes::new(&track.frames);
                    let shader = shader.filter(|_| !track.uniforms.is_empty()).map(|shader| {
                        crate::render::ui_quad::UiShaderSource {
                            shader: shader.shader.clone(),
                            textures: shader.textures.clone(),
                            keys: track.uniforms.clone(),
                            blend: shader.blend,
                            alpha_blend: shader.alpha_blend,
                        }
                    });
                    commands.queue(move |world: &mut World| {
                        if let Ok(mut entity) = world.get_entity_mut(entity) {
                            entity.insert(frames);
                            if let Some(shader) = shader {
                                entity.insert(shader);
                            }
                        }
                    });
                }
            }
            Some(None) => eprintln!(
                "UI track `{}` does not exist in its animation asset",
                pending.name
            ),
            None if matches!(
                server.get_load_state(pending.handle.id()),
                Some(LoadState::Failed(_))
            ) =>
            {
                eprintln!("failed to load UI animation track `{}`", pending.name);
            }
            None => continue,
        }
        let root = pending.root;
        commands.queue(move |world: &mut World| {
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                entity.remove::<PendingTrack>();
            }
            if let Some(mut deps) = world.get_mut::<UiTrackAssets>(root) {
                deps.pending = deps.pending.saturating_sub(1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_deserialize_typed_geometry_and_uniforms() {
        let source=br#".{ tracks: .{ alice: .{
            frames: [.{QuadStepAlpha: [0, 0, 0, 20, 0, 0, 30, 1]}, .{QuadStepAlpha: [1, 10, 0, 20, 0, 0, 30, 0]}],
            uniforms: [.{At: [0, 1, 0, 0, 1]}, .{At: [1, 0, 1, 0, 0]}]
        } } }"#;
        let data = UiTracks::parse(source).expect("typed timeline");
        assert_eq!(data.tracks["alice"].frames.len(), 2);
        assert_eq!(data.tracks["alice"].uniforms.len(), 2);
        let bad=br#".{ tracks: .{ bob: .{ frames: [.{Quad: [1, 0, 0, 20, 0, 0, 30, 1]}, .{Quad: [0, 0, 0, 20, 0, 0, 30, 1]}] } } }"#;
        assert!(
            UiTracks::parse(bad)
                .expect_err("decreasing times")
                .contains("bob")
        );
    }
}
