//! Named scene-space pictures. Identity, placement and animation survive saves.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureState {
    #[serde(default)]
    pub screen_space: bool,
    /// Frozen backing layers retained until the incoming image is ready and
    /// its entrance finishes. Owned by this replacement, never by callbacks.
    #[serde(default)]
    pub previous: Vec<PictureState>,
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    #[serde(default)]
    pub slice: Option<[f32; 4]>,
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
        screen_space: bool,
        size: Option<[f32; 2]>,
        slice: Option<[f32; 4]>,
        color: Option<[f32; 4]>,
        id: String,
        path: String,
        rect: Option<[f32; 4]>,
        position: [f32; 2],
        scale: f32,
        rotation: f32,
        layer: f32,
        seconds: f32,
    },
    Transform {
        id: String,
        position: [Option<f32>; 2],
        scale: Option<f32>,
        rotation: Option<f32>,
        seconds: f32,
        ease: String,
    },
    Hide {
        id: String,
        seconds: f32,
    },
    Exit {
        id: String,
        position: [f32; 2],
        seconds: f32,
        ease: String,
    },
    AnimateX {
        id: String,
        offsets: Vec<f32>,
        step_seconds: f32,
    },
    Clear,
    StopMotion {
        id: String,
    },
}

#[derive(Component)]
pub(crate) struct PictureEntity(pub String);

#[derive(Component)]
pub(crate) struct PreviousPicture(usize);

fn values(p: &PictureState) -> [f32; 5] {
    [p.position[0], p.position[1], p.scale, p.rotation, p.alpha]
}

