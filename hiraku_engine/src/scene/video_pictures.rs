//! Video presentation is a picture surface, not a second animation system.
use std::collections::{BTreeMap, HashSet};

use bevy::{ecs::system::SystemParam, prelude::*};
use hiraku_video::{VideoAsset, VideoPlaybackId, VideoPlaybackState, VideoPlayer, VideoWorldView};

use super::pictures::{PictureState, PictureVideo};

#[derive(Component)]
pub(crate) struct VideoPicture {
    id: String,
    previous: Option<usize>,
    path: String,
    source: PictureVideo,
    playback: VideoPlaybackId,
    owns_playback: bool,
}

impl VideoPicture {
    pub(super) fn state<'a>(
        &self,
        id: &str,
        picture: &PictureState,
        player: &'a VideoPlayer,
    ) -> Option<&'a VideoPlaybackState> {
        if self.id == id
            && self.previous.is_none()
            && self.path == picture.path
            && picture.video.as_ref() == Some(&self.source)
        {
            player.state(self.playback)
        } else {
            None
        }
    }
}

#[derive(SystemParam)]
pub(crate) struct VideoPictures<'w, 's> {
    player: ResMut<'w, VideoPlayer>,
    clock: Res<'w, Time<super::clock::SceneClock>>,
    entities: Query<
        'w,
        's,
        (Entity, &'static mut VideoPicture, &'static mut Transform),
        Without<crate::render::world_sprite::WorldSprite>,
    >,
}

impl VideoPictures<'_, '_> {
    pub(super) fn ready(&self, pictures: &BTreeMap<String, PictureState>) -> HashSet<String> {
        self.entities
            .iter()
            .filter_map(|(_, entity, _)| {
                let picture = pictures.get(&entity.id)?;
                (entity.previous.is_none()
                    && picture.path == entity.path
                    && picture.video.as_ref() == Some(&entity.source)
                    && matches!(
                        self.player.state(entity.playback),
                        Some(
                            VideoPlaybackState::Playing
                                | VideoPlaybackState::Paused
                                | VideoPlaybackState::Finished
                                | VideoPlaybackState::Failed(_)
                        )
                    ))
                .then(|| entity.id.clone())
            })
            .collect()
    }

