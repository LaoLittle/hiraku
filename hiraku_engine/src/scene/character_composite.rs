//! Logical part children retain animation/save state; only the composite draws.
use super::*;
use hiraku_sprite3d::{BlendMode, MaskMode, Sprite3d, Sprite3dPlugin, Sprite3dSync, SpriteLayer};
use std::time::Duration;

#[derive(Component, Clone)]
pub(crate) struct LogicalCharacterPart(pub CharacterPartDefinition);
#[derive(Component)]
struct CompositeDisplay;
#[derive(Component, Default)]
pub(crate) struct CharacterGroup {
    alpha: f32,
    tween: Option<GroupTween>,
    display: Option<Entity>,
    atlas: Option<Handle<TextureAtlasLayout>>,
    error: Option<String>,
}
struct GroupTween {
    from: f32,
    to: f32,
    timer: Timer,
    animation_id: Option<String>,
}

impl CharacterGroup {
    pub(super) fn is_animating(&self) -> bool {
        self.tween.is_some()
    }
}

pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<Sprite3dPlugin>() {
        app.add_plugins(Sprite3dPlugin);
    }
    install_runtime_systems(app);
}

fn install_runtime_systems(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (advance_group_fades, compose_groups)
            .chain()
            // Stage resources are created only after runtime content has loaded.
            .run_if(crate::runtime_initialized)
            .before(Sprite3dSync),
    );
}
pub(super) fn fade_group(
    commands: &mut Commands,
    root: Entity,
    target: f32,
    duration: Duration,
    animation_id: Option<String>,
) {
    commands.queue(move |world: &mut World| {
        let Some(mut group) = world.get_mut::<CharacterGroup>(root) else {
            complete_missing_animation(&mut world.resource_mut::<AnimationState>(), animation_id);
            return;
        };
        let previous = group.tween.take().and_then(|t| t.animation_id);
        group.tween = Some(GroupTween {
            from: group.alpha,
            to: target,
            timer: Timer::new(duration, TimerMode::Once),
            animation_id,
        });
        complete_missing_animation(&mut world.resource_mut::<AnimationState>(), previous);
    });
}
fn advance_group_fades(
    mut commands: Commands,
    time: Res<Time>,
    mut animations: ResMut<AnimationState>,
    mut groups: Query<(&Children, &mut CharacterGroup)>,
    parts: Query<(), With<LogicalCharacterPart>>,
) {
    for (children, mut group) in &mut groups {
        let Some(tween) = group.tween.as_mut() else {
            continue;
        };
        tween.timer.tick(time.delta());
        let alpha = tween.from + (tween.to - tween.from) * tween.timer.fraction();
        let complete = tween.timer.is_finished();
        group.alpha = alpha;
        if complete {
            let tween = group.tween.take().expect("active group tween");
            group.alpha = tween.to;
            complete_missing_animation(&mut animations, tween.animation_id);
            if tween.to == 0.0 {
                for child in children.iter() {
                    if parts.contains(child) {
                        commands
                            .entity(child)
                            .try_insert(Visibility::Hidden)
                            .try_remove::<HideAfterTween>();
                    }
                }
            }
        }
    }
}
fn compose_groups(
    mut commands: Commands,
    images: Res<Assets<Image>>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    mut roots: Query<(Entity, &Children, &mut CharacterGroup)>,
    parts: Query<(
        &LogicalCharacterPart,
        &WorldSprite,
        &Transform,
        &Visibility,
        Has<FocusedActorPart>,
    )>,
    mut displays: Query<
        (&mut Sprite3d, &mut Transform, &mut Visibility),
        (With<CompositeDisplay>, Without<LogicalCharacterPart>),
    >,
) {
    for (root, children, mut group) in &mut roots {
        let mut selected = children
            .iter()
            .filter_map(|e| parts.get(e).ok())
            .filter(|(_, _, _, v, _)| **v != Visibility::Hidden)
            .collect::<Vec<_>>();
        selected.sort_by(|a, b| {
            a.0.0
                .layer
                .total_cmp(&b.0.0.layer)
                .then_with(|| a.0.0.id.cmp(&b.0.0.id))
        });
        if selected.is_empty() || group.alpha == 0.0 {
            if let Some(e) = group.display {
                commands.entity(e).try_insert(Visibility::Hidden);
            }
            continue;
        }
        let (image, rects, mut layers, size, center, _depth) =
            match build_layers(&selected, &images) {
                Ok(value) => value,
                Err(error) => {
                    if group.error.as_ref() != Some(&error) {
                        warn!("character composition failed: {error}");
                        group.error = Some(error);
                    }
                    if let Some(e) = group.display {
                        commands.entity(e).try_insert(Visibility::Hidden);
                    }
                    continue;
                }
            };
        group.error = None;
        let atlas_size = images
            .get(&image)
            .expect("build_layers checked image readiness")
            .size();
        let atlas = match group.atlas.as_ref().filter(|h| atlases.contains(*h)) {
            Some(handle) => {
                let handle = handle.clone();
                let previous = atlases.get(&handle).expect("atlas exists");
                if previous.size != atlas_size || previous.textures != rects {
                    *atlases.get_mut(&handle).expect("atlas exists") = TextureAtlasLayout {
                        size: atlas_size,
                        textures: rects,
                    };
                }
                handle
            }
            None => {
                let handle = atlases.add(TextureAtlasLayout {
                    size: atlas_size,
                    textures: rects,
                });
                group.atlas = Some(handle.clone());
                handle
            }
        };
        for (index, layer) in layers.iter_mut().enumerate() {
            layer.texture_atlas = Some(TextureAtlas {
                layout: atlas.clone(),
                index,
            });
        }
        let sprite = Sprite3d {
            image: Some(image),
            layers,
            custom_size: Some(size),
            color: Color::linear_rgba(1.0, 1.0, 1.0, group.alpha),
            ..default()
        };
        // Per-part layers order composition only. Actor ordering belongs to
        // the root and must not change when a high-layer expression is swapped.
        let transform = Transform::from_xyz(center.x, center.y, super::STAGE_Z_SPRITE);
        let render_layers = if selected.iter().any(|p| p.4) {
            focus_layer()
        } else {
            scene_layer()
        };
        if let Some((entity, (mut current, mut position, mut visibility))) = group
            .display
            .and_then(|e| displays.get_mut(e).ok().map(|data| (e, data)))
        {
            if *current != sprite {
                *current = sprite;
            }
            if *position != transform {
                *position = transform;
            }
            if *visibility != Visibility::Visible {
                *visibility = Visibility::Visible;
            }
            commands.entity(entity).try_insert(render_layers);
        } else {
            group.display = Some(
                commands
                    .spawn((
                        sprite,
                        transform,
                        Visibility::Visible,
                        render_layers,
                        CompositeDisplay,
                        ChildOf(root),
                    ))
                    .id(),
            );
        }
    }
}

