//! Statement-scoped scene presentation builders, unrelated to anchored 3D stages.
use std::{collections::BTreeMap, time::Duration};

use hiraku_script::native::{NativeError, NativeRegistry};
use serde::{Deserialize, Serialize};

use super::{CameraScope, CharacterContext, Position, StoryEffect};
use crate::scene::clipping::{ClipCommand, ClipRegion};
use crate::scene::pictures::PictureCommand;

mod builders;
pub(super) use builders::*;

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "SceneClip", handle_type = 5)]
pub(super) struct SceneClipHandle(u64);

// Internal identity only: public handles expose the modifiers supported by
// their operation, rather than one universal scene-builder method table.
#[derive(Clone, Copy)]
pub(super) struct SceneTransitionHandle(pub(super) u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum SceneVisualTarget {
    Shake {
        amplitude: [f32; 2],
        interval: f32,
    },
    Spatial(crate::stage::runtime::StageCommand),
    Clip {
        name: String,
        region: ClipRegion,
    },
    HideCharacters {
        duration_ms: u64,
    },
    Picture(PictureCommand),
    Curtain {
        color: [u8; 3],
        opacity: f32,
        mask: Option<String>,
        softness: f32,
    },
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SceneVisualState {
    next: u64,
    pub(super) pending: BTreeMap<u64, (SceneVisualTarget, Option<u64>)>,
}

impl SceneVisualState {
    pub(super) fn hide_characters(
        &mut self,
        duration_ms: u64,
    ) -> Result<FadeTransitionHandle, NativeError> {
        self.begin_as(SceneVisualTarget::HideCharacters { duration_ms })
    }
    pub(super) fn begin_as<T: From<SceneTransitionHandle>>(
        &mut self,
        target: SceneVisualTarget,
    ) -> Result<T, NativeError> {
        self.begin(target).map(Into::into)
    }
    pub(super) fn begin(
        &mut self,
        target: SceneVisualTarget,
    ) -> Result<SceneTransitionHandle, NativeError> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| NativeError::message("scene transition identifiers exhausted"))?;
        self.pending.insert(self.next, (target, None));
        Ok(SceneTransitionHandle(self.next))
    }

    pub(super) fn commit(&mut self, effects: &mut Vec<StoryEffect>) {
        for (_, (target, fade_ms)) in std::mem::take(&mut self.pending) {
            effects.push(match target {
                SceneVisualTarget::Shake {
                    amplitude,
                    interval,
                } => StoryEffect::ShakeCamera {
                    amplitude,
                    interval,
                    duration_ms: fade_ms.unwrap_or(0),
                },
                SceneVisualTarget::Spatial(command) => StoryEffect::Spatial(command),
                SceneVisualTarget::Clip { name, region } => {
                    StoryEffect::Clip(ClipCommand::Define { name, region })
                }
                SceneVisualTarget::HideCharacters { duration_ms } => StoryEffect::HideCharacter {
                    actor_id: None,
                    fade_ms: fade_ms.unwrap_or(duration_ms),
                },
                SceneVisualTarget::Picture(mut picture) => {
                    let seconds = fade_ms.unwrap_or(0) as f32 / 1000.0;
                    match &mut picture {
                        PictureCommand::Show {
                            seconds: duration, ..
                        }
                        | PictureCommand::Hide {
                            seconds: duration, ..
                        }
                        | PictureCommand::Blur {
                            seconds: duration, ..
                        }
                        | PictureCommand::Tint {
                            seconds: duration, ..
                        }
                        | PictureCommand::Oscillate {
                            seconds: duration, ..
                        } => *duration = seconds,
                        _ => {}
                    }
                    StoryEffect::Picture(picture)
                }
                SceneVisualTarget::Curtain {
                    color,
                    opacity,
                    mask,
                    softness,
                } => StoryEffect::SetCurtain {
                    color,
                    opacity,
                    fade_ms,
                    mask,
                    softness,
                },
            });
        }
    }
}

pub(super) fn register(registry: &mut NativeRegistry<CharacterContext>) {
    api::register_hks(registry)
        .expect("scene presentation API registration must be internally consistent");
    builders::register(registry)
        .expect("scene builder API registration must be internally consistent");
}

fn milliseconds(seconds: f64) -> Result<u64, NativeError> {
    let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
        NativeError::message("duration must be finite, non-negative and representable")
    })?;
    u64::try_from(duration.as_millis())
        .map_err(|_| NativeError::message("duration exceeds the supported millisecond range"))
}

#[hiraku_script::hks_module]
mod api {
    use super::*;

    /// Additive, frame-stepped camera displacement in canvas units. No decay
    /// is imposed; the camera returns to its base transform at completion.
    #[hks(name = "shake", selector = "scene")]
    fn shake(
        context: &mut CharacterContext,
        x: f64,
        y: f64,
        interval: f64,
    ) -> Result<TimedSceneTransitionHandle, NativeError> {
        if ![x, y, interval].into_iter().all(f64::is_finite)
            || x < 0.0
            || y < 0.0
            || x > f32::MAX as f64
            || y > f32::MAX as f64
            || interval < 0.001
            || interval > 60.0
        {
            return Err(NativeError::message(
                "shake requires nonnegative finite amplitudes and an interval in 0.001..60 seconds",
            ));
        }
        context.scene_visuals.begin_as(SceneVisualTarget::Shake {
            amplitude: [x as f32, y as f32],
            interval: interval as f32,
        })
    }

    /// Dimensions and position are world-space canvas units, not percentages.
    #[hks(name = "clipRect", selector = "scene")]
    fn clip_rect(
        context: &mut CharacterContext,
        name: String,
        width: f64,
        height: f64,
    ) -> Result<SceneClipHandle, NativeError> {
        if name.trim().is_empty() {
            return Err(NativeError::message("clip name must not be empty"));
        }
        let region = ClipRegion {
            mask: None,
            center: [0.0; 2],
            size: [width as f32, height as f32],
            rotation: 0.0,
        };
        region.rect().map_err(NativeError::message)?;
        let handle = context
            .scene_visuals
            .begin(SceneVisualTarget::Clip { name, region })?;
        Ok(SceneClipHandle(handle.0))
    }

    /// Use texture alpha inside the world-space clip rectangle (pictures only).
    #[hks(name = "mask", selector = "SceneClip", receiver)]
    fn clip_mask(
        context: &mut CharacterContext,
        handle: SceneClipHandle,
        texture: String,
    ) -> Result<SceneClipHandle, NativeError> {
        if texture.trim().is_empty() {
            return Err(NativeError::message("clip mask texture must not be empty"));
        }
        let Some((SceneVisualTarget::Clip { region, .. }, _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("clip handle is not pending"));
        };
        region.mask = Some(texture);
        Ok(handle)
    }

    #[hks(name = "at", selector = "SceneClip", receiver)]
    fn clip_at(
        context: &mut CharacterContext,
        handle: SceneClipHandle,
        position: super::super::Position,
    ) -> Result<SceneClipHandle, NativeError> {
        let super::super::Position::Absolute(x, y) = position else {
            return Err(NativeError::message(
                "clip position requires .pos(x, y) in world-space canvas units",
            ));
        };
        let Some((SceneVisualTarget::Clip { region, .. }, _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("clip builder has already committed"));
        };
        let mut next = region.clone();
        next.center = [x as f32, y as f32];
        next.rect().map_err(NativeError::message)?;
        *region = next;
        Ok(handle)
    }

    #[hks(name = "rotation", selector = "SceneClip", receiver)]
    fn clip_rotation(
        context: &mut CharacterContext,
        handle: SceneClipHandle,
        degrees: f64,
    ) -> Result<SceneClipHandle, NativeError> {
        let Some((SceneVisualTarget::Clip { region, .. }, _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("clip builder has already committed"));
        };
        let mut next = region.clone();
        next.rotation = degrees as f32;
        next.rect().map_err(NativeError::message)?;
        *region = next;
        Ok(handle)
    }