pub(super) fn apply_picture_command(
    pictures: &mut BTreeMap<String, PictureState>,
    command: PictureCommand,
) -> Result<(), String> {
    match command {
        PictureCommand::StopMotion { id } => {
            if let Some(picture) = pictures.get_mut(&id) {
                // State already contains the displayed interpolation sample.
                // Do not snap to the destination or cancel independent fades.
                picture.motion = None;
            }
        }
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
            screen_space,
            size,
            slice,
            color,
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
            let previous = pictures
                .get(&id)
                .map(|old| {
                    let mut layers = old.previous.clone();
                    if (old.path != path || old.rect != rect) && old.alpha > 0.0 {
                        let mut frozen = old.clone();
                        frozen.previous.clear();
                        frozen.motion = None;
                        frozen.fade = None;
                        frozen.tint_tween = None;
                        frozen.blur_tween = None;
                        layers.push(frozen);
                    }
                    layers
                })
                .unwrap_or_default();
            let blur = pictures
                .get(&id)
                .map(|p| (p.blur_radius, p.blur_tween.clone()))
                .unwrap_or_default();
            let (tint, tint_tween) = color.map(|color| (color, None)).unwrap_or_else(|| {
                pictures
                    .get(&id)
                    .map(|p| (p.tint, p.tint_tween.clone()))
                    .unwrap_or(([1.0; 4], None))
            });
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
                    screen_space,
                    previous,
                    size,
                    slice,
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
        PictureCommand::Transform {
            id,
            position,
            scale,
            rotation,
            seconds,
            ease,
        } => {
            let picture = pictures
                .get_mut(&id)
                .ok_or_else(|| format!("picture `{id}` is not visible"))?;
            let from = values(picture);
            let to = [
                position[0].unwrap_or(picture.position[0]),
                position[1].unwrap_or(picture.position[1]),
                scale.unwrap_or(picture.scale),
                rotation.unwrap_or(picture.rotation),
                picture.alpha,
            ];
            if seconds == 0.0 {
                picture.position = [to[0], to[1]];
                picture.scale = to[2];
                picture.rotation = to[3];
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
        PictureCommand::Exit {
            id,
            position,
            seconds,
            ease,
        } => {
            if seconds <= 0.0 {
                pictures.remove(&id);
            } else if let Some(picture) = pictures.get_mut(&id) {
                let from = values(picture);
                let mut to = from;
                to[0] = position[0];
                to[1] = position[1];
                picture.motion = Some(PictureMotion {
                    from,
                    to,
                    elapsed: 0.0,
                    seconds,
                    ease,
                    remove: false,
                    offsets_x: Vec::new(),
                });
                picture.fade = Some(PictureFade {
                    from: picture.alpha,
                    to: 0.0,
                    elapsed: 0.0,
                    seconds,
                    remove: true,
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
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    time: crate::scene::playback::StoryTime,
    canvas: Res<crate::HirakuCanvas>,
    assets: Res<AssetServer>,
    images: Res<Assets<Image>>,
    cameras: Query<
        (&Transform, &Projection),
        (
            With<crate::render::camera::WorldCamera3d>,
            Without<PictureEntity>,
        ),
    >,
    mut shared: ResMut<SceneSharedState>,
    mut entities: Query<(
        Entity,
        &PictureEntity,
        Option<&PreviousPicture>,
        &mut WorldSprite,
        &mut Transform,
    )>,
) {
    let crate::state::SceneSnapshot {
        pictures, clips, ..
    } = &mut shared.0;
    // Keep the first visible frame at the start of its animation: loading a
    // large picture must not consume the entire entrance before it is ready.
    let ready: HashSet<_> = entities
        .iter()
        .filter_map(|(_, marker, previous, sprite, _)| {
            if previous.is_some() {
                return None;
            }
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
        if picture.fade.is_some()
            || picture.motion.is_some()
            || picture.tint_tween.is_some()
            || picture.blur_tween.is_some()
            || !ready.contains(id)
        {
            redraw.request();
        }
        // Exit is independent of image readiness: hiding an unloaded image
        // must not retain it forever or wait for a failed download.
        let exiting = picture.fade.as_ref().is_some_and(|fade| fade.remove);
        if !ready.contains(id) && !exiting {
            return true;
        }
        let keep = tick_picture(picture, time.delta_secs());
        if picture.fade.is_none() && picture.motion.is_none() {
            picture.previous.clear();
        }
        keep
    });
    let render_pictures: BTreeMap<_, _> = pictures
        .iter()
        .flat_map(|(id, picture)| {
            std::iter::once(((id.clone(), None), picture)).chain(
                picture
                    .previous
                    .iter()
                    .enumerate()
                    .map(move |(i, p)| ((id.clone(), Some(i)), p)),
            )
        })
        .collect();
    let mut existing = HashSet::new();
    for (entity, marker, previous, mut sprite, mut transform) in &mut entities {
        let key = (marker.0.clone(), previous.map(|p| p.0));
        let Some(picture) = render_pictures.get(&key) else {
            commands.entity(entity).try_despawn();
            continue;
        };
        existing.insert(key.clone());
        let size = picture.size.map(Vec2::from_array);
        if sprite.custom_size != size {
            sprite.custom_size = size;
        }
        if sprite.slice != picture.slice {
            sprite.slice = picture.slice;
        }
        let clip = clips.picture(&marker.0);
        if sprite.clip != clip {
            sprite.clip = clip;
        }
        if !sprite
            .image
            .as_ref()
            .and_then(|image| image.path())
            .is_some_and(|path| path.to_string() == picture.path)
        {
            sprite.image = Some(crate::texture::load_static_image(
                &assets,
                picture.path.clone(),
            ));
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
        let mut next = picture_transform(picture, canvas.size.as_vec2());
        if let Some(index) = key.1 {
            let incoming = &pictures[&marker.0];
            next.translation.z = incoming.layer - (incoming.previous.len() - index) as f32 * 0.001;
        }
        if picture.screen_space {
            if let Ok((camera, projection)) = cameras.single() {
                next = screen_picture_transform(next, canvas.size.as_vec2(), camera, projection);
            }
        }
        if *transform != next {
            *transform = next;
        }
    }
    for ((id, previous), picture) in render_pictures
        .iter()
        .filter(|(key, _)| !existing.contains(*key))
    {
        let mut sprite = WorldSprite::from_image(crate::texture::load_static_image(
            &assets,
            picture.path.clone(),
        ));
        sprite.custom_size = picture.size.map(Vec2::from_array);
        sprite.slice = picture.slice;
        sprite.clip = clips.picture(id);
        sprite.rect = picture.rect;
        sprite.blur_radius = picture.blur_radius;
        sprite.color = picture_color(picture);
        let mut transform = picture_transform(picture, canvas.size.as_vec2());
        if let Some(index) = previous {
            transform.translation.z =
                pictures[id].layer - (pictures[id].previous.len() - index) as f32 * 0.001;
        }
        if picture.screen_space {
            if let Ok((camera, projection)) = cameras.single() {
                transform =
                    screen_picture_transform(transform, canvas.size.as_vec2(), camera, projection);
            }
        }
        let mut entity = commands.spawn((
            PictureEntity(id.clone()),
            BackgroundLayer {
                path: picture.path.clone(),
            },
            sprite,
            transform,
        ));
        if let Some(index) = previous {
            entity.insert(PreviousPicture(*index));
        }
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
            "easeOutSine" => (p * std::f32::consts::FRAC_PI_2).sin(),
            "easeInOutSine" => (1.0 - (p * std::f32::consts::PI).cos()) * 0.5,
            "smoothStep" => p * p * (3.0 - 2.0 * p),
            "easeOutQuad" => 1.0 - (1.0 - p).powi(2),
            "easeInQuad" => p * p,
            "easeInOutQuad" if p < 0.5 => 2.0 * p * p,
            "easeInOutQuad" => 1.0 - (-2.0 * p + 2.0).powi(2) / 2.0,
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
    if picture.fade.is_none() && picture.motion.is_none() {
        picture.previous.clear();
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

fn screen_picture_transform(
    mut local: Transform,
    canvas: Vec2,
    camera: &Transform,
    projection: &Projection,
) -> Transform {
    // Unproject screen coordinates onto a camera-facing plane. Derive pixel
    // scale from the actual projection, covering both orthographic and perspective.
    let clip = projection.get_clip_from_view();
    // Reverse-Z depth keeps overlays inside either lens's clipping range,
    // including small, metre-based stages. Higher layers remain nearer.
    let depth = match projection {
        Projection::Orthographic(_) => (0.8 + local.translation.z * 0.001).clamp(0.6, 0.95),
        _ => (0.1 + local.translation.z * 0.0001).clamp(0.05, 0.2),
    };
    let distance = -clip.inverse().project_point3(Vec3::new(0.0, 0.0, depth)).z;
    let w = (clip * Vec4::new(0.0, 0.0, -distance, 1.0)).w;
    let pixel_scale = Vec3::new(
        2.0 * w / (clip.x_axis.x * canvas.x),
        2.0 * w / (clip.y_axis.y * canvas.y),
        1.0,
    );
    local.translation = Vec3::new(
        local.translation.x * pixel_scale.x,
        local.translation.y * pixel_scale.y,
        -distance,
    );
    local.scale *= pixel_scale;
    camera.mul_transform(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_space_picture_keeps_projected_position_and_size_for_both_lenses() {
        let canvas = Vec2::new(2560.0, 1440.0);
        let local = Transform::from_xyz(-700.0, 200.0, 25.0);
        for perspective in [false, true] {
            for zoom in [1.0, 2.0, 3.0] {
                let mut lens = if perspective {
                    Projection::Perspective(PerspectiveProjection {
                        fov: 60.0_f32.to_radians() / zoom,
                        near: 0.1,
                        far: 100.0,
                        ..default()
                    })
                } else {
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                            viewport_height: canvas.y,
                        },
                        scale: 1.0 / zoom,
                        ..OrthographicProjection::default_3d()
                    })
                };
                lens.update(canvas.x, canvas.y);
                let camera = Transform::from_xyz(-6.0, 3.0, 10.0).with_rotation(Quat::from_euler(
                    EulerRot::XYZ,
                    0.1,
                    -0.2,
                    0.3,
                ));
                let sprite = screen_picture_transform(local, canvas, &camera, &lens);
                let clip = lens.get_clip_from_view() * camera.to_matrix().inverse();
                let center = clip.project_point3(sprite.translation);
                assert!(
                    center.z > 0.0 && center.z < 1.0,
                    "overlay must not be clipped"
                );
                let right = clip.project_point3(sprite.transform_point(Vec3::X));
                assert!((center.x - 2.0 * local.translation.x / canvas.x).abs() < 0.00001);
                assert!((center.y - 2.0 * local.translation.y / canvas.y).abs() < 0.00001);
                assert!((right.x - center.x - 2.0 / canvas.x).abs() < 0.00001);
            }
        }
    }

    #[test]
    fn atomic_exit_survives_restore_and_large_fast_forward_steps() {
        let mut pictures = shown();
        tick_picture(pictures.get_mut("room").expect("picture"), 0.3);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Exit {
                id: "room".into(),
                position: [20.0, 30.0],
                seconds: 0.4,
                ease: "easeInQuad".into(),
            },
        )
        .expect("one exit owns movement and removal");
        let picture = pictures.get_mut("room").expect("exiting picture");
        assert!(tick_picture(picture, 0.2));
        assert_eq!(picture.alpha, 0.5);
        assert_eq!(picture.position, [65.0, -18.75]);
        let saved = hiraku_script::hson::to_string(picture).expect("save exit");
        let mut restored: PictureState =
            hiraku_script::hson::from_str(&saved).expect("restore exit");
        assert!(!tick_picture(&mut restored, 1.0));
        // Nothing remains scheduled to address this identity after removal.
        assert_eq!(restored.alpha, 0.0);
    }

    #[test]
    fn exit_motion_and_fade_run_together_and_restore_at_the_sampled_pose() {
        let mut pictures = shown();
        tick_picture(pictures.get_mut("room").expect("shown picture"), 1.0);
        let before = pictures["room"].clone();
        assert_eq!(before.alpha, 1.0);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Hide {
                id: "room".into(),
                seconds: 0.4,
            },
        )
        .expect("exit fade");
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [None, Some(before.position[1] - 10.0)],
                scale: None,
                rotation: None,
                seconds: 0.4,
                ease: "easeInQuad".into(),
            },
        )
        .expect("exit movement");
        let picture = pictures.get_mut("room").expect("exiting picture");
        assert!(tick_picture(picture, 0.2));
        assert!((picture.position[1] - (before.position[1] - 2.5)).abs() < 0.001);
        assert!((picture.alpha - before.alpha * 0.5).abs() < 0.001);
        let data = hiraku_script::hson::to_vec(picture).expect("snapshot");
        let mut restored: PictureState = hiraku_script::hson::from_slice(&data).expect("restore");
        assert!(!tick_picture(picture, 0.2));
        assert!(!tick_picture(&mut restored, 0.2));
        assert_eq!(picture, &restored);
    }

    #[test]
    fn scale_after_cancel_uses_displayed_pose_and_restores_without_reshowing() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [Some(20.0), None],
                scale: None,
                rotation: None,
                seconds: 2.0,
                ease: "linear".into(),
            },
        )
        .expect("start motion");
        tick_picture(pictures.get_mut("room").expect("picture"), 0.4);
        apply_picture_command(
            &mut pictures,
            PictureCommand::StopMotion { id: "room".into() },
        )
        .expect("cancel");
        let before = pictures["room"].clone();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [None; 2],
                scale: Some(1.5),
                rotation: Some(20.0),
                seconds: 2.0,
                ease: "easeOutQuad".into(),
            },
        )
        .expect("retarget only scale and rotation");
        let picture = pictures.get_mut("room").expect("picture");
        assert_eq!(picture.position, before.position);
        assert_eq!(picture.path, before.path);
        assert_eq!(picture.fade, before.fade);
        assert_eq!(picture.previous, before.previous);
        tick_picture(picture, 0.5);
        let bytes = hiraku_script::hson::to_vec(picture).expect("snapshot");
        let mut restored: PictureState = hiraku_script::hson::from_slice(&bytes).expect("restore");
        tick_picture(picture, 1.5);
        tick_picture(&mut restored, 1.5);
        assert_eq!(picture, &restored);
        assert_eq!(restored.position, before.position);
        assert_eq!(restored.scale, 1.5);
        assert_eq!(restored.rotation, 20.0);
        assert!(restored.motion.is_none());
    }

    #[test]
    fn immediate_transform_changes_only_requested_fields() {
        let mut pictures = shown();
        let mut expected = pictures["room"].clone();
        expected.scale = 2.0;
        expected.position[1] = 25.0;
        expected.motion = None;
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [None, Some(25.0)],
                scale: Some(2.0),
                rotation: None,
                seconds: 0.0,
                ease: "linear".into(),
            },
        )
        .expect("immediate transform");
        assert_eq!(pictures["room"], expected);
    }

    #[test]
    fn smoothstep_motion_restores_without_changing_its_curve() {
        let mut pictures = shown();
        let origin = pictures["room"].position;
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [origin[0], origin[1] + 8.0].map(Some),
                scale: None,
                rotation: None,
                seconds: 1.0,
                ease: "smoothStep".into(),
            },
        )
        .expect("move");
        let picture = pictures.get_mut("room").expect("picture");
        tick_picture(picture, 0.25);
        assert!((picture.position[1] - origin[1] - 1.25).abs() < 0.001);
        let bytes = hiraku_script::hson::to_vec(picture).expect("serialize motion");
        let mut restored: PictureState =
            hiraku_script::hson::from_slice(&bytes).expect("restore motion");
        tick_picture(picture, 0.75);
        tick_picture(&mut restored, 0.75);
        assert_eq!(picture, &restored);
        assert_eq!(restored.position, [origin[0], origin[1] + 8.0]);
    }

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
                screen_space: false,
                path: "background/room".into(),
                size: None,
                slice: None,
                color: None,
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
    fn stopping_motion_preserves_pose_and_independent_fade() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [20.0, 25.0].map(Some),
                scale: None,
                rotation: None,
                seconds: 1.0,
                ease: "linear".into(),
            },
        )
        .expect("start movement");
        tick_picture(pictures.get_mut("room").expect("picture exists"), 0.25);
        let before = pictures["room"].clone();
        assert!(before.motion.is_some());
        assert_eq!(before.position, [65.0, -20.0]);
        assert!(before.fade.is_some());
        apply_picture_command(
            &mut pictures,
            PictureCommand::StopMotion { id: "room".into() },
        )
        .expect("stop shown picture");
        let mut expected = before;
        expected.motion = None;
        assert_eq!(pictures["room"], expected);
        apply_picture_command(
            &mut pictures,
            PictureCommand::StopMotion {
                id: "absent".into(),
            },
        )
        .expect("stopping an absent picture is idempotent");
    }

    #[test]
    fn movement_and_fade_are_independent_and_saveable() {
        let mut pictures = shown();
        apply_picture_command(
            &mut pictures,
            PictureCommand::Transform {
                id: "room".into(),
                position: [60.0, -20.0].map(Some),
                scale: None,
                rotation: None,
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
            PictureCommand::Transform {
                id: "room".into(),
                position: [60.0, -20.0].map(Some),
                scale: None,
                rotation: None,
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
                PictureCommand::Transform {
                    id: "missing".into(),
                    position: [0.0, 0.0].map(Some),
                    scale: None,
                    rotation: None,
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
    fn immediate_hide_then_same_image_show_restarts_fade_at_new_pose() {
        let mut pictures = shown();
        tick_picture(pictures.get_mut("room").expect("initial picture"), 0.3);
        apply_picture_command(
            &mut pictures,
            PictureCommand::Hide {
                id: "room".into(),
                seconds: 0.0,
            },
        )
        .expect("remove old presentation immediately");
        assert!(!pictures.contains_key("room"));
        apply_picture_command(
            &mut pictures,
            PictureCommand::Show {
                screen_space: false,
                id: "room".into(),
                path: "background/room".into(),
                size: None,
                slice: None,
                color: None,
                rect: None,
                position: [-40.0, 15.0],
                scale: 1.0,
                rotation: 0.0,
                layer: 5.0,
                seconds: 0.4,
            },
        )
        .expect("restart presentation");
        let picture = pictures.get_mut("room").expect("replacement picture");
        assert_eq!(picture.position, [-40.0, 15.0]);
        assert_eq!(picture.alpha, 0.0);
        assert!(picture.previous.is_empty());
        tick_picture(picture, 0.2);
        assert_eq!(picture.position, [-40.0, 15.0]);
        assert_eq!(picture.alpha, 0.5);
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
                screen_space: false,
                size: None,
                slice: None,
                color: None,
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
        assert_eq!(picture.previous.len(), 1);
        assert_eq!(picture.previous[0].alpha, 1.0);
        tick_picture(picture, 0.5);
        assert_eq!(picture.position, [50.0, 50.0]);
        assert_eq!(picture.alpha, 0.5);
        assert_eq!(picture.previous[0].alpha, 1.0);
        let encoded = hiraku_script::hson::to_string(picture).expect("save replacement layers");
        let mut restored: PictureState =
            hiraku_script::hson::from_str(&encoded).expect("restore replacement layers");
        tick_picture(&mut restored, 0.5);
        assert!(restored.previous.is_empty());
        assert_eq!(restored.path, "pictures/bob.png");
    }
}
