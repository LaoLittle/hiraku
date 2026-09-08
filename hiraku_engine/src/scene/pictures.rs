//! Named scene-space pictures. Identity, placement and animation survive saves.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureState {
    pub tint: [f32; 4],
    pub tint_tween: Option<PictureTint>,
    #[serde(default)]
    pub blur_radius: f32,
    #[serde(default)]
    pub blur_tween: Option<PictureBlur>,
    pub id: String,
    pub path: String,
    pub rect: Option<[f32; 4]>,
    /// Center position in virtual-screen percentages; offscreen values allowed.
    pub position: [f32; 2],
    pub scale: f32,
    pub rotation: f32,
    pub layer: f32,
    pub alpha: f32,
    pub motion: Option<PictureMotion>,
    #[serde(default)]
    pub fade: Option<PictureFade>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureTint {
    pub from: [f32; 4],
    pub to: [f32; 4],
    pub elapsed: f32,
    pub seconds: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureBlur {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub seconds: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureFade {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub seconds: f32,
    pub remove: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureMotion {
    pub from: [f32; 5],
    pub to: [f32; 5],
    pub elapsed: f32,
    pub seconds: f32,
    pub ease: String,
    pub remove: bool,
    #[serde(default)]
    pub offsets_x: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PictureCommand {
    Tint {
        id: String,
        color: [f32; 4],
        seconds: f32,
    },
    Blur {
        id: String,
        radius: f32,
        seconds: f32,
    },
    Show {
        id: String,
        path: String,
        rect: Option<[f32; 4]>,
        position: [f32; 2],
        scale: f32,
        rotation: f32,
        layer: f32,
        seconds: f32,
    },
    Move {
        id: String,
        position: [f32; 2],
        seconds: f32,
        ease: String,
    },
    Hide {
        id: String,
        seconds: f32,
    },
    AnimateX {
        id: String,
        offsets: Vec<f32>,
        step_seconds: f32,
    },
    Clear,
}

#[derive(Component)]
pub(crate) struct PictureEntity(pub String);

fn values(p: &PictureState) -> [f32; 5] {
    [p.position[0], p.position[1], p.scale, p.rotation, p.alpha]
}

pub(super) fn apply_picture_command(
    pictures: &mut BTreeMap<String, PictureState>,
    command: PictureCommand,
) -> Result<(), String> {
    match command {
        PictureCommand::Tint { id, color, seconds } => {
            if !color
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                || !seconds.is_finite()
                || seconds < 0.0
            {
                return Err("invalid picture tint or duration".into());
            }
            let picture = pictures
                .get_mut(&id)
                .ok_or_else(|| format!("picture `{id}` is not shown"))?;
            picture.tint_tween = (seconds > 0.0).then_some(PictureTint {
                from: picture.tint,
                to: color,
                elapsed: 0.0,
                seconds,
            });
            if seconds == 0.0 {
                picture.tint = color;
            }
        }
        PictureCommand::Blur {
            id,
            radius,
            seconds,
        } => {
            if !radius.is_finite()
                || !(0.0..=128.0).contains(&radius)
                || !seconds.is_finite()
                || seconds < 0.0
            {
                return Err("invalid picture blur radius or duration".into());
            }
            let picture = pictures
                .get_mut(&id)
                .ok_or_else(|| format!("picture `{id}` is not shown"))?;
            picture.blur_tween = (seconds > 0.0).then_some(PictureBlur {
                from: picture.blur_radius,
                to: radius,
                elapsed: 0.0,
                seconds,
            });
            if seconds == 0.0 {
                picture.blur_radius = radius;
            }
        }
        PictureCommand::Clear => pictures.clear(),
        PictureCommand::Show {
            id,
            path,
            rect,
            position,
            scale,
            rotation,
            layer,
            seconds,
        } => {
            // Only an update to the same visible image may interpolate its pose.
            let blur = pictures
                .get(&id)
                .map(|p| (p.blur_radius, p.blur_tween.clone()))
                .unwrap_or_default();
            let (tint, tint_tween) = pictures
                .get(&id)
                .map(|p| (p.tint, p.tint_tween.clone()))
                .unwrap_or(([1.0; 4], None));
            // Replacement images and a new show during hide own a fresh entrance.
            let old = pictures
                .get(&id)
                .filter(|picture| {
                    picture.path == path
                        && picture.rect == rect
                        && !picture.fade.as_ref().is_some_and(|fade| fade.remove)
                })
                .map(values);
            let target = [position[0], position[1], scale, rotation, 1.0];
            let start = old.unwrap_or([position[0], position[1], scale, rotation, 0.0]);
            let motion = (seconds > 0.0).then_some(PictureMotion {
                from: start,
                to: target,
                elapsed: 0.0,
                seconds,
                ease: "linear".into(),
                remove: false,
                offsets_x: Vec::new(),
            });
            let initial = if motion.is_some() { start } else { target };
            let fade = (seconds > 0.0).then_some(PictureFade {
                from: start[4],
                to: 1.0,
                elapsed: 0.0,
                seconds,
                remove: false,
            });
            pictures.insert(
                id.clone(),
                PictureState {
                    tint,
                    tint_tween,
                    blur_radius: blur.0,
                    blur_tween: blur.1,
                    id,
                    path,
                    rect,
                    position: [initial[0], initial[1]],
                    scale: initial[2],
                    rotation: initial[3],
                    layer,
                    alpha: initial[4],
                    motion,
                    fade,
                },
            );
        }
        PictureCommand::Move {
            id,
            position,
            seconds,
            ease,
        } => {
            let picture = pictures
                .get_mut(&id)
                .ok_or_else(|| format!("picture `{id}` is not visible"))?;
            let from = values(picture);
            let to = [
                position[0],
                position[1],
                picture.scale,
                picture.rotation,
                picture.alpha,
            ];
            if seconds == 0.0 {
                picture.position = position;
                picture.motion = None;
            } else {
                picture.motion = Some(PictureMotion {
                    from,
                    to,
                    elapsed: 0.0,
                    seconds,
                    ease,
                    remove: false,
                    offsets_x: Vec::new(),
                });
            }
        }
        PictureCommand::Hide { id, seconds } => {
            if seconds == 0.0 {
                pictures.remove(&id);
            } else if let Some(picture) = pictures.get_mut(&id) {
                // Freeze the currently displayed pose, including a partially
                // completed shake. No previous motion may run behind the exit.
                picture.motion = None;
                picture.fade = Some(PictureFade {
                    from: picture.alpha,
                    to: 0.0,
                    elapsed: 0.0,
                    seconds,
                    remove: true,
                });
            }
        }
        PictureCommand::AnimateX {
            id,
            offsets,
            step_seconds,
        } => {
            let picture = pictures
                .get_mut(&id)
                .ok_or_else(|| format!("picture `{id}` is not visible"))?;
            let from = values(picture);
            let mut to = from;
            to[0] += offsets.last().copied().unwrap_or(0.0);
            picture.motion = Some(PictureMotion {
                from,
                to,
                elapsed: 0.0,
                seconds: step_seconds * offsets.len() as f32,
                ease: "linear".into(),
                remove: false,
                offsets_x: offsets,
            });
        }
    }
    Ok(())
}

pub fn sync_pictures(
    mut commands: Commands,
    time: Res<Time>,
    canvas: Res<crate::HirakuCanvas>,
    assets: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut shared: ResMut<SceneSharedState>,
    mut entities: Query<(Entity, &PictureEntity, &mut WorldSprite, &mut Transform)>,
) {
    let pictures = &mut shared.0.pictures;
    // Keep the first visible frame at the start of its animation: loading a
    // large picture must not consume the entire entrance before it is ready.
    let ready: HashSet<_> = entities
        .iter()
        .filter_map(|(_, marker, sprite, _)| {
            let image = sprite.image.as_ref()?;
            let picture = pictures.get(&marker.0)?;
            (images.contains(image.id())
                && sprite.rect == picture.rect
                && image
                    .path()
                    .is_some_and(|path| path.to_string() == picture.path))
            .then(|| marker.0.clone())
        })
        .collect();
    pictures.retain(|id, picture| {
        // Exit is independent of image readiness: hiding an unloaded image
        // must not retain it forever or wait for a failed download.
        let exiting = picture.fade.as_ref().is_some_and(|fade| fade.remove);
        (!ready.contains(id) && !exiting) || tick_picture(picture, time.delta_secs())
    });
    let mut existing = HashSet::new();
    for (entity, marker, mut sprite, mut transform) in &mut entities {
        let Some(picture) = pictures.get(&marker.0) else {
            commands.entity(entity).try_despawn();
            continue;
        };
        existing.insert(marker.0.clone());
        if !sprite
            .image
            .as_ref()
            .and_then(|image| image.path())
            .is_some_and(|path| path.to_string() == picture.path)
        {
            sprite.image = Some(assets.load(picture.path.clone()));
        }
        if sprite.rect != picture.rect {
            sprite.rect = picture.rect;
        }
        if sprite.blur_radius != picture.blur_radius {
            sprite.blur_radius = picture.blur_radius;
        }
        let color = picture_color(picture);
        if sprite.color != color {
            sprite.color = color;
        }
        let next = picture_transform(picture, canvas.size.as_vec2());
        if *transform != next {
            *transform = next;
        }
    }
    for (id, picture) in pictures.iter().filter(|(id, _)| !existing.contains(*id)) {
        let mut sprite = WorldSprite::from_image(assets.load(picture.path.clone()));
        sprite.rect = picture.rect;
        sprite.blur_radius = picture.blur_radius;
        sprite.color = picture_color(picture);
        commands.spawn((
            PictureEntity(id.clone()),
            BackgroundLayer {
                path: picture.path.clone(),
            },
            sprite,
            picture_transform(picture, canvas.size.as_vec2()),
        ));
    }
}

fn picture_color(picture: &PictureState) -> Color {
    let [r, g, b, a] = picture.tint;
    Color::srgba(r, g, b, a * picture.alpha)
}

fn tick_picture(picture: &mut PictureState, delta: f32) -> bool {
    if let Some(tint) = &mut picture.tint_tween {
        tint.elapsed = (tint.elapsed + delta).min(tint.seconds);
        picture.tint = std::array::from_fn(|i| {
            tint.from[i] + (tint.to[i] - tint.from[i]) * (tint.elapsed / tint.seconds)
        });
        if tint.elapsed >= tint.seconds {
            picture.tint_tween = None;
        }
    }
    if let Some(blur) = &mut picture.blur_tween {
        blur.elapsed = (blur.elapsed + delta).min(blur.seconds);
        picture.blur_radius = blur.from + (blur.to - blur.from) * (blur.elapsed / blur.seconds);
        if blur.elapsed >= blur.seconds {
            picture.blur_tween = None;
        }
    }
    if let Some(motion) = &mut picture.motion {
        motion.elapsed = (motion.elapsed + delta).min(motion.seconds);
        let p = (motion.elapsed / motion.seconds).clamp(0.0, 1.0);
        let t = match motion.ease.as_str() {
            "easeOutQuad" => 1.0 - (1.0 - p).powi(2),
            "easeOutBack" => 1.0 + 2.70158 * (p - 1.0).powi(3) + 1.70158 * (p - 1.0).powi(2),
            _ => p,
        };
        let v = std::array::from_fn::<_, 5, _>(|i| {
            motion.from[i] + (motion.to[i] - motion.from[i]) * t
        });
        picture.position = [v[0], v[1]];
        picture.scale = v[2];
        picture.rotation = v[3];
        if !motion.offsets_x.is_empty() {
            let progress = p * motion.offsets_x.len() as f32;
            let index = (progress.floor() as usize).min(motion.offsets_x.len() - 1);
            let previous = if index == 0 {
                0.0
            } else {
                motion.offsets_x[index - 1]
            };
            picture.position[0] = motion.from[0]
                + previous
                + (motion.offsets_x[index] - previous) * (progress - index as f32).min(1.0);
        }
        if p == 1.0 {
            if motion.remove {
                return false;
            }
            picture.motion = None;
        }
    }
    if let Some(fade) = &mut picture.fade {
        fade.elapsed = (fade.elapsed + delta).min(fade.seconds);
        let t = fade.elapsed / fade.seconds;
        picture.alpha = fade.from + (fade.to - fade.from) * t;
        if t >= 1.0 {
            if fade.remove {
                return false;
            }
            picture.fade = None;
        }
    }
    true
}

fn picture_transform(p: &PictureState, canvas: Vec2) -> Transform {
    Transform::from_xyz(
        (p.position[0] / 100.0 - 0.5) * canvas.x,
        (p.position[1] / 100.0 - 0.5) * canvas.y,
        p.layer,
    )
    .with_scale(Vec3::splat(p.scale))
    .with_rotation(Quat::from_rotation_z(p.rotation.to_radians()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_preserves_pose_and_restores_mid_transition() {
        let mut pictures = shown();
        let position = pictures["room"].position;
        apply_picture_command(
            &mut pictures,
            PictureCommand::Tint {
                id: "room".into(),
                color: [0.0, 0.0, 0.0, 0.5],
                seconds: 1.0,
            },
        )
        .expect("tint");
        tick_picture(pictures.get_mut("room").expect("picture"), 0.5);
        assert_eq!(pictures["room"].tint, [0.5, 0.5, 0.5, 0.75]);
        assert_eq!(pictures["room"].position, position);
        assert_eq!(pictures["room"].path, "background/room");
        let encoded = hiraku_script::hson::to_vec(&pictures).expect("snapshot");
        let mut restored: BTreeMap<String, PictureState> =
            hiraku_script::hson::from_slice(&encoded).expect("restore");
        tick_picture(restored.get_mut("room").expect("picture"), 0.5);
        assert_eq!(restored["room"].tint, [0.0, 0.0, 0.0, 0.5]);
        assert!(restored["room"].tint_tween.is_none());
        apply_picture_command(
            &mut restored,
            PictureCommand::Hide {
                id: "room".into(),
                seconds: 1.0,
            },
        )
        .expect("hide");
        tick_picture(restored.get_mut("room").expect("picture"), 0.5);
        assert_eq!(picture_color(&restored["room"]).alpha(), 0.25);
        assert!(
            apply_picture_command(
                &mut restored,
                PictureCommand::Tint {
                    id: "missing".into(),
                    color: [1.0; 4],
                    seconds: 0.0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn local_blur_is_independent_retargetable_and_restorable() {
        let mut pictures = shown();
        pictures.insert("other".into(), pictures["room"].clone());
        apply_picture_command(
            &mut pictures,
            PictureCommand::Blur {
                id: "room".into(),
                radius: 16.0,
                seconds: 1.0,
            },
        )
        .expect("blur");
        tick_picture(pictures.get_mut("room").expect("room"), 0.5);
        assert_eq!(pictures["room"].blur_radius, 8.0);
        assert_eq!(pictures["other"].blur_radius, 0.0);
        let saved = hiraku_script::hson::to_vec(&pictures).expect("snapshot");
        let mut restored: BTreeMap<String, PictureState> =
            hiraku_script::hson::from_slice(&saved).expect("restore");
        tick_picture(restored.get_mut("room").expect("room"), 0.5);
        assert_eq!(restored["room"].blur_radius, 16.0);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Blur {
                id: "room".into(),
                radius: 0.0,
                seconds: 1.0,
            },
        )
        .expect("retarget");
        tick_picture(pictures.get_mut("room").expect("room"), 0.5);
        assert_eq!(pictures["room"].blur_radius, 4.0);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Blur {
                id: "room".into(),
                radius: 0.0,
                seconds: 0.0,
            },
        )
        .expect("disable");
        assert_eq!(pictures["room"].blur_radius, 0.0);
        assert!(pictures["room"].blur_tween.is_none());
        assert!(
            apply_picture_command(
                &mut pictures,
                PictureCommand::Blur {
                    id: "missing".into(),
                    radius: 1.0,
                    seconds: 0.0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn clearing_pictures_removes_render_entities_before_the_next_scene() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_resource::<Assets<Image>>()
            .init_resource::<SceneSharedState>()
            .insert_resource(crate::HirakuCanvas {
                image: Handle::default(),
                size: UVec2::new(1920, 1080),
            })
            .add_systems(Update, sync_pictures);
        app.world_mut()
            .resource_mut::<SceneSharedState>()
            .0
            .pictures = shown();
        let outgoing = app
            .world_mut()
            .spawn((
                PictureEntity("room".into()),
                BackgroundLayer {
                    path: "background/room".into(),
                },
                WorldSprite::from_image(Handle::default()),
                Transform::default(),
            ))
            .id();

        apply_picture_command(
            &mut app
                .world_mut()
                .resource_mut::<SceneSharedState>()
                .0
                .pictures,
            PictureCommand::Clear,
        )
        .expect("clear presentation layers");
        app.update();

        assert!(app.world().get_entity(outgoing).is_err());
        assert!(
            app.world()
                .resource::<SceneSharedState>()
                .0
                .pictures
                .is_empty()
        );
    }

    fn shown() -> BTreeMap<String, PictureState> {
        let mut pictures = BTreeMap::new();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Show {
                id: "room".into(),
                path: "background/room".into(),
                rect: None,
                position: [80.0, -35.0],
                scale: 3.0,
                rotation: -10.0,
                layer: 5.0,
                seconds: 0.3,
            },
        )
        .expect("valid picture");
        pictures
    }

    #[test]
    fn movement_and_fade_are_independent_and_saveable() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Move {
                id: "room".into(),
                position: [60.0, -20.0],
                seconds: 0.4,
                ease: "easeOutBack".into(),
            },
        )
        .expect("move visible picture");
        let picture = pictures.get("room").expect("picture exists");
        assert_eq!(
            picture.fade.as_ref().expect("fade survives movement").to,
            1.0
        );
        assert_eq!(picture.motion.as_ref().expect("movement").to[0], 60.0);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Hide {
                id: "room".into(),
                seconds: 0.5,
            },
        )
        .expect("fade out");
        let picture = pictures.get("room").expect("picture fades before removal");
        assert!(picture.motion.is_none());
        assert!(picture.fade.as_ref().expect("fade").remove);
        let data = hiraku_script::hson::to_vec(&pictures).expect("encode pictures");
        let restored: BTreeMap<String, PictureState> =
            hiraku_script::hson::from_slice(&data).expect("decode pictures");
        assert_eq!(pictures, restored);
    }

    #[test]
    fn movement_does_not_restore_alpha_after_fade_finishes() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Move {
                id: "room".into(),
                position: [60.0, -20.0],
                seconds: 1.0,
                ease: "linear".into(),
            },
        )
        .expect("move");
        let picture = pictures.get_mut("room").expect("picture");
        assert!(tick_picture(picture, 0.3));
        assert_eq!(picture.alpha, 1.0);
        assert!(tick_picture(picture, 0.2));
        assert_eq!(picture.alpha, 1.0);
        assert_eq!(picture.position, [70.0, -27.5]);
    }

    #[test]
    fn clearing_pictures_does_not_require_render_entities() {
        let mut pictures = shown();
        apply_picture_command(&mut pictures, PictureCommand::Clear).expect("clear");
        assert!(pictures.is_empty());
        assert!(
            apply_picture_command(
                &mut pictures,
                PictureCommand::Move {
                    id: "missing".into(),
                    position: [0.0, 0.0],
                    seconds: 1.0,
                    ease: "linear".into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn hide_cancels_shake_and_fades_from_the_current_pose() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::AnimateX {
                id: "room".into(),
                offsets: vec![-4.0, 4.0, 0.0],
                step_seconds: 1.0,
            },
        )
        .expect("shake starts");
        tick_picture(pictures.get_mut("room").expect("picture"), 0.15);
        let position = pictures["room"].position;
        let alpha = pictures["room"].alpha;
        apply_picture_command(
            &mut pictures,
            PictureCommand::Hide {
                id: "room".into(),
                seconds: 1.0,
            },
        )
        .expect("hide starts");
        let picture = pictures.get_mut("room").expect("picture fades");
        assert!(picture.motion.is_none());
        assert!(tick_picture(picture, 0.5));
        assert_eq!(picture.position, position);
        assert_eq!(picture.alpha, alpha * 0.5);
        assert!(!tick_picture(picture, 0.5));
    }

    #[test]
    fn replacing_a_still_does_not_replay_the_previous_pose() {
        let mut pictures = shown();
        tick_picture(pictures.get_mut("room").expect("old picture"), 0.3);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Show {
                id: "room".into(),
                path: "pictures/bob.png".into(),
                rect: None,
                position: [50.0, 50.0],
                scale: 1.0,
                rotation: 0.0,
                layer: 5.0,
                seconds: 1.0,
            },
        )
        .expect("replacement starts");
        let picture = pictures.get_mut("room").expect("new picture");
        assert_eq!(picture.position, [50.0, 50.0]);
        assert_eq!(picture.scale, 1.0);
        assert_eq!(picture.rotation, 0.0);
        assert_eq!(picture.alpha, 0.0);
        tick_picture(picture, 0.5);
        assert_eq!(picture.position, [50.0, 50.0]);
        assert_eq!(picture.alpha, 0.5);
    }
}
