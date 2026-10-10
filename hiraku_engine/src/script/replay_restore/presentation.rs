use super::*;

/// A reconstruction batch, dispatched through the normal ECS effect boundary.
/// It is deliberately not the saved presentation: current scripts define it.
#[derive(Default)]
pub(crate) struct Presentation {
    pub effects: Vec<StoryEffect>,
    pub dialogue: Option<crate::state::DialogueSnapshot>,
    pub actor_blurs: std::collections::BTreeMap<String, crate::script::actor_blur::ActorBlur>,
    pictures: std::collections::BTreeMap<String, crate::scene::pictures::PictureState>,
}

impl Presentation {
    pub(super) fn record(
        &mut self,
        mut effect: StoryEffect,
        path: &str,
        vfs: &VfsResource,
    ) -> Result<(), String> {
        use crate::scene::pictures::PictureCommand as Picture;
        use crate::stage::runtime::StageCommand as Spatial;
        use StoryEffect::*;
        let instant = super::super::animation::AnimationSpec::Linear(0.0, false);
        match &mut effect {
            Say { speaker, text } => {
                self.dialogue = Some(crate::state::DialogueSnapshot {
                    speaker: speaker.clone(),
                    text: text.clone(),
                });
                return Ok(());
            }
            ContinueDialogue { text } => {
                self.dialogue
                    .get_or_insert_with(Default::default)
                    .text
                    .push_str(text);
                return Ok(());
            }
            ClearDialogue | ClearDialogueText => {
                self.dialogue = None;
                return Ok(());
            }
            ShowCharacter {
                placement_animation,
                ..
            } => *placement_animation = Some(instant),
            HideCharacter { fade_ms, .. } | StopBgm { fade_ms } => *fade_ms = 0,
            ActorMotion { transition, .. } => {
                transition.animation = instant;
                transition.oscillation = None;
            }
            ActorBlur {
                actor_id, radius, ..
            } => {
                let mut blur =
                    crate::script::actor_blur::ActorBlur::new(*radius, *radius, instant, None);
                blur.advance(0.0);
                self.actor_blurs.insert(actor_id.clone(), blur);
                return Ok(());
            }
            SetBackground { fade_in_ms, .. } | PlayBgm { fade_in_ms, .. } => *fade_in_ms = None,
            SetCurtain { fade_ms, .. } => *fade_ms = None,
            SetCamera { duration_ms, .. } => *duration_ms = 0,
            Spatial(command) => match command {
                Spatial::Open { path: target, .. } => {
                    *target = vfs.0.resolve_path(Some(path), target)
                }
                Spatial::Camera { animation, .. } | Spatial::View { animation, .. } => {
                    *animation = instant
                }
                Spatial::Play { looping: false, .. } => return Ok(()),
                Spatial::Play { looping: true, .. } => (),
                Spatial::Close { .. } | Spatial::Place { .. } | Spatial::Clip { .. } => (),
            },
            Picture(command) => match command {
                Picture::Show { seconds, .. }
                | Picture::Transform { seconds, .. }
                | Picture::Hide { seconds, .. }
                | Picture::Exit { seconds, .. }
                | Picture::Tint { seconds, .. }
                | Picture::Blur { seconds, .. } => *seconds = 0.0,
                Picture::Oscillate { .. } | Picture::AnimateX { .. } => return Ok(()),
                Picture::Pivot { .. }
                | Picture::PostProcess { .. }
                | Picture::Noise { .. }
                | Picture::Clear
                | Picture::StopMotion { .. } => (),
            },
            InputEnabled(_)
            | InputPermissions(_)
            | PostProcess { .. }
            | Clip(_)
            | SetActorDepth { .. }
            | StopActorMotion { .. }
            | DialogueSpeed(_) => (),
            // Input, persistence, transient audio and notifications must not be replayed into ECS.
            ShakeCamera { .. }
            | MovieBackground { .. }
            | StopMovie
            | Log(_)
            | SaveSlot(_)
            | StopVoice
            | StopSfxChannel { .. }
            | SetSfxChannelVolume { .. }
            | PlaySfxChannel { .. }
            | PlayVoice { .. }
            | PlaySfx { .. }
            | Exit
            | Delay { .. }
            | Navigate(_)
            | SetUiRole { .. }
            | MountUiOverlay { .. }
            | UnmountUiOverlay { .. }
            | AdjustSetting { .. } => return Ok(()),
        }
        if let Picture(command) = effect {
            return crate::scene::pictures::apply_picture_command(&mut self.pictures, command);
        }
        // Only the final music selection is relevant. Never start all earlier tracks.
        if matches!(effect, PlayBgm { .. } | StopBgm { .. }) {
            self.effects
                .retain(|effect| !matches!(effect, PlayBgm { .. } | StopBgm { .. }));
        }
        self.effects.push(effect);
        Ok(())
    }

    pub(super) fn finish(mut self) -> Self {
        use crate::scene::pictures::PictureCommand;
        // Materialize only pictures which still exist at the destination. This
        // avoids loading every historical background while rebuilding the scene.
        for (_, picture) in std::mem::take(&mut self.pictures) {
            let id = picture.id;
            self.effects
                .push(StoryEffect::Picture(PictureCommand::Show {
                    id: id.clone(),
                    path: picture.path,
                    view: picture.view,
                    dissolve: picture.dissolve,
                    post_process: Some(picture.post_process),
                    video: picture.video,
                    replace: false,
                    screen_space: picture.screen_space,
                    size: picture.size,
                    slice: picture.slice,
                    color: Some(picture.tint),
                    rect: picture.rect,
                    position: picture.position,
                    scale: picture.scale,
                    rotation: picture.rotation,
                    layer: picture.layer,
                    seconds: 0.0,
                }));
            self.effects
                .push(StoryEffect::Picture(PictureCommand::Pivot {
                    id: id.clone(),
                    pivot: picture.pivot,
                }));
            if let Some(noise) = picture.noise {
                self.effects
                    .push(StoryEffect::Picture(PictureCommand::Noise {
                        id,
                        grid: noise.grid,
                        interval: noise.interval,
                    }));
            }
        }
        self
    }
}
