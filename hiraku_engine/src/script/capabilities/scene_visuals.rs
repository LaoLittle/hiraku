//! Statement-scoped scene presentation builders, unrelated to anchored 3D stages.
use std::{collections::BTreeMap, time::Duration};

use hiraku_script::native::{NativeError, NativeRegistry};
use serde::{Deserialize, Serialize};

use super::{CharacterContext, StoryEffect};
use crate::scene::clipping::{ClipCommand, ClipRegion};
use crate::scene::pictures::PictureCommand;

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "SceneClip", handle_type = 5)]
pub(super) struct SceneClipHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "SceneTransition", handle_type = 4)]
pub(super) struct SceneTransitionHandle(pub(super) u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum SceneVisualTarget {
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        self.begin(SceneVisualTarget::HideCharacters { duration_ms })
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        if id.trim().is_empty() || texture.trim().is_empty() {
            return Err(NativeError::message(
                "picture identity and texture must not be empty",
            ));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Show {
                screen_space: false,
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

    #[hks(name = "screenSpace", receiver)]
    fn screen_space(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let Some((SceneVisualTarget::Picture(PictureCommand::Show { screen_space, .. }), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "screenSpace requires an uncommitted picture",
            ));
        };
        *screen_space = true;
        Ok(handle)
    }

    #[hks(name = "frame", receiver)]
    fn frame(
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

    #[hks(name = "size", selector = "SceneTransition", receiver)]
    fn picture_size(
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

    #[hks(name = "slice", selector = "SceneTransition", receiver)]
    fn picture_slice(
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

    #[hks(name = "tint", selector = "SceneTransition", receiver)]
    fn picture_tint(
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

    /// Change only a shown picture's tint; channels use sRGB bytes.
    #[hks(name = "tintPicture", selector = "scene")]
    fn tint_picture(
        context: &mut CharacterContext,
        id: String,
        r: i32,
        g: i32,
        b: i32,
        a: i32,
    ) -> Result<SceneTransitionHandle, NativeError> {
        let bytes = [r, g, b, a];
        if id.trim().is_empty() || !bytes.iter().all(|v| (0..=255).contains(v)) {
            return Err(NativeError::message(
                "tintPicture requires an identity and RGBA channels in 0..=255",
            ));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Tint {
                id,
                color: bytes.map(|v| v as f32 / 255.0),
                seconds: 0.0,
            }))
    }

    /// Radius is in source-image pixels, not a global camera blur amount.
    #[hks(name = "blurPicture", selector = "scene")]
    fn blur_picture(
        context: &mut CharacterContext,
        id: String,
        radius: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if id.trim().is_empty() || !radius.is_finite() || !(0.0..=128.0).contains(&radius) {
            return Err(NativeError::message(
                "blurPicture requires an identity and radius in 0..=128 pixels",
            ));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Blur {
                id,
                radius: radius as f32,
                seconds: 0.0,
            }))
    }

    #[hks(name = "hidePicture", selector = "scene")]
    fn hide_picture(
        context: &mut CharacterContext,
        id: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Hide {
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
    ) -> Result<SceneTransitionHandle, NativeError> {
        milliseconds(seconds)?;
        if ![x, y].iter().all(|n| n.is_finite() && n.abs() <= 100000.0)
            || !["linear", "smoothStep", "easeOutQuad", "easeOutBack"].contains(&ease.as_str())
        {
            return Err(NativeError::message("invalid picture movement or easing"));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Transform {
                id,
                position: [Some(x as f32), Some(y as f32)],
                scale: None,
                rotation: None,
                seconds: seconds as f32,
                ease,
            }))
    }

    /// Edit the displayed pose without showing/replacing the texture. Missing
    /// fields are resolved from the current pose when ECS applies the command.
    #[hks(name = "transformPicture", selector = "scene")]
    fn transform_picture(
        context: &mut CharacterContext,
        id: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if id.trim().is_empty() {
            return Err(NativeError::message("picture identity must not be empty"));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::Transform {
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
    #[hks(name = "x", selector = "SceneTransition", receiver)]
    fn transform_x(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 0, value)
    }

    #[hks(name = "y", selector = "SceneTransition", receiver)]
    fn transform_y(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 1, value)
    }

    #[hks(name = "scale", selector = "SceneTransition", receiver)]
    fn transform_scale(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 2, value)
    }

    #[hks(name = "rotation", selector = "SceneTransition", receiver)]
    fn transform_rotation(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        value: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        transform_field(context, handle, 3, value)
    }

    #[hks(name = "animation", selector = "SceneTransition", receiver)]
    fn transform_animation(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        animation: crate::script::animation::AnimationSpec,
    ) -> Result<SceneTransitionHandle, NativeError> {
        use crate::script::animation::AnimationSpec;
        let (duration, curve) = match animation {
            AnimationSpec::Linear(duration, _) => (duration, "linear"),
            AnimationSpec::EaseIn(duration, _) => (duration, "easeInQuad"),
            AnimationSpec::EaseOut(duration, _) => (duration, "easeOutQuad"),
            AnimationSpec::EaseOutSine(duration, _) => (duration, "easeOutSine"),
            AnimationSpec::EaseInOutSine(duration, _) => (duration, "easeInOutSine"),
            AnimationSpec::EaseInOut(duration, _) => (duration, "easeInOutQuad"),
        };
        milliseconds(duration)?;
        if animation.repeats() {
            return Err(NativeError::message(
                "scene command animations must complete",
            ));
        }
        if let Some((
            SceneVisualTarget::Spatial(
                crate::stage::runtime::StageCommand::Camera {
                    animation: target, ..
                }
                | crate::stage::runtime::StageCommand::View {
                    animation: target, ..
                },
            ),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        {
            *target = animation;
            return Ok(handle);
        }
        let Some((
            SceneVisualTarget::Picture(
                PictureCommand::Transform { seconds, ease, .. }
                | PictureCommand::Exit { seconds, ease, .. },
            ),
            _,
        )) = context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "animation requires an uncommitted picture transform",
            ));
        };
        *seconds = duration as f32;
        *ease = curve.into();
        Ok(handle)
    }

    /// One owned exit: final position and alpha finish before removal.
    #[hks(name = "to", selector = "SceneTransition", receiver)]
    fn exit_to(
        context: &mut CharacterContext,
        handle: SceneTransitionHandle,
        x: f64,
        y: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !x.is_finite() || !y.is_finite() || x.abs() > 100000.0 || y.abs() > 100000.0 {
            return Err(NativeError::message("invalid picture exit position"));
        }
        let Some((SceneVisualTarget::Picture(command), _)) =
            context.scene_visuals.pending.get_mut(&handle.0)
        else {
            return Err(NativeError::message(
                "to requires an uncommitted picture exit",
            ));
        };
        let PictureCommand::Hide { id, seconds } = command else {
            return Err(NativeError::message("to requires hidePicture"));
        };
        *command = PictureCommand::Exit {
            id: id.clone(),
            position: [x as f32, y as f32],
            seconds: *seconds,
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

    #[hks(name = "animatePictureX", selector = "scene")]
    fn animate_picture_x(
        context: &mut CharacterContext,
        id: String,
        offsets: Vec<f64>,
        step_seconds: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if offsets.is_empty()
            || offsets.len() > 4096
            || !offsets.iter().all(|n| n.is_finite() && n.abs() < 100000.0)
            || !(0.001..=60.0).contains(&step_seconds)
        {
            return Err(NativeError::message("invalid picture keyframes"));
        }
        context
            .scene_visuals
            .begin(SceneVisualTarget::Picture(PictureCommand::AnimateX {
                id,
                offsets: offsets.into_iter().map(|n| n as f32).collect(),
                step_seconds: step_seconds as f32,
            }))
    }

    #[hks(name = "bg")]
    fn background(
        context: &mut CharacterContext,
        texture: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        picture(context, "Backgrounds".into(), texture)
    }

    #[hks(name = "cg")]
    fn cg(
        context: &mut CharacterContext,
        texture: String,
    ) -> Result<SceneTransitionHandle, NativeError> {
        picture(context, "Stills".into(), texture)
    }

    /// A scene-space blackout, below script-owned UI. Independent of background
    /// identity, so replacing a background does not reveal it before the fade.
    #[hks(name = "curtain", selector = "scene")]
    fn curtain(
        context: &mut CharacterContext,
        opacity: f64,
    ) -> Result<SceneTransitionHandle, NativeError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(NativeError::message(
                "curtain opacity must be between 0 and 1",
            ));
        }
        context.scene_visuals.begin(SceneVisualTarget::Curtain {
            color: [0; 3],
            opacity: opacity as f32,
            mask: None,
            softness: 0.0,
        })
    }

    /// Select the curtain pigment; fade controls its opacity independently.
    #[hks(name = "color", receiver)]
    fn color(
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
    #[hks(name = "dissolve", receiver)]
    fn dissolve(
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
    #[hks(name = "await", selector = "SceneTransition", receiver)]
    fn await_transition(
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
    #[hks(name = "fade", receiver)]
    fn fade_in(
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
        pending.1 = (duration > 0).then_some(duration);
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
    use super::*;

    #[test]
    fn screen_space_and_atomic_exit_are_registered_native_builders() {
        let mut show = runtime("scene.picture(\"panel\", \"image/panel\").screenSpace()");
        assert!(matches!(
            event(&mut show),
            StoryRuntimeEvent::Effect(StoryEffect::Picture(PictureCommand::Show {
                screen_space: true,
                ..
            }))
        ));
        let mut exit =
            runtime("scene.hidePicture(\"panel\").to(20, 30).animation(.easeIn(0.4)).await()");
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut exit) else {
            panic!("exit must expose one awaitable effect");
        };
        assert!(
            matches!(&effect, StoryEffect::Picture(PictureCommand::Exit { position: [20.0, 30.0], seconds, ease, .. })
            if (*seconds - 0.4).abs() < 0.0001 && ease == "easeInQuad")
        );
        assert!(exit.step().expect("wait for owned exit").is_none());
        exit.complete_task_effect(task, &effect)
            .expect("complete exit");
    }
    use crate::script::capabilities::{StoryWait, compile_story_bytecode};
    use crate::script::{StoryRuntime, StoryRuntimeEvent};
    use hiraku_script::Value;

    fn runtime(source: &str) -> StoryRuntime {
        StoryRuntime::new(compile_story_bytecode("test.hks", source).expect("script compiles"))
            .expect("runtime initializes")
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
            scene.transformPicture("panel").scale(1.15).animation(.easeOut(2.4)).await()
            log("finished")
        "#,
        );
        let StoryRuntimeEvent::TaskEffect { task, effect } = event(&mut runtime) else {
            panic!("expected awaited picture transform");
        };
        assert!(
            matches!(&effect, StoryEffect::Picture(PictureCommand::Transform {
            position: [None, None], scale: Some(scale), rotation: None, seconds, ease, ..
        }) if (*scale - 1.15).abs() < 0.001 && (*seconds - 2.4).abs() < 0.001 && ease == "easeOutQuad")
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
                    scene.transformPicture("panel").y(60).animation(.easeOut(0.4))
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
                alice.offset(.pos(0, 20)).animation(.easeOut(0.2))
                alice.offset(.pos(0, 0)).animation(.easeIn(0.2))
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
                alice.offset(.pos(0, 20)).animation(.easeOut(0.2))
                alice.offset(.pos(0, 0)).animation(.easeIn(0.2))
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
                alice.offset(.pos(0, 20)).animation(.linear(2))
                bob.offset(.pos(0, 10)).animation(.linear(1))
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
            runtime(r#"char("alice").show().offset(.pos(0, 2)).animation(.linear(-1))"#);
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
    fn picture_blur_commits_a_scoped_transition() {
        let mut runtime = runtime("scene.blurPicture(\"room-middle\", 12).fade(300)");
        assert!(
            matches!(event(&mut runtime), StoryRuntimeEvent::Effect(StoryEffect::Picture(
            PictureCommand::Blur { id, radius: 12.0, seconds }
        )) if id == "room-middle" && (seconds - 0.3).abs() < 0.001)
        );
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
            "char(\"alice\").show().at(.pos(10, 20)).scale(1.2).animation(.linear(0.9)).await()",
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
            char("alice").show().offset(.pos(0, 20)).animation(.linear(1)).await()
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
            let mut original = runtime(&format!(
                "scene.picture(\"{explicit}\", \"alice/image\"){suffix}"
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
                screen_space: false,
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
        runtime.resume(Value::Number(0.0)).expect("select option");
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