    pub(super) fn sync(
        &mut self,
        commands: &mut Commands,
        assets: &AssetServer,
        pictures: &BTreeMap<String, PictureState>,
        rendered: &BTreeMap<(String, Option<usize>), &PictureState>,
        canvas: Vec2,
        camera: Option<(&Transform, &Projection)>,
        background_view: Option<&crate::render::camera::CameraView>,
    ) {
        let mut existing = HashSet::new();
        let mut owners = Vec::new();
        for (entity, mut marker, mut transform) in &mut self.entities {
            let previous = marker.previous.or_else(|| {
                let incoming = pictures.get(&marker.id)?;
                if incoming.path == marker.path && incoming.video.as_ref() == Some(&marker.source) {
                    return None;
                }
                incoming.previous.iter().rposition(|old| {
                    old.path == marker.path && old.video.as_ref() == Some(&marker.source)
                })
            });
            let key = (marker.id.clone(), previous);
            let picture = rendered.get(&key).copied().filter(|picture| {
                picture.path == marker.path && picture.video.as_ref() == Some(&marker.source)
            });
            let Some(picture) = picture else {
                if marker.owns_playback {
                    self.player.skip(marker.playback);
                }
                commands.entity(entity).try_despawn();
                continue;
            };
            marker.previous = previous;
            existing.insert(key);
            if marker.owns_playback {
                owners.push((
                    marker.id.clone(),
                    marker.path.clone(),
                    marker.source.clone(),
                    marker.playback,
                ));
            }
            commands.entity(entity).try_insert(picture.view);
            let next =
                video_transform(picture, previous, pictures, canvas, camera, background_view);
            if *transform != next {
                *transform = next;
            }
            if marker.owns_playback {
                self.player
                    .set_opacity(marker.playback, picture.alpha * picture.tint[3]);
            } else {
                let mut view = VideoWorldView::new(marker.playback, Vec2::ONE).expect("unit view");
                view.set_opacity(picture.alpha * picture.tint[3])
                    .expect("validated picture alpha");
                commands.entity(entity).try_insert(view);
            }
            self.player
                .set_suspended(marker.playback, self.clock.context().paused);
        }
        for ((id, previous), picture) in rendered {
            let Some(source) = &picture.video else {
                continue;
            };
            if existing.contains(&(id.clone(), *previous)) {
                continue;
            }
            let entity = commands
                .spawn((
                    video_transform(
                        picture,
                        *previous,
                        pictures,
                        canvas,
                        camera,
                        background_view,
                    ),
                    picture.view,
                    Visibility::Inherited,
                    crate::render::camera::scene_layer(),
                ))
                .id();
            let layout = source.layout;
            if let Some((_, _, _, playback)) = owners.iter().find(|(owner_id, path, video, _)| {
                owner_id == id && path == &picture.path && video == source
            }) {
                if previous.is_none() && self.player.reparent_world(*playback, entity) {
                    // An interrupted replacement may return to a source whose
                    // owner is currently an outgoing layer. Promote the new
                    // surface so retiring that layer cannot stop this playback.
                    for (old_entity, mut old, _) in &mut self.entities {
                        if old.playback == *playback && old.owns_playback {
                            old.owns_playback = false;
                            let mut view =
                                VideoWorldView::new(*playback, Vec2::ONE).expect("unit view");
                            if let Some(old_picture) = rendered.get(&(old.id.clone(), old.previous))
                            {
                                view.set_opacity(old_picture.alpha * old_picture.tint[3])
                                    .expect("validated picture alpha");
                            }
                            commands.entity(old_entity).try_insert(view);
                        }
                    }
                    self.player
                        .set_opacity(*playback, picture.alpha * picture.tint[3]);
                    commands.entity(entity).insert(VideoPicture {
                        id: id.clone(),
                        previous: None,
                        path: picture.path.clone(),
                        source: source.clone(),
                        playback: *playback,
                        owns_playback: true,
                    });
                    continue;
                }
                let mut view = VideoWorldView::new(*playback, Vec2::ONE).expect("unit view");
                view.set_opacity(picture.alpha * picture.tint[3])
                    .expect("validated picture alpha");
                commands.entity(entity).insert((
                    view,
                    VideoPicture {
                        id: id.clone(),
                        previous: *previous,
                        path: picture.path.clone(),
                        source: source.clone(),
                        playback: *playback,
                        owns_playback: false,
                    },
                ));
                continue;
            }
            let asset: Handle<VideoAsset> = assets
                .load_builder()
                .with_settings(move |settings: &mut hiraku_video::VideoLoaderSettings| {
                    settings.layout = layout
                })
                .load(picture.path.clone());
            let playback = self
                .player
                .play_world(asset, entity, Vec2::ONE)
                .expect("unit video quad has valid dimensions");
            self.player.set_looping(playback, source.looping);
            self.player
                .set_opacity(playback, picture.alpha * picture.tint[3]);
            self.player
                .set_suspended(playback, self.clock.context().paused);
            commands.entity(entity).insert(VideoPicture {
                id: id.clone(),
                previous: *previous,
                path: picture.path.clone(),
                source: source.clone(),
                playback,
                owns_playback: true,
            });
            owners.push((id.clone(), picture.path.clone(), source.clone(), playback));
        }
    }
}