type PartView<'a> = (
    &'a LogicalCharacterPart,
    &'a WorldSprite,
    &'a Transform,
    &'a Visibility,
    bool,
);
type CompositeLayers = (Handle<Image>, Vec<URect>, Vec<SpriteLayer>, Vec2, Vec2, f32);
fn build_layers(parts: &[PartView<'_>], images: &Assets<Image>) -> Result<CompositeLayers, String> {
    if parts.len() > hiraku_sprite3d::MAX_LAYERS {
        return Err(format!(
            "{} visible layers exceed the {} layer limit",
            parts.len(),
            hiraku_sprite3d::MAX_LAYERS
        ));
    }
    let image = parts
        .first()
        .and_then(|p| p.1.image.clone())
        .ok_or("character has no atlas image")?;
    let image_size = images
        .get(&image)
        .ok_or("character atlas is still loading")?
        .size();
    let mut source = Vec::new();
    let mut regions = Vec::new();
    let mut layers = Vec::new();
    let mut minimum = Vec2::splat(f32::INFINITY);
    let mut maximum = Vec2::splat(f32::NEG_INFINITY);
    for (part, sprite, transform, _, _) in parts {
        if sprite.image.as_ref() != Some(&image) {
            return Err("a composed character must use a single atlas image".into());
        }
        let r = part
            .0
            .rect
            .unwrap_or([0.0, 0.0, image_size.x as f32, image_size.y as f32]);
        if r.iter()
            .any(|v| !v.is_finite() || *v < 0.0 || v.fract() != 0.0)
            || r[2] <= r[0]
            || r[3] <= r[1]
            || r[2] > image_size.x as f32
            || r[3] > image_size.y as f32
        {
            return Err(format!(
                "part `{}` has an invalid atlas rectangle",
                part.0.id
            ));
        }
        let scale = transform.scale.truncate();
        if !scale.is_finite()
            || scale.min_element() <= 0.0
            || !transform.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5)
        {
            return Err("composed parts require positive scale and no independent rotation".into());
        }
        let half = Vec2::new(r[2] - r[0], r[3] - r[1]) * scale * 0.5;
        let center = transform.translation.truncate();
        minimum = minimum.min(center - half);
        maximum = maximum.max(center + half);
        regions.push((center - half, center + half));
        source.push(URect::new(
            r[0] as u32,
            r[1] as u32,
            r[2] as u32,
            r[3] as u32,
        ));
        let mask = match part.0.mask {
            None => MaskMode::None,
            Some(mask) => {
                let reference = u8::try_from(mask.reference)
                    .map_err(|_| "mask reference exceeds supported range")?;
                if reference == 0 || reference > hiraku_sprite3d::MAX_MASKS {
                    return Err("mask reference outside 1..=8".into());
                }
                match mask.kind {
                    CharacterMaskKind::Read => MaskMode::Read(reference),
                    CharacterMaskKind::Write => MaskMode::StencilWrite {
                        reference,
                        cutoff: 1.0 / 255.0,
                        visible: true,
                    },
                }
            }
        };
        layers.push(SpriteLayer {
            color: sprite.color,
            mask,
            blend: if part.0.blend == CharacterBlendMode::Multiply {
                BlendMode::Multiply
            } else {
                BlendMode::Normal
            },
            ..default()
        });
    }
    let size = maximum - minimum;
    if !size.is_finite() || size.min_element() <= 0.0 {
        return Err("invalid composite bounds".into());
    }
    for (layer, (min, max)) in layers.iter_mut().zip(regions) {
        layer.bounds = Rect::from_corners(
            Vec2::new(min.x - minimum.x, maximum.y - max.y) / size,
            Vec2::new(max.x - minimum.x, maximum.y - min.y) / size,
        );
    }
    Ok((
        image,
        source,
        layers,
        size,
        (minimum + maximum) * 0.5,
        parts[0].2.translation.z,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_waits_for_runtime_initialization() {
        let mut app = App::new();
        install_runtime_systems(&mut app);
        // No stage resources exist while asynchronous content loading proceeds.
        app.update();
        app.update();
        app.init_resource::<Time>()
            .init_resource::<AnimationState>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<TextureAtlasLayout>>()
            .insert_resource(FrontendState {
                startup_script: "startup.hks".into(),
                notice: None,
                runtime_started: false,
            });
        let child = app.world_mut().spawn_empty().id();
        let root = app
            .world_mut()
            .spawn((CharacterGroup {
                alpha: 1.0,
                tween: Some(GroupTween {
                    from: 1.0,
                    to: 0.0,
                    timer: Timer::new(Duration::from_secs(1), TimerMode::Once),
                    animation_id: None,
                }),
                ..default()
            },))
            .add_child(child)
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(500));
        app.update();
        assert_eq!(
            app.world()
                .get::<CharacterGroup>(root)
                .expect("group")
                .alpha,
            0.5
        );
    }

    fn part(id: &str, rect: [f32; 4]) -> LogicalCharacterPart {
        LogicalCharacterPart(CharacterPartDefinition {
            id: id.into(),
            slot: None,
            path: "atlas.png".into(),
            atlas_rect: None,
            offset: Vec2::ZERO,
            layer: 0.0,
            rect: Some(rect),
            mask: None,
            blend: CharacterBlendMode::Normal,
            color: [255; 4],
        })
    }

    #[test]
    fn atlas_cells_preserve_bounds_tint_and_mask_modes() {
        let mut images = Assets::<Image>::default();
        let mut image = Image::default();
        image.resize(bevy::render::render_resource::Extent3d {
            width: 32,
            height: 16,
            depth_or_array_layers: 1,
        });
        let handle = images.add(image);
        let mut writer = part("alice/eyes", [0.0, 0.0, 16.0, 16.0]);
        writer.0.mask = Some(crate::character::CharacterMaskDefinition {
            kind: CharacterMaskKind::Write,
            reference: 1,
        });
        let mut reader = part("alice/shadow", [16.0, 0.0, 32.0, 16.0]);
        reader.0.mask = Some(crate::character::CharacterMaskDefinition {
            kind: CharacterMaskKind::Read,
            reference: 1,
        });
        reader.0.blend = CharacterBlendMode::Multiply;
        let mut sprite = WorldSprite::from_image(handle);
        sprite.color = Color::linear_rgba(1.0, 1.0, 1.0, 0.4);
        let a = Transform::default();
        let b = Transform::from_xyz(16.0, 0.0, 0.0);
        let visible = Visibility::Visible;
        let (_, rects, layers, size, center, _) = build_layers(
            &[
                (&writer, &sprite, &a, &visible, false),
                (&reader, &sprite, &b, &visible, false),
            ],
            &images,
        )
        .expect("valid synthetic atlas");
        assert_eq!(rects[1], URect::new(16, 0, 32, 16));
        assert_eq!(size, Vec2::new(32.0, 16.0));
        assert_eq!(center, Vec2::new(8.0, 0.0));
        assert_eq!(layers[0].bounds, Rect::new(0.0, 0.0, 0.5, 1.0));
        assert_eq!(layers[1].color.alpha(), 0.4);
        assert_eq!(layers[1].blend, BlendMode::Multiply);
        assert_eq!(layers[1].mask, MaskMode::Read(1));
        assert!(matches!(
            layers[0].mask,
            MaskMode::StencilWrite { visible: true, .. }
        ));
    }

    #[test]
    fn group_fade_preserves_intrinsic_part_alpha_and_hides_children() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<AnimationState>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<TextureAtlasLayout>>()
            .add_systems(Update, (advance_group_fades, compose_groups).chain());
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let mut sprite = WorldSprite::from_image(image);
        sprite.color = Color::linear_rgba(1.0, 1.0, 1.0, 0.4);
        let root = app
            .world_mut()
            .spawn(CharacterGroup {
                alpha: 1.0,
                tween: Some(GroupTween {
                    from: 1.0,
                    to: 0.0,
                    timer: Timer::new(Duration::from_secs(1), TimerMode::Once),
                    animation_id: None,
                }),
                ..default()
            })
            .id();
        let child = app
            .world_mut()
            .spawn((
                part("bob/body", [0.0, 0.0, 1.0, 1.0]),
                sprite,
                Transform::default(),
                Visibility::Visible,
                HideAfterTween,
                ChildOf(root),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(500));
        app.update();
        let display = app
            .world()
            .get::<CharacterGroup>(root)
            .expect("root")
            .display
            .expect("composite display");
        let composed = app
            .world()
            .get::<Sprite3d>(display)
            .expect("one composed sprite");
        assert_eq!(composed.color.alpha(), 0.5);
        assert_eq!(composed.layers[0].color.alpha(), 0.4);
        assert_eq!(
            app.world().get::<CharacterGroup>(root).expect("root").alpha,
            0.5
        );
        assert_eq!(
            app.world()
                .get::<WorldSprite>(child)
                .expect("part")
                .color
                .alpha(),
            0.4
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(500));
        app.update();
        assert_eq!(
            *app.world()
                .get::<Visibility>(child)
                .expect("part visibility"),
            Visibility::Hidden
        );
        assert!(app.world().get::<HideAfterTween>(child).is_none());
    }
}
