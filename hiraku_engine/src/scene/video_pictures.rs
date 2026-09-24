//! Video presentation is a picture surface, not a second animation system.
use std::collections::{BTreeMap, HashSet};

use bevy::{ecs::system::SystemParam, prelude::*};
use hiraku_video::{VideoAsset, VideoPlaybackId, VideoPlaybackState, VideoPlayer};

use super::pictures::{PictureState, PictureVideo};

#[derive(Component)]
pub(crate) struct VideoPicture {
    id: String,
    previous: Option<usize>,
    path: String,
    source: PictureVideo,
    playback: VideoPlaybackId,
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
                self.player.skip(marker.playback);
                commands.entity(entity).try_despawn();
                continue;
            };
            marker.previous = previous;
            existing.insert(key);
            commands.entity(entity).try_insert(picture.view);
            let next =
                video_transform(picture, previous, pictures, canvas, camera, background_view);
            if *transform != next {
                *transform = next;
            }
            self.player
                .set_opacity(marker.playback, picture.alpha * picture.tint[3]);
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
            });
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