fn video_transform(
    picture: &PictureState,
    previous: Option<usize>,
    pictures: &BTreeMap<String, PictureState>,
    canvas: Vec2,
    camera: Option<(&Transform, &Projection)>,
    background_view: Option<&crate::render::camera::CameraView>,
) -> Transform {
    let mut transform = super::pictures::viewed_picture_transform(picture, canvas, background_view);
    if let Some(index) = previous {
        let incoming = &pictures[&picture.id];
        transform.translation.z = incoming.layer - (incoming.previous.len() - index) as f32 * 0.001;
    }
    if picture.screen_space
        && let Some((camera, projection)) = camera
    {
        transform =
            super::pictures::screen_picture_transform(transform, canvas, camera, projection);
    }
    let size = picture.size.map(Vec2::from_array).unwrap_or(canvas);
    transform.scale *= Vec3::new(size.x, size.y, 1.0);
    transform
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::pictures::{PictureCommand, apply_picture_command};
    use bevy::asset::AssetPlugin;

    fn fixture() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<VideoAsset>()
            .init_resource::<VideoPlayer>()
            .init_resource::<Time<super::super::clock::SceneClock>>()
            .init_resource::<crate::state::SceneSharedState>()
            .insert_resource(crate::HirakuCanvas {
                image: Handle::default(),
                size: UVec2::new(640, 360),
            })
            .add_systems(Update, super::super::pictures::sync_pictures);
        let mut shared = app
            .world_mut()
            .resource_mut::<crate::state::SceneSharedState>();
        apply_picture_command(
            &mut shared.0.pictures,
            PictureCommand::Show {
                dissolve: None,
                post_process: None,
                video: Some(PictureVideo {
                    looping: true,
                    layout: Default::default(),
                }),
                replace: false,
                screen_space: false,
                view: crate::scene::pictures::PictureView::Scene,
                size: Some([320.0, 180.0]),
                slice: None,
                color: None,
                id: "water".into(),
                path: "water.webma".into(),
                rect: None,
                position: [50.0, 50.0],
                scale: 2.0,
                rotation: 0.0,
                layer: 3.0,
                seconds: 1.0,
            },
        )
        .expect("video state");
        app
    }

    #[test]
    fn video_picture_uses_one_spatial_parent_and_clear_removes_it() {
        let mut app = fixture();
        app.update();
        let (entity, playback) = {
            let mut query = app
                .world_mut()
                .query::<(Entity, &VideoPicture, &Transform)>();
            let (entity, video, transform) = query.single(app.world()).expect("one video parent");
            assert_eq!(transform.translation, Vec3::new(0.0, 0.0, 3.0));
            assert_eq!(transform.scale, Vec3::new(640.0, 360.0, 2.0));
            assert!(app.world().get::<Node>(entity).is_none());
            assert!(
                app.world()
                    .get::<crate::render::world_sprite::WorldSprite>(entity)
                    .is_none()
            );
            (entity, video.playback)
        };
        app.update();
        assert_eq!(
            app.world()
                .get::<VideoPicture>(entity)
                .expect("same parent")
                .playback,
            playback
        );
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .pictures
            .clear();
        app.update();
        assert!(app.world().get_entity(entity).is_err());
    }

    #[test]
    fn replacement_keeps_outgoing_video_until_the_picture_transition_releases_it() {
        let mut app = fixture();
        app.update();
        let outgoing = app
            .world_mut()
            .query_filtered::<Entity, With<VideoPicture>>()
            .single(app.world())
            .expect("outgoing parent");
        {
            let mut shared = app
                .world_mut()
                .resource_mut::<crate::state::SceneSharedState>();
            let picture = shared.0.pictures.get_mut("water").expect("picture");
            let mut previous = picture.clone();
            previous.alpha = 1.0;
            previous.fade = None;
            previous.motion = None;
            picture.path = "other.webma".into();
            picture.previous.push(previous);
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<VideoPicture>(outgoing)
                .expect("retained outgoing")
                .previous,
            Some(0)
        );
        assert_eq!(
            app.world_mut()
                .query::<&VideoPicture>()
                .iter(app.world())
                .count(),
            2
        );
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .pictures
            .get_mut("water")
            .expect("picture")
            .previous
            .clear();
        app.update();
        assert!(app.world().get_entity(outgoing).is_err());
        assert_eq!(
            app.world_mut()
                .query::<&VideoPicture>()
                .iter(app.world())
                .count(),
            1
        );
    }

    #[test]
    fn same_playback_replacement_creates_a_non_owning_view() {
        let mut app = fixture();
        app.update();
        let playback = app
            .world_mut()
            .query::<&VideoPicture>()
            .single(app.world())
            .expect("primary")
            .playback;
        {
            let mut shared = app
                .world_mut()
                .resource_mut::<crate::state::SceneSharedState>();
            let picture = shared.0.pictures.get_mut("water").expect("picture");
            let mut previous = picture.clone();
            previous.alpha = 1.0;
            previous.fade = None;
            previous.motion = None;
            picture.previous.push(previous);
            picture.position = [75.0, 50.0];
        }
        app.update();
        let mut query = app.world_mut().query::<&VideoPicture>();
        let views: Vec<_> = query.iter(app.world()).collect();
        assert_eq!(views.len(), 2);
        assert!(views.iter().all(|view| view.playback == playback));
        assert_eq!(views.iter().filter(|view| view.owns_playback).count(), 1);
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .pictures
            .get_mut("water")
            .expect("picture")
            .previous
            .clear();
        app.update();
        let primary = app
            .world_mut()
            .query::<&VideoPicture>()
            .single(app.world())
            .expect("primary retained");
        assert_eq!(primary.playback, playback);
        assert!(primary.owns_playback);
    }

    #[test]
    fn returning_to_an_outgoing_video_transfers_ownership_before_retirement() {
        let mut app = fixture();
        app.update();
        let playback = app
            .world_mut()
            .query::<&VideoPicture>()
            .single(app.world())
            .expect("original")
            .playback;
        {
            let mut shared = app
                .world_mut()
                .resource_mut::<crate::state::SceneSharedState>();
            let picture = shared.0.pictures.get_mut("water").expect("picture");
            let mut old = picture.clone();
            old.previous.clear();
            old.alpha = 1.0;
            picture.previous.push(old);
            picture.path = "other.webma".into();
        }
        app.update();
        {
            let mut shared = app
                .world_mut()
                .resource_mut::<crate::state::SceneSharedState>();
            let picture = shared.0.pictures.get_mut("water").expect("picture");
            let mut old = picture.clone();
            old.previous.clear();
            old.alpha = 1.0;
            picture.previous.push(old);
            picture.path = "water.webma".into();
        }
        app.update();
        let mut query = app.world_mut().query::<&VideoPicture>();
        let primary = query
            .iter(app.world())
            .find(|p| p.previous.is_none())
            .expect("primary");
        assert_eq!(primary.playback, playback);
        assert!(primary.owns_playback);
        assert_eq!(
            query
                .iter(app.world())
                .filter(|p| p.playback == playback && p.owns_playback)
                .count(),
            1
        );
        app.world_mut()
            .resource_mut::<crate::state::SceneSharedState>()
            .0
            .pictures
            .get_mut("water")
            .expect("picture")
            .previous
            .clear();
        app.update();
        let primary = app
            .world_mut()
            .query::<&VideoPicture>()
            .single(app.world())
            .expect("surviving owner");
        assert_eq!(primary.playback, playback);
        assert!(primary.owns_playback);
    }

    #[test]
    fn video_picture_source_survives_serialization_without_runtime_entities() {
        let app = fixture();
        let picture = &app
            .world()
            .resource::<crate::state::SceneSharedState>()
            .0
            .pictures["water"];
        let serialized = hiraku_script::hson::to_string(picture).expect("serialize");
        let restored: PictureState =
            hiraku_script::hson::from_str(&serialized).expect("deserialize");
        assert_eq!(&restored, picture);
        assert!(restored.video.expect("video source").looping);
    }
}