    #[hks(name = "removeClip", selector = "scene")]
    fn remove_clip(context: &mut CharacterContext, name: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Clip(ClipCommand::Remove { name }));
        Ok(())
    }

    /// Attach independently of picture visibility; replacement retains the clip.
    #[hks(name = "clipPicture", selector = "scene")]
    fn clip_picture(
        context: &mut CharacterContext,
        id: String,
        region: Option<String>,
    ) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Clip(ClipCommand::Picture { id, region }));
        Ok(())
    }

    #[hks(name = "picture", selector = "scene")]
    fn picture(
        context: &mut CharacterContext,
        id: String,
        texture: String,
    ) -> Result<PictureShowHandle, NativeError> {
        if id.trim().is_empty() || texture.trim().is_empty() {
            return Err(NativeError::message(
                "picture identity and texture must not be empty",
            ));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Show {
                dissolve: None,
                post_process: None,
                video: None,
                replace: false,
                screen_space: false,
                view: crate::scene::pictures::PictureView::Scene,
                size: None,
                slice: None,
                color: None,
                id,
                path: texture,
                rect: None,
                position: [50.0, 50.0],
                scale: 1.0,
                rotation: 0.0,
                layer: 1.0,
                seconds: 0.0,
            }))
    }

    /// A named animated picture; pose, transitions and hiding use Picture APIs.
    #[hks(name = "video", selector = "scene")]
    fn video(
        context: &mut CharacterContext,
        id: String,
        movie: String,
    ) -> Result<PictureShowHandle, NativeError> {
        let handle = picture(context, id, movie)?;
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { video, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("missing video builder"));
        };
        *video = Some(crate::scene::pictures::PictureVideo {
            layout: Default::default(),
            looping: false,
        });
        Ok(handle)
    }

    #[hks(name = "looping", receiver)]
    fn video_looping(
        context: &mut CharacterContext,
        handle: PictureShowHandle,
        looping: bool,
    ) -> Result<PictureShowHandle, NativeError> {
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                video: Some(video), ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "looping requires an uncommitted video picture",
            ));
        };
        video.looping = looping;
        Ok(handle)
    }

    pub(super) fn picture_at(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        position: Position,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let [x, y] = match position {
            Position::Left => [25.0, 50.0],
            Position::Center => [50.0, 50.0],
            Position::Right => [75.0, 50.0],
            Position::Relative(x, y) => [x, y],
            Position::Absolute(x, y) => [(x / 1920.0 + 0.5) * 100.0, (y / 1080.0 + 0.5) * 100.0],
        };
        let Some((SceneVisualTarget::Picture(command), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "at requires an uncommitted picture or picture transform",
            ));
        };
        match command {
            PictureCommand::Show { position, .. } => *position = [x as f32, y as f32],
            PictureCommand::Transform { position, .. } => {
                *position = [Some(x as f32), Some(y as f32)]
            }
            _ => {
                return Err(NativeError::message(
                    "at requires a picture show or transform",
                ));
            }
        }
        Ok(handle)
    }

    pub(super) fn screen_space(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                screen_space, view, ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "screenSpace requires an uncommitted picture",
            ));
        };
        if *view == crate::scene::pictures::PictureView::Background {
            return Err(NativeError::message(
                "background pictures cannot use screenSpace",
            ));
        }
        *screen_space = true;
        Ok(handle)
    }

    pub(super) fn frame(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        x: f64,
        y: f64,
        scale: f64,
        rotation: f64,
        layer: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if ![x, y, scale, rotation, layer]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 100000.0)
            || scale <= 0.0
            || !(0.0..30.0).contains(&layer)
        {
            return Err(NativeError::message("invalid picture frame"));
        }
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                position,
                scale: size,
                rotation: angle,
                layer: z,
                ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&id)
        else {
            return Err(NativeError::message(
                "frame requires an uncommitted picture",
            ));
        };
        *position = [x as f32, y as f32];
        *size = scale as f32;
        *angle = rotation as f32;
        *z = layer as f32;
        Ok(SceneTransitionHandle(id))
    }

    /// Presentation order independent of position, size and rotation.
    pub(super) fn picture_layer(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !value.is_finite() || !(0.0..30.0).contains(&value) {
            return Err(NativeError::message(
                "picture layer must be finite and in [0, 30)",
            ));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { layer, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "layer requires an uncommitted picture",
            ));
        };
        *layer = value as f32;
        Ok(handle)
    }

    pub(super) fn picture_replace(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { replace, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "replace requires an uncommitted picture",
            ));
        };
        *replace = true;
        Ok(handle)
    }

    pub(super) fn picture_size(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        width: f64,
        height: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if ![width, height]
            .iter()
            .all(|n| n.is_finite() && *n > 0.0 && *n <= 100000.0)
        {
            return Err(NativeError::message(
                "picture dimensions must be finite and in (0, 100000]",
            ));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { size, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("size requires an uncommitted picture"));
        };
        *size = Some([width as f32, height as f32]);
        Ok(handle)
    }

    pub(super) fn picture_blur(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        radius: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !radius.is_finite() || !(0.0..=128.0).contains(&radius) {
            return Err(NativeError::message(
                "blur radius must be in 0..=128 pixels",
            ));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { post_process, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("blur requires an uncommitted picture"));
        };
        post_process.get_or_insert_default().blur_radius = radius as f32;
        Ok(handle)
    }

    pub(super) fn picture_grayscale_gamma(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        red: f64,
        green: f64,
        blue: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if [red, green, blue]
            .into_iter()
            .any(|value| !value.is_finite() || !(0.0..=16.0).contains(&value) || value == 0.0)
        {
            return Err(NativeError::message("grayscale gamma must be in (0, 16]"));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { post_process, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "grayscaleGamma requires an uncommitted picture",
            ));
        };
        post_process.get_or_insert_default().grayscale_gamma =
            bevy::prelude::Vec4::new(red as f32, green as f32, blue as f32, 1.0);
        Ok(handle)
    }

    pub(super) fn picture_slice(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        left: f64,
        top: f64,
        right: f64,
        bottom: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let borders = [left, top, right, bottom];
        if !borders
            .iter()
            .all(|n| n.is_finite() && *n >= 0.0 && *n <= 100000.0)
        {
            return Err(NativeError::message(
                "slice borders must be finite and in [0, 100000]",
            ));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { slice, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "slice requires an uncommitted picture",
            ));
        };
        *slice = Some(borders.map(|n| n as f32));
        Ok(handle)
    }

    pub(super) fn picture_tint(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        red: i32,
        green: i32,
        blue: i32,
        alpha: i32,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let channels = [red, green, blue, alpha];
        if !channels.iter().all(|n| (0..=255).contains(n)) {
            return Err(NativeError::message("picture tint requires RGBA bytes"));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { color, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("tint requires an uncommitted picture"));
        };
        *color = Some(channels.map(|n| n as f32 / 255.0));
        Ok(handle)
    }

    /// Reveal an incoming picture through a full-canvas rule texture.
    pub(super) fn picture_dissolve(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        texture: String,
        softness: Option<f64>,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let softness = softness.unwrap_or(0.0);
        if texture.trim().is_empty() || !softness.is_finite() || !(0.0..=1.0).contains(&softness) {
            return Err(NativeError::message(
                "picture dissolve needs a texture and softness between 0 and 1",
            ));
        }
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { dissolve, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "dissolve requires an uncommitted scene.picture(...) or bg(...) builder",
            ));
        };
        *dissolve = Some(crate::scene::pictures::PictureDissolve {
            path: texture,
            softness: softness as f32,
        });
        Ok(handle)
    }

    /// Change only a shown picture's tint; channels use sRGB bytes.
    #[hks(name = "tintPicture", selector = "scene")]
    fn tint_picture(
        context: &mut CharacterContext,
        id: String,
        r: i32,
        g: i32,
        b: i32,
        a: i32,
    ) -> Result<FadeTransitionHandle, NativeError> {
        let bytes = [r, g, b, a];
        if id.trim().is_empty() || !bytes.iter().all(|v| (0..=255).contains(v)) {
            return Err(NativeError::message(
                "tintPicture requires an identity and RGBA channels in 0..=255",
            ));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Tint {
                id,
                color: bytes.map(|v| v as f32 / 255.0),
                seconds: 0.0,
            }))
    }

    /// A frame-stepped monochrome raster. Uses story time, independent of wall
    /// time; pausing a story also pauses the noise. Dimensions are sample cells.
    #[hks(name = "noisePicture", selector = "scene")]
    fn noise_picture(
        context: &mut CharacterContext,
        id: String,
        width: i32,
        height: i32,
        interval_ms: f64,
    ) -> Result<(), NativeError> {
        if id.trim().is_empty()
            || !(1..=16384).contains(&width)
            || !(1..=16384).contains(&height)
            || !interval_ms.is_finite()
            || interval_ms <= 0.0
        {
            return Err(NativeError::message(
                "noisePicture requires an identity, grid in 1..=16384 and positive finite interval",
            ));
        }
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::Noise {
                id,
                grid: [width as u32, height as u32],
                interval: interval_ms / 1000.0,
            }));
        Ok(())
    }

    /// Radius is in source-image pixels, not a global camera blur amount.
    #[hks(name = "blurPicture", selector = "scene")]
    fn blur_picture(
        context: &mut CharacterContext,
        id: String,
        radius: f64,
    ) -> Result<FadeTransitionHandle, NativeError> {
        if id.trim().is_empty() || !radius.is_finite() || !(0.0..=128.0).contains(&radius) {
            return Err(NativeError::message(
                "blurPicture requires an identity and radius in 0..=128 pixels",
            ));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Blur {
                id,
                radius: radius as f32,
                seconds: 0.0,
            }))
    }

    #[hks(name = "hidePicture", selector = "scene")]
    fn hide_picture(
        context: &mut CharacterContext,
        id: String,
    ) -> Result<PictureHideHandle, NativeError> {
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Hide {
                id,
                seconds: 0.0,
            }))
    }

    #[hks(name = "movePicture", selector = "scene")]
    fn move_picture(
        context: &mut CharacterContext,
        id: String,
        x: f64,
        y: f64,
        seconds: f64,
        ease: String,
    ) -> Result<PictureTransformHandle, NativeError> {
        milliseconds(seconds)?;
        if ![x, y].iter().all(|n| n.is_finite() && n.abs() <= 100000.0)
            || !["linear", "smoothStep", "easeOutQuad", "easeOutBack"].contains(&ease.as_str())
        {
            return Err(NativeError::message("invalid picture movement or easing"));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Transform {
                id,
                position: [Some(x as f32), Some(y as f32)],
                scale: None,
                rotation: None,
                seconds: seconds as f32,
                ease: crate::script::animation::Easing::named(&ease)?,
            }))
    }

    /// Edit the displayed pose without showing/replacing the texture. Missing
    /// fields are resolved from the current pose when ECS applies the command.
    #[hks(name = "transformPicture", selector = "scene")]
    fn transform_picture(
        context: &mut CharacterContext,
        id: String,
    ) -> Result<PictureTransformHandle, NativeError> {
        if id.trim().is_empty() {
            return Err(NativeError::message("picture identity must not be empty"));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Transform {
                id,
                position: [None; 2],
                scale: None,
                rotation: None,
                seconds: 0.0,
                ease: "linear".into(),
            }))
    }

    fn transform_field(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        field: usize,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !value.is_finite() || value.abs() > 100000.0 || (field == 2 && value <= 0.0) {
            return Err(NativeError::message(
                "picture transform requires finite coordinates and positive scale",
            ));
        }
        if let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                position,
                scale,
                rotation,
                ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        {
            match field {
                0 | 1 => position[field] = value as f32,
                2 => *scale = value as f32,
                _ => *rotation = value as f32,
            }
            return Ok(handle);
        }
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Transform {
                position,
                scale,
                rotation,
                ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "transform property requires an uncommitted picture transform",
            ));
        };
        match field {
            0 | 1 => position[field] = Some(value as f32),
            2 => *scale = Some(value as f32),
            _ => *rotation = Some(value as f32),
        }
        Ok(handle)
    }

    /// Virtual-canvas percentages; offscreen and fractional positions allowed.
    pub(super) fn transform_x(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 0, value)
    }

    pub(super) fn transform_y(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 1, value)
    }

    pub(super) fn transform_scale(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 2, value)
    }

    pub(super) fn transform_rotation(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 3, value)
    }

    fn transition_spec(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
    ) -> Result<crate::script::animation::AnimationSpec, NativeError> {
        use crate::{script::animation::AnimationSpec, stage::runtime::StageCommand};
        match context.scene_visuals.pending.get(&handle.0) {
            Some((
                SceneVisualTarget::Spatial(
                    StageCommand::Camera { animation, .. } | StageCommand::View { animation, .. },
                ),
                _,
            )) => Ok(*animation),
            Some((
                SceneVisualTarget::Picture(
                    PictureCommand::Transform { seconds, ease, .. }
                    | PictureCommand::Exit { seconds, ease, .. },
                ),
                _,
            )) => Ok(AnimationSpec::new(*seconds as f64, *ease, false)),
            _ => Err(NativeError::message(
                "time/easing requires an uncommitted scene transition",
            )),
        }
    }
    fn set_transition_spec(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        spec: crate::script::animation::AnimationSpec,
    ) -> Result<SceneTransitionHandle, NativeError> {
        use crate::stage::runtime::StageCommand;
        match context.scene_visuals.pending.get_mut(&handle.0) {
            Some((
                SceneVisualTarget::Spatial(
                    StageCommand::Camera { animation, .. } | StageCommand::View { animation, .. },
                ),
                _,
            )) => *animation = spec,
            Some((
                SceneVisualTarget::Picture(
                    PictureCommand::Transform { seconds, ease, .. }
                    | PictureCommand::Exit { seconds, ease, .. },
                ),
                _,
            )) => {
                *seconds = spec.duration();
                *ease = spec.easing();
            }
            _ => {
                return Err(NativeError::message(
                    "time/easing requires an uncommitted scene transition",
                ));
            }
        }
        Ok(handle)
    }
    pub(super) fn transition_time(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        seconds: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let duration = crate::script::animation::duration_millis(seconds)?;
        if let Some((target, fade)) = context.scene_visuals.pending.get_mut(&handle.0) {
            if matches!(
                target,
                SceneVisualTarget::Picture(
                    PictureCommand::Show { .. }
                        | PictureCommand::Hide { .. }
                        | PictureCommand::Tint { .. }
                        | PictureCommand::Blur { .. }
                        | PictureCommand::Oscillate { .. }
                ) | SceneVisualTarget::HideCharacters { .. }
                    | SceneVisualTarget::Shake { .. }
                    | SceneVisualTarget::Curtain { .. }
            ) {
                *fade = Some(duration);
                return Ok(handle);
            }
        }
        let spec = transition_spec(context, handle)?.with_time(seconds)?;
        set_transition_spec(context, handle, spec)
    }
    pub(super) fn transition_easing(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        easing: crate::script::animation::Easing,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let spec = transition_spec(context, handle)?.with_easing(easing)?;
        set_transition_spec(context, handle, spec)
    }

    /// One owned exit: final position and alpha finish before removal.
    pub(super) fn exit_to(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        x: f64,
        y: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !x.is_finite() || !y.is_finite() || x.abs() > 100000.0 || y.abs() > 100000.0 {
            return Err(NativeError::message("invalid picture exit position"));
        }
        let Some((SceneVisualTarget::Picture(command), duration_override)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "to requires an uncommitted picture exit",
            ));
        };
        let PictureCommand::Hide { id, seconds } = command else {
            return Err(NativeError::message("to requires hidePicture"));
        };
        // Hide stores visibility timing as an override; Exit owns one motion
        // and fade timeline. Transfer that value once rather than leaving a
        // stale override that can hide or replace a later `.time(...)`.
        let seconds = duration_override
            .take()
            .map(|ms| ms as f32 / 1000.0)
            .unwrap_or(*seconds);
        *command = PictureCommand::Exit {
            id: id.clone(),
            position: [x as f32, y as f32],
            seconds,
            ease: "linear".into(),
        };
        Ok(handle)
    }

    #[hks(name = "clearPictures", selector = "scene")]
    fn clear_pictures(context: &mut CharacterContext) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::Clear));
        Ok(())
    }

    /// Freeze placement at its displayed value; tint, blur and fade continue.
    #[hks(name = "stopPictureMotion", selector = "scene")]
    fn stop_picture_motion(context: &mut CharacterContext, id: String) -> Result<(), NativeError> {
        context
            .commands
            .push(StoryEffect::Picture(PictureCommand::StopMotion { id }));
        Ok(())
    }

    /// Amplitudes are canvas percentages, periods and duration are seconds.
    #[hks(name = "oscillatePicture", selector = "scene")]
    fn oscillate_picture(
        context: &mut CharacterContext,
        id: String,
        x: f64,
        y: f64,
        period_x: f64,
        period_y: f64,
    ) -> Result<TimedSceneTransitionHandle, NativeError> {
        if ![x, y].iter().all(|v| v.is_finite() && v.abs() < 100000.0)
            || ![period_x, period_y]
                .iter()
                .all(|v| (0.001..=3600.0).contains(v))
        {
            return Err(NativeError::message(
                "invalid picture oscillation amplitude or period",
            ));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::Oscillate {
                id,
                amplitude: [x as f32, y as f32],
                period: [period_x as f32, period_y as f32],
                seconds: 0.0,
            }))
    }

    #[hks(name = "animatePictureX", selector = "scene")]
    fn animate_picture_x(
        context: &mut CharacterContext,
        id: String,
        offsets: Vec<f64>,
        step_seconds: f64,
    ) -> Result<SceneEffectHandle, NativeError> {
        if offsets.is_empty()
            || offsets.len() > 4096
            || !offsets.iter().all(|n| n.is_finite() && n.abs() < 100000.0)
            || !(0.001..=60.0).contains(&step_seconds)
        {
            return Err(NativeError::message("invalid picture keyframes"));
        }
        context
            .scene_visuals
            .begin_as(SceneVisualTarget::Picture(PictureCommand::AnimateX {
                id,
                offsets: offsets.into_iter().map(|n| n as f32).collect(),
                step_seconds: step_seconds as f32,
            }))
    }

    #[hks(name = "bg")]
    fn background(
        context: &mut CharacterContext,
        texture: String,
    ) -> Result<PictureShowHandle, NativeError> {
        let picture = picture(context, "Backgrounds".into(), texture)?;
        picture_view(
            context,
            SceneTransitionHandle(picture.0),
            CameraScope::Background,
        )?;
        Ok(picture)
    }

    pub(super) fn picture_view(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        scope: CameraScope,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let next = match scope {
            CameraScope::Background => crate::scene::pictures::PictureView::Background,
            CameraScope::Scene => crate::scene::pictures::PictureView::Scene,
            _ => {
                return Err(NativeError::message(
                    "picture view must be background or scene; UI belongs to the UI tree",
                ));
            }
        };
        let Some((
            SceneVisualTarget::Picture(PictureCommand::Show {
                view, screen_space, ..
            }),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message("view requires a picture show builder"));
        };
        if *screen_space && next == crate::scene::pictures::PictureView::Background {
            return Err(NativeError::message(
                "background pictures cannot use screenSpace",
            ));
        }
        *view = next;
        Ok(handle)
    }

    #[hks(name = "cg")]
    fn cg(
        context: &mut CharacterContext,
        texture: String,
    ) -> Result<PictureShowHandle, NativeError> {
        picture(context, "Stills".into(), texture)
    }

    /// A scene-space blackout, below script-owned UI. Independent of background
    /// identity, so replacing a background does not reveal it before the fade.
    #[hks(name = "curtain", selector = "scene")]
    fn curtain(
        context: &mut CharacterContext,
        opacity: f64,
    ) -> Result<CurtainTransitionHandle, NativeError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(NativeError::message(
                "curtain opacity must be between 0 and 1",
            ));
        }
        context.scene_visuals.begin_as(SceneVisualTarget::Curtain {
            color: [0; 3],
            opacity: opacity as f32,
            mask: None,
            softness: 0.0,
        })
    }

    /// Select the curtain pigment; fade controls its opacity independently.
    pub(super) fn color(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        red: i32,
        green: i32,
        blue: i32,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if ![red, green, blue]
            .iter()
            .all(|value| (0..=255).contains(value))
        {
            return Err(NativeError::message(
                "curtain color requires RGB bytes in 0..=255",
            ));
        }
        let Some((SceneVisualTarget::Curtain { color, .. }, _)) =
            context.scene_visuals.pending.get_mut(&id)
        else {
            return Err(NativeError::message(
                "color requires an uncommitted scene.curtain(...)",
            ));
        };
        *color = [red as u8, green as u8, blue as u8];
        Ok(SceneTransitionHandle(id))
    }

    /// A red-channel threshold mask, sampled as linear data across the canvas.
    pub(super) fn dissolve(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        texture: String,
        softness: Option<f64>,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let softness = softness.unwrap_or(0.0);
        if texture.trim().is_empty() || !(0.0..=1.0).contains(&softness) {
            return Err(NativeError::message(
                "dissolve needs a texture and softness between 0 and 1",
            ));
        }
        let Some((
            SceneVisualTarget::Curtain {
                mask,
                softness: edge,
                ..
            },
            _,
        )) = context.scene_visuals.pending.get_mut(&id)
        else {
            return Err(NativeError::message(
                "dissolve requires an uncommitted scene.curtain(...)",
            ));
        };
        *mask = Some(texture);
        *edge = softness as f32;
        Ok(SceneTransitionHandle(id))
    }

    /// Join this statement's scene transition through the shared effect protocol.
    pub(super) fn await_transition(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
    ) -> Result<(), NativeError> {
        if !context.scene_visuals.pending.contains_key(&id) {
            return Err(NativeError::message(
                "await requires a scene transition in the same statement",
            ));
        }
        context.await_effects = true;
        Ok(())
    }

    /// Milliseconds, matching Bgm.fadeIn. The builder commits at statement end.
    pub(super) fn fade_in(
        context: &mut CharacterContext,
        SceneTransitionHandle(id): SceneTransitionHandle,
        duration_ms: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let duration = milliseconds(duration_ms / 1000.0)?;
        let pending = context.scene_visuals.pending.get_mut(&id).ok_or_else(|| {
            NativeError::message(
                "scene transition has already been committed; start a new presentation statement",
            )
        })?;
        if !matches!(
            &pending.0,
            SceneVisualTarget::Picture(
                PictureCommand::Show { .. }
                    | PictureCommand::Hide { .. }
                    | PictureCommand::Exit { .. }
                    | PictureCommand::Tint { .. }
                    | PictureCommand::Blur { .. }
            ) | SceneVisualTarget::HideCharacters { .. }
                | SceneVisualTarget::Curtain { .. }
        ) {
            return Err(NativeError::message(
                "fade is not supported by this operation; use time(seconds) for movement or camera transitions",
            ));
        }
        if let SceneVisualTarget::Picture(PictureCommand::Exit { seconds, .. }) = &mut pending.0 {
            *seconds = duration as f32 / 1000.0;
        } else {
            pending.1 = Some(duration);
        }
        Ok(SceneTransitionHandle(id))
    }

    /// Seconds. Interactive stories wait; seq waits in order; par schedules the
    /// timer without suspending its command list, and wait(handle) joins it.
    #[hks(name = "sleep")]
    fn sleep(context: &mut CharacterContext, seconds: f64) -> Result<(), NativeError> {
        context.commands.push(StoryEffect::Delay {
            duration_ms: milliseconds(seconds)?,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picture_dissolve_is_typed_and_retains_the_rule_in_its_effect() {
        use super::*;
        use crate::script::{StoryRuntime, StoryRuntimeEvent};
        let code = compile_story_bytecode(
            "test.hks",
            "scene.picture(\"wipe\", \"image/room\").dissolve(\"rule/door\", 0.25).time(1).await()",
        )
        .expect("picture dissolve compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::TaskEffect {
            effect:
                StoryEffect::Picture(PictureCommand::Show {
                    dissolve: Some(crate::scene::pictures::PictureDissolve { path, softness }),
                    seconds,
                    ..
                }),
            ..
        }) = runtime.step().expect("submit masked picture")
        else {
            panic!("expected awaitable picture dissolve")
        };
        assert_eq!(path, "rule/door");
        assert_eq!(softness, 0.25);
        assert_eq!(seconds, 1.0);
    }

    #[test]
    fn named_video_uses_picture_transitions_and_retains_loop_configuration() {
        use super::*;
        use crate::script::{StoryRuntime, StoryRuntimeEvent};
        let code = compile_story_bytecode("video.hks", r#"
            scene.video("water", "movies/water").looping(true).at(.center).size(640, 360).layer(2).time(0.5)
            scene.hidePicture("water").time(0.25)
        "#).expect("video builder compiles");
        let mut runtime = StoryRuntime::new(code).expect("runtime");
        let Some(StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
            video: Some(video),
            path,
            seconds,
            ..
        }))) = runtime.step().expect("video show")
        else {
            panic!("expected video picture")
        };
        assert!(video.looping);
        assert_eq!(path, "movies/water");
        assert_eq!(seconds, 0.5);
        assert!(matches!(
            runtime.step().expect("hide"),
            Some(StoryRuntimeEvent::Effect(StoryEffect::Picture(
                PictureCommand::Hide { seconds: 0.25, .. }
            )))
        ));
    }
    use super::*;

    #[test]
    fn scene_time_uses_the_common_duration_contract() {
        let mut picture = runtime("bg(\"room\").time(0.0006)");
        let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
            seconds, ..
        })) = event(&mut picture)
        else {
            panic!("picture show");
        };
        assert!((seconds - 0.001).abs() < 0.000001);
        for builder in [
            "bg(\"room\")",
            "scene.hidePicture(\"panel\")",
            "scene.curtain(1)",
            "scene.hideCharacters()",
            "scene.shake(1, 1, 0.05)",
        ] {
            let mut invalid = runtime(&format!("{builder}.time(3600.1)"));
            let error = invalid.step().expect_err("out-of-range duration");
            assert!(
                error.to_string().contains("0..=3600 seconds"),
                "{builder}: {error}"
            );
        }
    }

    #[test]
    fn stepped_shake_uses_the_normal_animation_wait_path() {
        let mut script = runtime("scene.shake(15, 15, 0.05).time(0.4)");
        assert!(matches!(
            event(&mut script),
            StoryRuntimeEvent::Effect(StoryEffect::ShakeCamera {
                amplitude: [15.0, 15.0],
                duration_ms: 400,
                ..
            })
        ));
        let mut script = runtime("scene.shake(15, 15, 0.05).time(0.4).await()");
        assert!(matches!(
            event(&mut script),
            StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::ShakeCamera {
                    duration_ms: 400,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn picture_oscillation_is_timed_and_awaitable() {
        let mut script =
            runtime("scene.oscillatePicture(\"panel\", 1, 2, 0.1, 0.2).time(0.25).await()");
        assert!(matches!(
            event(&mut script),
            StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::Picture(PictureCommand::Oscillate {
                    amplitude: [1.0, 2.0],
                    seconds: 0.25,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn time_applies_to_picture_visibility_and_preserves_modifier_order() {
        for source in [
            "bg(\"alice/background\").time(0.3).scale(1.2)",
            "bg(\"alice/background\").scale(1.2).time(0.3)",
        ] {
            let mut runtime = runtime(source);
            let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                seconds,
                scale,
                ..
            })) = event(&mut runtime)
            else {
                panic!("picture show");
            };
            assert!((seconds - 0.3).abs() < 0.00001);
            assert!((scale - 1.2).abs() < 0.00001);
        }
        for suffix in ["time(0)", "fade(0)"] {
            let mut runtime = runtime(&format!("scene.hideCharacters(300).{suffix}"));
            assert!(matches!(
                event(&mut runtime),
                StoryRuntimeEvent::Effect(StoryEffect::HideCharacter { fade_ms: 0, .. })
            ));
        }
    }

    #[test]
    fn unsupported_modifiers_fail_before_execution() {
        for (source, method) in [
            ("scene.transformPicture(\"alice\").fade(300)", "fade"),
            ("scene.transformPicture(\"alice\").size(40, 50)", "size"),
            ("scene.hidePicture(\"panel\").at(.center)", "at"),
            ("scene.hidePicture(\"panel\").easing(.easeOut)", "easing"),
            ("bg(\"room\").easing(.easeOut)", "easing"),
            ("bg(\"room\").color(255, 255, 255)", "color"),
            ("bg(\"room\").to(50, 50)", "to"),
            ("scene.curtain(1).size(40, 50)", "size"),
            ("scene.hideCharacters().at(.center)", "at"),
            ("scene.shake(1, 1, 0.05).fade(300)", "fade"),
            (
                "scene.animatePictureX(\"panel\", [0.0, 2.0], 0.1).time(1)",
                "time",
            ),
            (
                "stage.open(\"room.stage.hson\").camera(\"front\").size(40, 50)",
                "size",
            ),
            (
                "stage.open(\"room.stage.hson\").showView(\"side\").track(\"front\")",
                "track",
            ),
        ] {
            let error = compile_story_bytecode("builder.hks", source)
                .expect_err("unsupported modifiers must be compile errors");
            assert!(error.to_string().contains(method), "{source}: {error}");
        }
    }

    #[test]
    fn scene_builder_handles_are_distinct_in_native_values() {
        use hiraku_script::native::{FromHksValue, IntoHksValue};
        let show = PictureShowHandle::from(SceneTransitionHandle(1)).into_hks_value();
        let exit = PictureExitHandle::from(SceneTransitionHandle(1)).into_hks_value();
        assert!(PictureShowHandle::from_hks_value(&show).is_ok());
        assert!(PictureExitHandle::from_hks_value(&show).is_err());
        assert!(PictureShowHandle::from_hks_value(&exit).is_err());
    }

    #[test]
    fn every_virtual_camera_accepts_typed_canvas_positions() {
        for (name, scope) in [
            ("background", crate::script::CameraEffectScope::Background),
            ("scene", crate::script::CameraEffectScope::World),
            ("ui", crate::script::CameraEffectScope::Ui),
            ("canvas", crate::script::CameraEffectScope::Canvas),
        ] {
            let mut story = runtime(&format!("camera(.{name}).at(.right).zoom(1.5).time(0.25)"));
            let StoryRuntimeEvent::Effect(StoryEffect::SetCamera {
                anchor,
                offset,
                scope: actual,
                duration_ms,
                ..
            }) = event(&mut story)
            else {
                panic!("camera position effect");
            };
            assert_eq!(anchor, Some([75.0, 50.0]));
            assert_eq!(offset, None);
            assert_eq!(actual, scope);
            assert_eq!(duration_ms, 250);
        }
        for (position, anchor, offset) in [
            (".center", Some([50.0, 50.0]), None),
            (".rel(10, 90)", Some([10.0, 90.0]), None),
            (".pos(10, 90)", None, Some([10.0, 90.0, 0.0])),
        ] {
            let mut story = runtime(&format!("camera(.background).at({position})"));
            let StoryRuntimeEvent::Effect(StoryEffect::SetCamera {
                anchor: actual_anchor,
                offset: actual_offset,
                ..
            }) = event(&mut story)
            else {
                panic!("camera position effect");
            };
            assert_eq!(actual_anchor, anchor);
            assert_eq!(actual_offset, offset);
        }
    }

    #[test]
    fn actor_and_camera_use_typed_bezier_and_invalid_controls_report_errors() {
        use crate::script::animation::Easing;
        let curve = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
        let mut camera =
            runtime("camera().zoom(1.2).easing(.cubicBezier(0.25,0.1,0.25,1)).time(0.8)");
        let StoryRuntimeEvent::Effect(StoryEffect::SetCamera {
            ease, duration_ms, ..
        }) = event(&mut camera)
        else {
            panic!("camera")
        };
        assert_eq!(ease, curve);
        assert_eq!(duration_ms, 800);
        let mut actor = runtime(
            "char(\"alice\").at(.right).show().easing(.cubicBezier(0.25,0.1,0.25,1)).time(0.8)",
        );
        let StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter {
            placement_animation: Some(spec),
            ..
        }) = event(&mut actor)
        else {
            panic!("actor")
        };
        assert_eq!(spec.easing(), curve);
        assert!((spec.duration() - 0.8).abs() < 0.00001);
        for source in [
            "camera().zoom(1.2).easing(.cubicBezier(-1,0,1,1))",
            "char(\"alice\").show().time(-1)",
        ] {
            let mut invalid = runtime(source);
            let mut error = None;
            for _ in 0..32 {
                if let Err(e) = invalid.step() {
                    error = Some(e);
                    break;
                }
            }
            assert!(error.is_some(), "invalid animation must fail: {source}");
        }
    }

    #[test]
    fn bezier_picture_parameters_commit_in_either_order_and_await() {
        use crate::script::animation::Easing;
        for modifiers in [
            ".time(0.8).easing(.cubicBezier(0.25, 0.1, 0.25, 1))",
            ".easing(.cubicBezier(0.25, 0.1, 0.25, 1)).time(0.8)",
        ] {
            let mut runtime = runtime(&format!(
                "scene.transformPicture(\"panel\").scale(2){modifiers}.await()\nlog(\"done\")"
            ));
            let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
                panic!("expected awaited transition")
            };
            let StoryEffect::Picture(PictureCommand::Transform { seconds, ease, .. }) = &effect
            else {
                panic!("expected transform")
            };
            assert!((*seconds - 0.8).abs() < 0.00001);
            assert_eq!(*ease, Easing::CubicBezier(0.25, 0.1, 0.25, 1.0));
            assert!(runtime.step().expect("wait").is_none());
            runtime
                .complete_task_effect(task, &effect)
                .expect("complete");
            assert!(
                matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text=="done")
            );
        }
    }

    #[test]
    fn screen_space_and_atomic_exit_are_registered_native_builders() {
        let mut show = runtime("scene.picture(\"panel\", \"image/panel\").screenSpace()");
        assert!(matches!(
            event(&mut show),
            StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                dissolve: None,
                video: None,
                screen_space: true,
                ..
            }))
        ));
        let mut exit =
            runtime("scene.hidePicture(\"panel\").to(20, 30).time(0.4).easing(.easeIn).await()");
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut exit) else {
            panic!("exit must expose one awaitable effect");
        };
        assert!(
            matches!(&effect, StoryEffect::Picture(PictureCommand::Exit { position: [20.0, 30.0], seconds, ease, .. })
            if (*seconds - 0.4).abs() < 0.0001 && *ease == crate::script::animation::Easing::EaseIn)
        );
        assert!(exit.step().expect("wait for owned exit").is_none());
        exit.complete_task_effect(task, &effect)
            .expect("complete exit");
    }

    #[test]
    fn picture_exit_duration_is_independent_of_modifier_order() {
        for (modifiers, expected) in [
            (".time(0.4).to(20, 30)", 0.4),
            (".to(20, 30).time(0.4)", 0.4),
            (".fade(400).to(20, 30)", 0.4),
            (".to(20, 30).fade(400)", 0.4),
            (".time(0.4).to(20, 30).time(0.8)", 0.8),
            (".time(0.4).to(20, 30).time(0)", 0.0),
            (".time(0).to(20, 30)", 0.0),
        ] {
            let mut script = runtime(&format!(
                "scene.hidePicture(\"panel\"){modifiers}.easing(.easeIn).await()"
            ));
            let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut script) else {
                panic!("exit must be awaitable: {modifiers}");
            };
            let StoryEffect::Picture(PictureCommand::Exit { seconds, ease, .. }) = &effect else {
                panic!("expected picture exit: {modifiers}");
            };
            assert!(
                (*seconds - expected).abs() < 0.0001,
                "{modifiers}: {seconds}"
            );
            assert_eq!(*ease, crate::script::animation::Easing::EaseIn);
            assert!(script.step().expect("waiting").is_none());
            script
                .complete_task_effect(task, &effect)
                .expect("complete exit");
        }
    }
    use crate::script::capabilities::{StoryWait, compile_story_bytecode};
    use crate::script::{StoryRuntime, StoryRuntimeEvent};
    use hiraku_script::Value;

    fn runtime(source: &str) -> StoryRuntime {
        StoryRuntime::new(compile_story_bytecode("test.hks", source).expect("script compiles"))
            .expect("runtime initializes")
    }

    #[test]
    fn sibling_motion_group_follows_entrance_group() {
        let mut runtime = runtime(
            r#"
            let entrance = par {
                scene.picture("room", "room").fade(500)
                char("alice").at(.rel(50, 50)).show()
            }
            par {
                scene.transformPicture("room").at(.right).time(2).easing(.linear)
                char("alice").at(.rel(75, 50)).time(0.5).easing(.easeOut)
            }
            entrance.await()
        "#,
        );
        let mut order = Vec::new();
        for _ in 0..64 {
            if let Some(StoryRuntimeEvent::TaskEffect { effect, .. }) =
                runtime.step().expect("sibling groups execute")
            {
                match &effect {
                    StoryEffect::Picture(PictureCommand::Show { .. }) => order.push("picture show"),
                    StoryEffect::Picture(PictureCommand::Transform { .. }) => {
                        order.push("picture move")
                    }
                    StoryEffect::ShowCharacter {
                        placement_animation,
                        ..
                    } => order.push(if placement_animation.is_some() {
                        "actor move"
                    } else {
                        "actor show"
                    }),
                    _ => panic!("unexpected effect: {effect:?}"),
                }
                // Keep all effects in flight: movement must start after the
                // show commands, not after their fades have completed.
            }
            if order.len() == 4 {
                break;
            }
        }
        assert_eq!(
            order,
            ["picture show", "actor show", "picture move", "actor move"]
        );
    }

    fn event(runtime: &mut StoryRuntime) -> StoryRuntimeEvent {
        for _ in 0..100 {
            if let Some(event) = runtime.step().expect("runtime steps") {
                return event;
            }
        }
        panic!("expected a runtime event");
    }

    #[test]
    fn picture_convenience_positions_and_fractional_motion() {
        assert_eq!(Position::Left.resolve(), [-600.0, -200.0]);
        assert_eq!(Position::Center.resolve(), [0.0, -200.0]);
        assert_eq!(Position::Right.resolve(), [600.0, -200.0]);
        assert_eq!(Position::Relative(50.0, 50.0).resolve(), [0.0, 0.0]);
        for (position, expected) in [
            (".left", [25.0, 50.0]),
            (".center", [50.0, 50.0]),
            (".right", [75.0, 50.0]),
            (".rel(-12.5, 50.078125)", [-12.5, 50.078125]),
        ] {
            let mut show = runtime(&format!(
                "scene.picture(\"alice\", \"portrait\").at({position})"
            ));
            let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                position,
                ..
            })) = event(&mut show)
            else {
                panic!("expected positioned picture");
            };
            assert_eq!(position, expected);
        }
        let mut movement = runtime("scene.transformPicture(\"alice\").at(.rel(37.5, 62.5))");
        let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Transform {
            position,
            ..
        })) = event(&mut movement)
        else {
            panic!("expected positioned transform");
        };
        assert_eq!(position, [Some(37.5), Some(62.5)]);
    }

    #[test]
    fn validates_duration_without_panicking() {
        assert_eq!(milliseconds(1.8).expect("valid duration"), 1800);
        for value in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(milliseconds(value).is_err());
        }
    }

    #[test]
    fn sliced_picture_commits_style_and_joins_its_fade() {
        let mut runtime = runtime(
            r#"
            scene.picture("panel", "textures/border").size(640, 320)
                .slice(20, 30, 20, 30).tint(0, 0, 0, 204).fade(300).await()
            log("finished")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("expected styled picture");
        };
        let StoryEffect::Picture(PictureCommand::Show {
            size,
            slice,
            color,
            seconds,
            ..
        }) = &effect
        else {
            panic!("expected picture effect");
        };
        assert_eq!(*size, Some([640.0, 320.0]));
        assert_eq!(*slice, Some([20.0, 30.0, 20.0, 30.0]));
        assert_eq!(*color, Some([0.0, 0.0, 0.0, 0.8]));
        assert!((seconds - 0.3).abs() < 0.0001);
        assert!(runtime.step().expect("waiting for fade").is_none());
        runtime
            .complete_task_effect(task, &effect)
            .expect("fade completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(message)) if message == "finished")
        );
    }

    #[test]
    fn picture_entrance_rotation_and_smoothstep_are_native_builder_properties() {
        let mut runtime = runtime(
            r#"
            scene.picture("panel", "alice/portrait").at(.center).rotation(25).fade(1000)
            scene.transformPicture("panel").at(.left).time(0.4).easing(.smoothStep)
        "#,
        );
        let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
            rotation, ..
        })) = event(&mut runtime)
        else {
            panic!("expected rotated entrance");
        };
        assert_eq!(rotation, 25.0);
        let StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Transform {
            ease, ..
        })) = event(&mut runtime)
        else {
            panic!("expected smoothstep motion");
        };
        assert_eq!(ease, crate::script::animation::Easing::SmoothStep);
    }

    #[test]
    fn picture_alpha_clip_builder_commits_texture_and_world_bounds() {
        let mut runtime = runtime(
            r#"
            scene.clipRect("split", 800, 600).mask("masks/split").at(.pos(20, -10))
        "#,
        );
        let StoryRuntimeEvent::Effect(StoryEffect::Clip(ClipCommand::Define { region, .. })) =
            event(&mut runtime)
        else {
            panic!("expected clip definition");
        };
        assert_eq!(region.mask.as_deref(), Some("masks/split"));
        assert_eq!(region.center, [20.0, -10.0]);
        assert_eq!(region.size, [800.0, 600.0]);
    }

    #[test]
    fn named_clip_builders_commit_in_order_and_share_actor_picture_coordinates() {
        let mut runtime = runtime(
            r#"
            scene.clipRect("window", 400, 800).at(.pos(100, 20)).rotation(-10)
            scene.clipPicture("room", "window")
            char("alice").clip("window")
            scene.removeClip("window")
        "#,
        );
        let mut state = crate::scene::clipping::ClipState::default();
        for index in 0..4 {
            let StoryRuntimeEvent::Effect(StoryEffect::Clip(command)) = event(&mut runtime) else {
                panic!("expected clip event");
            };
            state.apply(command).expect("valid command");
            if index == 2 {
                assert!(state.actor("alice").is_some());
                assert_eq!(state.actor("alice"), state.picture("room"));
            }
        }
        assert!(state.actor("alice").is_none());
        assert!(state.picture("room").is_none());
    }

    #[test]
    fn sequence_removes_clip_only_after_border_fade_and_restore() {
        let code = compile_story_bytecode(
            "clip.hks",
            r#"
            seq {
                scene.hidePicture("border").fade(200).await()
                scene.removeClip("window")
            }.await()
            log("finished")
        "#,
        )
        .expect("compile");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("border fade");
        };
        assert!(runtime.step().expect("fade is pending").is_none());
        let snapshot = runtime.snapshot().expect("save during border exit");
        let mut runtime = StoryRuntime::restore(code, snapshot).expect("restore border exit");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect { effect: replay, .. } if replay == effect)
        );
        assert!(
            runtime
                .step()
                .expect("restored fade still pending")
                .is_none()
        );
        runtime
            .complete_task_effect(task, &effect)
            .expect("finish fade");
        // Clip edits have no animated lifetime: they must not create a task
        // completion request that the ECS dispatcher cannot fulfill.
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Clip(ClipCommand::Remove { name })) if name == "window")
        );
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(message)) if message == "finished")
        );
    }

    #[test]
    fn picture_transform_is_statement_scoped_and_awaitable() {
        let mut runtime = runtime(
            r#"
            scene.transformPicture("panel").scale(1.15).time(2.4).easing(.easeOut).await()
            log("finished")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("expected awaited picture transform");
        };
        assert!(
            matches!(&effect, StoryEffect::Picture(PictureCommand::Transform {
            position: [None, None], scale: Some(scale), rotation: None, seconds, ease, ..
        }) if (*scale - 1.15).abs() < 0.001 && (*seconds - 2.4).abs() < 0.001 && *ease == crate::script::animation::Easing::EaseOut)
        );
        assert!(runtime.step().expect("waiting").is_none());
        runtime
            .complete_task_effect(task, &effect)
            .expect("transform finishes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(message)) if message == "finished")
        );
    }

    #[test]
    fn script_presentation_returns_a_parallel_task_and_restores_its_join() {
        let code = compile_story_bytecode(
            "presentation.hks",
            r#"
            global fn present(texture: String) -> Task {
                par {
                    scene.picture("panel", texture).size(500, 500).fade(400)
                    scene.transformPicture("panel").y(60).time(0.4).easing(.easeOut)
                }
            }
            present("image/panel").await()
            log("complete")
        "#,
        )
        .expect("script-defined task factory");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        let mut pending = Vec::new();
        while let Some(next) = runtime.step().expect("start parallel effects") {
            let StoryRuntimeEvent::TaskEffect { task, effect } = next else {
                panic!("expected a presentation effect");
            };
            pending.push((task, effect));
        }
        assert_eq!(pending.len(), 2);
        assert!(
            matches!(&pending[0].1, StoryEffect::Picture(PictureCommand::Show { path, .. }) if path == "image/panel")
        );
        runtime = StoryRuntime::restore(code, runtime.snapshot().expect("snapshot joined task"))
            .expect("restore");
        // Restore reissues in-flight host effects so ECS can rebuild their
        // completion requests; consume those before reporting completion.
        for (expected_task, expected_effect) in &pending {
            assert!(
                matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect { task, effect }
                if task == *expected_task && effect == *expected_effect)
            );
        }
        let (task, effect) = &pending[1];
        runtime
            .complete_task_effect(*task, effect)
            .expect("movement ends first");
        let next = runtime.step().expect("still waiting for fade");
        assert!(next.is_none(), "unexpected event: {next:?}");
        let (task, effect) = &pending[0];
        runtime
            .complete_task_effect(*task, effect)
            .expect("fade ends");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(message)) if message == "complete")
        );
    }

    #[test]
    fn actor_motion_is_typed_and_joins_sequence_effects() {
        let mut runtime = runtime(
            r#"
            let alice = char("alice")
            alice.show()
            let task = seq {
                alice.offset(.pos(0, 20)).time(0.2).easing(.easeOut)
                alice.offset(.pos(0, 0)).time(0.2).easing(.easeIn)
            }
            wait(task)
        "#,
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { .. })
        ));
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect {
            effect: StoryEffect::ActorMotion { actor_id, revision: 1, transition }, ..
        } if actor_id == "alice" && transition.target == [0.0, 20.0])
        );
    }

    #[test]
    fn atomic_sequence_restores_without_prefetching_the_next_translation() {
        let code = compile_story_bytecode(
            "sequence.hks",
            r#"
            let alice = char("alice")
            alice.show()
            let jump = seq {
                alice.offset(.pos(0, 20)).time(0.2).easing(.easeOut)
                alice.offset(.pos(0, 0)).time(0.2).easing(.easeIn)
            }
            jump.await()
            log("joined")
        "#,
        )
        .expect("sequence compiles");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { .. })
        ));
        let StoryRuntimeEvent::TaskEffect {
            task,
            effect: first,
        } = event(&mut runtime)
        else {
            panic!("first translation");
        };
        for _ in 0..8 {
            assert!(runtime.step().expect("paused sequence").is_none());
        }
        let snapshot = runtime.snapshot().expect("snapshot while moving");
        let mut runtime = StoryRuntime::restore(code, snapshot).expect("restored sequence");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect { effect, .. } if effect == first)
        );
        assert!(
            runtime
                .step()
                .expect("still awaiting first motion")
                .is_none()
        );
        runtime
            .complete_task_effect(task, &first)
            .expect("first completes");
        let StoryRuntimeEvent::TaskEffect { effect: second, .. } = event(&mut runtime) else {
            panic!("second translation");
        };
        assert!(
            matches!(&second, StoryEffect::ActorMotion { revision: 2, transition, .. } if transition.target == [0.0, 0.0])
        );
        assert!(runtime.step().expect("join waits for second").is_none());
        runtime
            .complete_task_effect(task, &second)
            .expect("second completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "joined")
        );
    }

    #[test]
    fn parallel_atomic_translations_join_all_completions_out_of_order() {
        let mut runtime = runtime(
            r#"
            let alice = char("alice")
            let bob = char("bob")
            alice.show()
            bob.show()
            let group = par {
                alice.offset(.pos(0, 20)).time(2).easing(.linear)
                bob.offset(.pos(0, 10)).time(1).easing(.linear)
            }
            log("launched")
            group.await()
            log("joined")
        "#,
        );
        let mut effects = Vec::new();
        let mut launched = false;
        for _ in 0..5 {
            match event(&mut runtime) {
                StoryRuntimeEvent::TaskEffect { task, effect } => effects.push((task, effect)),
                StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "launched" => {
                    launched = true
                }
                StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { .. }) => {}
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(launched);
        assert_eq!(effects.len(), 2);
        runtime
            .complete_task_effect(effects[1].0, &effects[1].1)
            .expect("short animation completes");
        assert!(runtime.step().expect("long animation remains").is_none());
        runtime
            .complete_task_effect(effects[0].0, &effects[0].1)
            .expect("long animation completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "joined")
        );
    }

    #[test]
    fn actor_animation_is_statement_scoped_and_rejects_invalid_duration() {
        assert!(compile_story_bytecode("legacy.hks", r#"char("alice").motion([])"#).is_err());
        let mut runtime =
            runtime(r#"char("alice").show().offset(.pos(0, 2)).time(-1).easing(.linear)"#);
        assert!(runtime.step().is_err());
    }

    #[test]
    fn actor_identity_is_silent_and_visible_actors_retain_their_state() {
        let mut runtime = runtime(
            r#"
            global let alice = char("alice")
            alice.at(.pos(120, 80)).scale(0.5).e("happy")
            alice: "Not on stage yet"
            alice.show()
            char("alice").e("sad")
        "#,
        );
        assert!(
            matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::Say { speaker, .. }) if speaker == "alice")
        );
        assert!(matches!(event(&mut runtime), StoryRuntimeEvent::Wait(_)));
        runtime.resume(Value::Unit).expect("advance dialogue");
        assert!(
            matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { position:[120.0,80.0],scale, .. }) if scale==0.5)
        );
        assert!(
            matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { position:[120.0,80.0],scale,expressions,.. }) if scale==0.5 && expressions==["happy","sad"])
        );
    }

    #[test]
    fn actor_rotation_commits_with_show_and_does_not_conflict_with_camera() {
        let mut runtime = runtime(
            "let alice = char(\"alice\")\nalice.rotation(40).show()\nalice.e(\"happy\")\ncamera().rotation(0, 0, 10)",
        );
        for _ in 0..2 {
            assert!(
                matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::ShowCharacter { rotation, .. }) if rotation == 40.0)
            );
        }
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetCamera {
                rotation: Some([0.0, 0.0, 10.0]),
                ..
            })
        ));
    }

    #[test]
    fn hiding_accepts_a_fade_without_flushing_a_new_actor() {
        let mut runtime = runtime("char(\"alice\").hide(300)\nscene.hideCharacters(600)");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: Some("alice".into()),
                fade_ms: 300
            })
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: None,
                fade_ms: 600
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn picture_tint_commits_byte_channels_and_supports_await() {
        let mut runtime =
            runtime("scene.tintPicture(\"room\", 0, 128, 255, 255).fade(300).await()");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect { effect: StoryEffect::Picture(
            PictureCommand::Tint { id, color, seconds }
        ), .. } if id == "room" && color == [0.0, 128.0 / 255.0, 1.0, 1.0] && (seconds - 0.3).abs() < 0.001)
        );
    }

    #[test]
    fn picture_layer_is_independent_of_convenience_position() {
        let mut runtime =
            runtime("scene.picture(\"panel\", \"cover\").at(.center).size(1280, 720).layer(14)");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                layer: 14.0,
                position: [50.0, 50.0],
                size: Some([1280.0, 720.0]),
                ..
            }))
        ));
    }

    #[test]
    fn event_layer_fades_join_before_dialogue() {
        let mut runtime = runtime(
            r#"
            par {
                scene.picture("alice", "mask").at(.center).layer(12).fade(50)
                scene.picture("bob", "cover").at(.center).layer(13).fade(100)
            }.await()
            "Ready"
        "#,
        );
        let StoryRuntimeEvent::TaskEffect {
            task: first,
            effect: first_effect,
        } = event(&mut runtime)
        else {
            panic!("first picture must start asynchronously")
        };
        let StoryRuntimeEvent::TaskEffect {
            task: second,
            effect: second_effect,
        } = event(&mut runtime)
        else {
            panic!("second picture must start before the first completes")
        };
        assert!(runtime.step().expect("waiting for fades").is_none());
        runtime
            .complete_task_effect(first, &first_effect)
            .expect("first fade finishes");
        assert!(
            runtime
                .step()
                .expect("second fade is still active")
                .is_none()
        );
        runtime
            .complete_task_effect(second, &second_effect)
            .expect("second fade finishes");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Say { .. })
        ));
        assert!(matches!(event(&mut runtime), StoryRuntimeEvent::Wait(_)));
    }

    #[test]
    fn entrance_wait_does_not_join_independent_camera_track() {
        let mut runtime = runtime(
            r#"
            let entrance = par { scene.picture("panel", "room").fade(1000) }
            par { scene.transformPicture("panel").scale(2).time(80).easing(.linear) }
            entrance.await()
            log("entered")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect {
            task: entrance,
            effect: show,
        } = event(&mut runtime)
        else {
            panic!("entrance starts first")
        };
        assert!(matches!(
            show,
            StoryEffect::Picture(PictureCommand::Show { .. })
        ));
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::Picture(PictureCommand::Transform { seconds: 80.0, .. }),
                ..
            }
        ));
        assert!(runtime.step().expect("wait for entrance").is_none());
        runtime
            .complete_task_effect(entrance, &show)
            .expect("entrance finished");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(message)) if message == "entered")
        );
    }

    #[test]
    fn picture_blur_commits_a_scoped_transition() {
        let mut runtime = runtime("scene.blurPicture(\"room-middle\", 12).fade(300)");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Picture(
            PictureCommand::Blur { id, radius: 12.0, seconds }
        )) if id == "room-middle" && (seconds - 0.3).abs() < 0.001)
        );
    }

    #[test]
    fn post_process_settings_are_typed_values_with_independent_layer_targets() {
        let mut runtime = runtime(
            r#"
            scene.postProcess(.scene, .identity.blur(8).exposure(-0.5))
            scene.postProcess(.ui, .identity.saturation(0.25))
            scene.postProcessPicture("room", .identity.blur(4))
            scene.postProcess(.canvas, .identity)
            camera(.ui).blur(3)
        "#,
        );
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::PostProcess {
            scope: crate::script::CameraEffectScope::World, parameters
        }) if parameters.blur_radius == 8.0 && parameters.exposure == -0.5)
        );
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::PostProcess {
            scope: crate::script::CameraEffectScope::Ui, parameters
        }) if parameters.blur_radius == 0.0 && parameters.saturation == 0.25)
        );
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::PostProcess {
            id, parameters
        })) if id == "room" && parameters.blur_radius == 4.0)
        );
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::PostProcess {
            scope: crate::script::CameraEffectScope::Canvas, parameters
        }) if parameters == Default::default())
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetCamera {
                scope: crate::script::CameraEffectScope::Ui,
                blur: Some(3.0),
                ..
            })
        ));
    }

    #[test]
    fn picture_and_character_visibility_are_native_capabilities() {
        let mut runtime = runtime(
            "scene.picture(\"room\", \"alice/background\").frame(80, -35, 3, -10, 5).fade(300)\nchar(\"alice\").hide()\nscene.hideCharacters()",
        );
        assert!(
            matches!(event(&mut runtime),StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show { position:[80.0,-35.0],seconds, .. })) if (seconds-0.3).abs()<0.001)
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: Some("alice".into()),
                fade_ms: 0
            })
        );
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::HideCharacter {
                actor_id: None,
                fade_ms: 0
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn curtain_color_uses_byte_channels_and_keeps_await_semantics() {
        let mut runtime = runtime("scene.curtain(1).color(255, 32, 0).fade(200).await()");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::SetCurtain {
                    color: [255, 32, 0],
                    fade_ms: Some(200),
                    ..
                },
                ..
            }
        ));
        assert!(runtime.step().expect("curtain waits").is_none());
    }

    #[test]
    fn curtain_uses_a_scene_selector_and_commits_a_single_transition() {
        let mut runtime = runtime("scene.curtain(0).fade(1200)");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetCurtain {
                color: [0; 3],
                opacity: 0.0,
                fade_ms: Some(1200),
                mask: None,
                softness: 0.0,
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn curtain_dissolve_is_data_driven_and_statement_scoped() {
        let mut runtime =
            runtime("scene.curtain(1).dissolve(\"transitions/blinds\", 0.1).fade(900)");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::SetCurtain {
                color: [0; 3],
                opacity: 1.0,
                fade_ms: Some(900),
                mask: Some("transitions/blinds".into()),
                softness: 0.1,
            })
        );
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn curtain_wait_yields_after_commit_and_resumes_once() {
        let source = "scene.curtain(1).dissolve(\"transitions/blinds\").fade(900).await()";
        let mut runtime = runtime(source);
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("tracked curtain");
        };
        assert!(matches!(effect, StoryEffect::SetCurtain { .. }));
        assert!(runtime.step().expect("await blocks").is_none());
        let snapshot = runtime.snapshot().expect("curtain boundary can be saved");
        let mut runtime = StoryRuntime::restore(
            compile_story_bytecode("test.hks", source).expect("deterministic recompile"),
            snapshot,
        )
        .expect("curtain wait restores");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::TaskEffect { effect: restored, .. } if restored == effect)
        );
        runtime
            .complete_task_effect(task, &effect)
            .expect("curtain completion resumes execution");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Completed(_)
        ));
    }

    #[test]
    fn fluent_await_uses_effect_completions_for_every_builder() {
        for source in [
            "camera().zoom(1.2).time(1).await()",
            "bg(\"room\").fade(300).await()",
            "cg(\"still\").fade(300).await()",
            "scene.hidePicture(\"still\").fade(300).await()",
            "scene.blurPicture(\"room\", 12).fade(300).await()",
            "scene.tintPicture(\"room\", 0, 0, 0, 255).fade(300).await()",
            "scene.movePicture(\"room\", 50, 40, 1, \"linear\").await()",
            "voice(\"voice/alice\").await()",
            "sfx(\"sound/bell\").await()",
            "bgm(\"music/theme\").fadeIn(500).await()",
            "char(\"alice\").show().await()",
            "char(\"alice\").show().at(.pos(10, 20)).scale(1.2).time(0.9).easing(.linear).await()",
            "char(\"alice\").hide(300).await()",
            "scene.hideCharacters(300).await()",
        ] {
            let mut runtime = runtime(&format!("{source}\nlog(\"after\")"));
            let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
                panic!("expected completion protocol for {source}");
            };
            assert!(
                runtime.step().expect("await blocks execution").is_none(),
                "{source}"
            );
            runtime
                .complete_task_effect(task, &effect)
                .expect("effect completes");
            assert!(
                matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "after"),
                "{source}"
            );
        }
    }

    #[test]
    fn awaited_voice_is_not_replayed_after_load() {
        let source = "voice(\"voice/alice\").await()\nlog(\"after\")";
        let code = compile_story_bytecode("voice.hks", source).expect("voice compiles");
        let mut runtime = StoryRuntime::new(code.clone()).expect("runtime");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::TaskEffect {
                effect: StoryEffect::PlayVoice { .. },
                ..
            }
        ));
        assert!(runtime.step().expect("voice waits").is_none());
        let snapshot = runtime.snapshot().expect("voice save boundary");
        let mut restored = StoryRuntime::restore(code, snapshot).expect("voice restores");
        assert!(
            matches!(event(&mut restored), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "after")
        );
    }

    #[test]
    fn fluent_await_joins_every_effect_committed_by_the_statement() {
        let mut runtime = runtime(
            r#"
            char("alice").show().offset(.pos(0, 20)).time(1).easing(.linear).await()
            log("after")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect { task, effect: show } = event(&mut runtime) else {
            panic!("show effect");
        };
        let StoryRuntimeEvent::TaskEffect { effect: motion, .. } = event(&mut runtime) else {
            panic!("offset effect");
        };
        assert!(matches!(show, StoryEffect::ShowCharacter { .. }));
        assert!(matches!(motion, StoryEffect::ActorMotion { .. }));
        runtime
            .complete_task_effect(task, &show)
            .expect("show finishes first");
        assert!(runtime.step().expect("offset is still running").is_none());
        runtime
            .complete_task_effect(task, &motion)
            .expect("offset finishes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "after")
        );
    }

    #[test]
    fn explicit_await_also_pauses_parallel_execution() {
        let mut runtime = runtime(
            r#"
            let group = par {
                voice("voice/alice").await()
                voice("voice/bob")
            }
            group.await()
            log("joined")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("first voice");
        };
        assert!(runtime.step().expect("explicit wait in par").is_none());
        runtime
            .complete_task_effect(task, &effect)
            .expect("first completes");
        let StoryRuntimeEvent::TaskEffect { effect, .. } = event(&mut runtime) else {
            panic!("second voice");
        };
        runtime
            .complete_task_effect(task, &effect)
            .expect("second completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Log(text)) if text == "joined")
        );
    }

    #[test]
    fn picture_shortcuts_use_the_same_builder_as_explicit_picture_calls() {
        for (short, explicit) in [("bg", "Backgrounds"), ("cg", "Stills")] {
            let suffix = ".frame(50, 40, 1.2, 5, 2).fade(300)";
            let mut shorthand = runtime(&format!("{short}(\"alice/image\"){suffix}"));
            let view = if short == "bg" {
                ".view(.background)"
            } else {
                ""
            };
            let mut original = runtime(&format!(
                "scene.picture(\"{explicit}\", \"alice/image\"){view}{suffix}"
            ));
            assert_eq!(event(&mut shorthand), event(&mut original));
            assert!(matches!(
                event(&mut shorthand),
                StoryRuntimeEvent::Completed(_)
            ));
        }
    }

    #[test]
    fn background_builder_commits_once_and_delay_restores_at_host_boundary() {
        let source = "bg(\"alice/background\").fade(1200)\nsleep(1.8)\n\"after\"";
        let mut runtime = runtime(source);
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                dissolve: None,
                video: None,
                replace: false,
                post_process: None,
                screen_space: false,
                view: crate::scene::pictures::PictureView::Background,
                id: "Backgrounds".into(),
                size: None,
                slice: None,
                color: None,
                path: "alice/background".into(),
                rect: None,
                position: [50.0, 50.0],
                scale: 1.0,
                rotation: 0.0,
                layer: 1.0,
                seconds: 1.2,
            }))
        );
        let wait = StoryRuntimeEvent::Wait(StoryWait::Delay { duration_ms: 1800 });
        assert_eq!(event(&mut runtime), wait);
        assert!(runtime.step().expect("wait stays idle").is_none());
        let snapshot = runtime.snapshot().expect("snapshot at wait");
        let bytecode = compile_story_bytecode("test.hks", source).expect("same script compiles");
        let mut runtime = StoryRuntime::restore(bytecode, snapshot).expect("restore");
        assert_eq!(runtime.restored_boundary_event(), Some(wait));
        runtime.resume(Value::Unit).expect("timer completes");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Say { text, .. }) if text == "after")
        );
    }

    #[test]
    fn picture_replace_marks_a_same_asset_crossfade_without_pose_interpolation() {
        let mut runtime =
            runtime("scene.picture(\"room\", \"alice/background\").replace().fade(500)");
        assert!(matches!(event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                replace: true, seconds, ..
            })) if (seconds - 0.5).abs() < 0.001));
    }

    #[test]
    fn sequence_delays_suspend_but_parallel_delays_are_joined() {
        for mode in ["seq", "par"] {
            let mut runtime = runtime(&format!(
                "let h = {mode} {{ sleep(0.1); sleep(0.2) }}\nwait(h)\nlog(\"done\")"
            ));
            let StoryRuntimeEvent::TaskEffect {
                task,
                effect: StoryEffect::Delay { duration_ms: 100 },
            } = event(&mut runtime)
            else {
                panic!("first delay");
            };
            if mode == "seq" {
                assert!(runtime.step().expect("sequence waits").is_none());
                runtime.resume_task(task).expect("first timer completes");
            }
            assert!(matches!(
                event(&mut runtime),
                StoryRuntimeEvent::TaskEffect {
                    effect: StoryEffect::Delay { duration_ms: 200 },
                    ..
                }
            ));
            assert!(runtime.step().expect("join waits").is_none());
            if mode == "par" {
                runtime
                    .resume_task(task)
                    .expect("first parallel timer completes");
            }
            runtime.resume_task(task).expect("second timer completes");
            assert_eq!(
                event(&mut runtime),
                StoryRuntimeEvent::Effect(StoryEffect::Log("done".into()))
            );
        }
    }

    #[test]
    fn delay_in_a_choice_branch_waits_for_its_timer() {
        let mut runtime = runtime("choice { option(\"A\") { sleep(0.1); log(\"after\") } }");
        assert!(matches!(
            event(&mut runtime),
            StoryRuntimeEvent::Choice { .. }
        ));
        runtime.resume(Value::Int(0)).expect("select option");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Wait(StoryWait::Delay { duration_ms: 100 })
        );
        assert!(runtime.step().expect("branch waits").is_none());
        runtime.resume(Value::Unit).expect("timer completes");
        assert_eq!(
            event(&mut runtime),
            StoryRuntimeEvent::Effect(StoryEffect::Log("after".into()))
        );
    }
}
