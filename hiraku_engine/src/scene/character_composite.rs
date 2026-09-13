//! Logical part children retain animation/save state; only the composite draws.
use super::*;
use hiraku_sprite3d::{BlendMode, MaskMode, Sprite3d, Sprite3dPlugin, Sprite3dSync, SpriteLayer};
use std::time::Duration;
mod atlas;
pub(crate) mod source;

#[derive(Component, Clone)]
pub(crate) struct LogicalCharacterPart(
    pub CharacterPartDefinition,
    pub Option<Handle<source::AtlasSource>>,
);
#[derive(Component)]
struct CompositeDisplay;
#[derive(Component, Default)]
pub(crate) struct CharacterGroup {
    alpha: f32,
    tween: Option<GroupTween>,
    display: Option<Entity>,
    atlas: Option<Handle<TextureAtlasLayout>>,
    packed: atlas::PackedAtlas,
    error: Option<String>,
    last_active_revision: u64,
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
    app.init_asset::<source::AtlasSource>()
        .init_asset_loader::<source::AtlasSourceLoader>();
    if !app.is_plugin_added::<Sprite3dPlugin>() {
        app.add_plugins(Sprite3dPlugin);
    }
    install_runtime_systems(app);
}

fn install_runtime_systems(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (advance_group_fades, compose_groups, retire_hidden_groups)
            .chain()
            .after(crate::dependencies::update_resource_window)
            // Stage resources are created only after runtime content has loaded.
            .run_if(crate::runtime_initialized)
            .before(Sprite3dSync),
    );
}

/// Execution-based reuse window for hide/show cuts; persistent actor state lives in the
/// story model, not these rendering entities. Retire the entire hierarchy so
/// both logical sprites and GPU materials release their atlas handles.
fn retire_hidden_groups(
    mut commands: Commands,
    window: Option<Res<crate::dependencies::ScriptDependencies>>,
    mut stage: ResMut<StageState>,
    pending: Res<PendingCharacterShows>,
    children: Query<&Children>,
    parts: Query<&LogicalCharacterPart>,
    mut roots: Query<(Entity, &CharacterRoot, &mut CharacterGroup)>,
) {
    let Some(window) = window else {
        return;
    };
    for (entity, identity, mut group) in &mut roots {
        let name = &identity.actor_id;
        let prefix = format!("character::{name}::");
        let reusable = stage.character_active_parts.contains_key(name)
            || pending.items.iter().any(|show| &show.actor_id == name)
            || stage
                .pending_character_restore
                .iter()
                .any(|part| part.id.starts_with(&prefix))
            || group.tween.is_some()
            || group.alpha != 0.0;
        if reusable {
            group.last_active_revision = window.revision;
            continue;
        }
        if (!window.closed
            && window.revision.saturating_sub(group.last_active_revision)
                <= window.retain_steps as u64)
            || children.get(entity).is_ok_and(|children| {
                children.iter().any(|child| {
                    parts
                        .get(child)
                        .is_ok_and(|part| window.protects(&part.0.path))
                })
            })
        {
            continue;
        }
        if stage.character_roots.get(name) == Some(&entity) {
            stage.character_roots.remove(name);
            stage.sprites.retain(|id, _| !id.starts_with(&prefix));
            stage.character_order.retain(|id| id != name);
        }
        commands.entity(entity).try_despawn();
    }
}
fn composite_transform(center: Vec2, placement: Option<Transform>) -> Transform {
    let (center, rotation) = placement
        .map(|p| {
            let pivot = p.translation.truncate().extend(0.0);
            (
                (pivot + p.rotation * (center.extend(0.0) - pivot)).truncate(),
                p.rotation,
            )
        })
        .unwrap_or((center, Quat::IDENTITY));
    Transform::from_xyz(center.x, center.y, super::STAGE_Z_SPRITE).with_rotation(rotation)
}

#[test]
fn composite_rotates_about_actor_origin_not_bounds_center() {
    let pose = Transform::from_xyz(100.0, 50.0, 0.0)
        .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    let result = composite_transform(Vec2::new(120.0, 50.0), Some(pose));
    assert!(
        result
            .translation
            .truncate()
            .abs_diff_eq(Vec2::new(100.0, 70.0), 0.001)
    );
    assert!(result.rotation.abs_diff_eq(pose.rotation, 0.001));
    assert_eq!(result.scale, Vec3::ONE);
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
    mut redraw: crate::redraw::Redraw,
    mut commands: Commands,
    time: crate::scene::playback::StoryTime,
    mut animations: ResMut<AnimationState>,
    mut groups: Query<(&Children, &mut CharacterGroup)>,
    parts: Query<(), With<LogicalCharacterPart>>,
) {
    for (children, mut group) in &mut groups {
        let Some(tween) = group.tween.as_mut() else {
            continue;
        };
        redraw.request();
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
    mut redraw: crate::redraw::Redraw,
    shared: Res<super::SceneSharedState>,
    spatial: Option<Res<crate::stage::runtime::StageRuntime>>,
    mut images: ResMut<Assets<Image>>,
    mut cpu: source::AtlasSources,
    device: Option<Res<bevy::render::renderer::RenderDevice>>,
    mut image_events: MessageReader<AssetEvent<Image>>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    mut roots: Query<(
        Entity,
        &Children,
        &mut CharacterGroup,
        &super::CharacterRoot,
        Option<&super::character::ActorPlacement>,
    )>,
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
    let changed = image_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::Removed { id } => {
                Some(*id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let limit = device
        .as_ref()
        .map_or(8192, |device| device.limits().max_texture_dimension_2d);
    let changed_sources = cpu.changed();
    for (root, children, mut group, identity, placement) in &mut roots {
        group.packed.invalidate(&changed);
        group.packed.invalidate_sources(&changed_sources);
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
        // AssetServer gates initial reveal; expression changes keep the old
        // composite while their new CPU sources are still arriving.
        if selected.iter().any(|p| {
            p.0.1
                .as_ref()
                .is_some_and(|h| cpu.assets.as_ref().is_none_or(|a| !a.contains(h)))
        }) {
            redraw.request();
            continue;
        }
        let (image, rects, mut layers, size, center, _depth) = match build_layers(
            &selected,
            &mut images,
            &mut group.packed,
            limit,
            cpu.assets.as_deref(),
        ) {
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
            clip: shared.0.clips.actor(&identity.actor_id),
            image: Some(image),
            layers,
            custom_size: Some(size),
            color: Color::linear_rgba(1.0, 1.0, 1.0, group.alpha),
            ..default()
        };
        // Per-part layers order composition only. Actor ordering belongs to
        // the root and must not change when a high-layer expression is swapped.
        // Rotate the composed surface, not each part: mask coordinates and
        // premultiplied composition remain in their shared unrotated plane.
        let mut transform = composite_transform(center, placement.map(|p| p.current));
        if let Some(anchor) = shared
            .0
            .spatial_stage
            .as_ref()
            .and_then(|stage| stage.actors.get(&identity.actor_id))
        {
            let anchor = spatial
                .as_ref()
                .and_then(|runtime| runtime.definition.as_ref())
                .and_then(|definition| definition.anchor(anchor).ok());
            let Some(anchor) = anchor else {
                if let Some(entity) = group.display {
                    commands.entity(entity).try_insert(Visibility::Hidden);
                }
                continue;
            };
            // Pixel-space composition (including intrinsic alpha/masks) remains
            // unchanged. Only the final plane enters the anchor's local space.
            transform.translation.z = 0.0;
            transform = anchor.mul_transform(transform);
        } else if let Some(depth) = shared.0.actor_depths.get(&identity.actor_id) {
            transform.translation.z = *depth;
        }
        let render_layers = if shared
            .0
            .spatial_stage
            .as_ref()
            .is_some_and(|s| s.actors.contains_key(&identity.actor_id))
        {
            crate::stage::views::spatial_layer()
        } else if selected.iter().any(|p| p.4) {
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
                redraw.request();
            }
            if *position != transform {
                *position = transform;
            }
            if *visibility != Visibility::Visible {
                *visibility = Visibility::Visible;
            }
            commands.entity(entity).try_insert(render_layers);
        } else {
            redraw.request();
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
fn build_layers(
    parts: &[PartView<'_>],
    images: &mut Assets<Image>,
    packed: &mut atlas::PackedAtlas,
    limit: u32,
    sources: Option<&Assets<source::AtlasSource>>,
) -> Result<CompositeLayers, String> {
    if parts.len() > hiraku_sprite3d::MAX_LAYERS {
        return Err(format!(
            "{} visible layers exceed the {} layer limit",
            parts.len(),
            hiraku_sprite3d::MAX_LAYERS
        ));
    }
    let image = parts.first().and_then(|p| p.1.image.clone());
    let single_image =
        image.is_some() && parts.iter().all(|p| p.0.1.is_none() && p.1.image == image);
    let mut requested = Vec::new();
    let mut source = Vec::new();
    let mut regions = Vec::new();
    let mut layers = Vec::new();
    let mut minimum = Vec2::splat(f32::INFINITY);
    let mut maximum = Vec2::splat(f32::NEG_INFINITY);
    for (part, sprite, transform, _, _) in parts {
        let part_image = if let Some(handle) = &part.1 {
            atlas::SourceId::Cpu(handle.id())
        } else {
            atlas::SourceId::Render(
                sprite
                    .image
                    .as_ref()
                    .ok_or("character part has no image")?
                    .id(),
            )
        };
        let image_size = part_image
            .get(images, sources)
            .ok_or("character part image is still loading")?
            .size();
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
        requested.push(atlas::Region {
            image: part_image,
            rect: *source.last().expect("part rectangle"),
        });
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
    let (image, source) = if single_image {
        (image.expect("single atlas image"), source)
    } else {
        packed.resolve_with_sources(&requested, images, limit, sources)?
    };
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
    fn hidden_actor_cache_follows_execution_not_time_and_preserves_story_identity() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<crate::dependencies::ScriptDependencies>()
            .init_resource::<StageState>()
            .init_resource::<PendingCharacterShows>()
            .add_systems(Update, retire_hidden_groups);
        let mut roots = Vec::new();
        for name in ["alice", "bob"] {
            let child = app.world_mut().spawn_empty().id();
            let root = app
                .world_mut()
                .spawn((
                    CharacterRoot {
                        actor_id: name.into(),
                    },
                    CharacterGroup::default(),
                ))
                .add_child(child)
                .id();
            let mut stage = app.world_mut().resource_mut::<StageState>();
            stage.character_roots.insert(name.into(), root);
            stage
                .sprites
                .insert(format!("character::{name}::body"), child);
            stage
                .character_catalog_names
                .insert(name.into(), name.into());
            stage.character_order.push(name.into());
            roots.push((root, child));
        }
        app.world_mut()
            .resource_mut::<StageState>()
            .character_active_parts
            .insert("alice".into(), HashSet::from(["body".into()]));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs(2));
        app.update();
        assert!(
            app.world().get_entity(roots[1].0).is_ok(),
            "short hide/show cuts reuse the cache"
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs(600));
        app.update();
        assert!(
            app.world().get_entity(roots[1].0).is_ok(),
            "idle time cannot evict artwork"
        );
        app.world_mut()
            .resource_mut::<crate::dependencies::ScriptDependencies>()
            .revision = 5;
        app.update();
        assert!(app.world().get_entity(roots[0].0).is_ok());
        assert!(app.world().get_entity(roots[1].0).is_err());
        assert!(
            app.world().get_entity(roots[1].1).is_err(),
            "all child asset owners retire with the root"
        );
        let stage = app.world().resource::<StageState>();
        assert!(!stage.character_roots.contains_key("bob"));
        assert!(!stage.sprites.contains_key("character::bob::body"));
        assert_eq!(stage.character_catalog_names["bob"], "bob");
        assert_eq!(stage.character_order, ["alice"]);
    }

    #[test]
    fn composition_waits_for_runtime_initialization() {
        let mut app = App::new();
        install_runtime_systems(&mut app);
        // No stage resources exist while asynchronous content loading proceeds.
        app.update();
        app.update();
        app.init_resource::<Time>()
            .init_resource::<StageState>()
            .init_resource::<PendingCharacterShows>()
            .init_resource::<super::super::SceneSharedState>()
            .init_resource::<AnimationState>()
            .init_resource::<Assets<Image>>()
            .add_message::<AssetEvent<Image>>()
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
        LogicalCharacterPart(
            CharacterPartDefinition {
                id: id.into(),
                slot: None,
                path: "atlas.png".into(),
                pack_source: false,
                atlas_rect: None,
                offset: Vec2::ZERO,
                layer: 0.0,
                rect: Some(rect),
                mask: None,
                blend: CharacterBlendMode::Normal,
                color: [255; 4],
            },
            None,
        )
    }

    #[test]
    fn cpu_only_part_builds_without_any_render_source_even_for_one_visible_part() {
        let mut images = Assets::<Image>::default();
        let mut sources = Assets::<source::AtlasSource>::default();
        let mut pixels = Image::default();
        pixels.resize(bevy::render::render_resource::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        });
        let handle = sources.add(source::AtlasSource(pixels));
        let mut alice = part("alice/body", [0.0, 0.0, 16.0, 16.0]);
        alice.0.pack_source = true;
        alice.1 = Some(handle);
        let sprite = WorldSprite::from_color(Color::WHITE, Vec2::ZERO);
        let transform = Transform::default();
        let visible = Visibility::Visible;
        let parts = [(&alice, &sprite, &transform, &visible, false)];
        let mut cache = atlas::PackedAtlas::default();
        let first = build_layers(&parts, &mut images, &mut cache, 256, Some(&sources))
            .expect("CPU-only composition");
        assert_eq!(
            images.len(),
            1,
            "only the generated atlas is a render asset"
        );
        assert_eq!(first.3, Vec2::splat(16.0));
        let mut atlas = images.get_mut(&first.0).expect("atlas");
        assert_eq!(
            atlas.asset_usage,
            bevy::asset::RenderAssetUsages::RENDER_WORLD
        );
        atlas.data = None;
        drop(atlas);
        let second = build_layers(&parts, &mut images, &mut cache, 256, Some(&sources))
            .expect("reuse after GPU extraction");
        assert_eq!(first.0, second.0);
        assert_eq!(images.len(), 1);
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
            &mut images,
            &mut atlas::PackedAtlas::default(),
            8192,
            None,
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
    fn loose_parts_keep_geometry_masks_and_blend_when_packed() {
        let mut images = Assets::<Image>::default();
        let mut image = Image::default();
        image.resize(bevy::render::render_resource::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        });
        let first = images.add(image.clone());
        let second = images.add(image);
        let writer = part("alice/eyes", [0.0, 0.0, 16.0, 16.0]);
        let mut reader = part("alice/shadow", [0.0, 0.0, 8.0, 16.0]);
        reader.0.mask = Some(crate::character::CharacterMaskDefinition {
            kind: CharacterMaskKind::Read,
            reference: 1,
        });
        reader.0.blend = CharacterBlendMode::Multiply;
        let a = WorldSprite::from_image(first.clone());
        let mut b = WorldSprite::from_image(second.clone());
        b.color = Color::linear_rgba(1.0, 1.0, 1.0, 0.4);
        let pose = Transform::default();
        let visible = Visibility::Visible;
        let parts = [
            (&writer, &a, &pose, &visible, false),
            (&reader, &b, &pose, &visible, false),
        ];
        let mut cache = atlas::PackedAtlas::default();
        let (image, rects, layers, size, center, _) =
            build_layers(&parts, &mut images, &mut cache, 256, None)
                .expect("loose character parts compose");
        assert_ne!(image, first);
        assert_ne!(image, second);
        assert_eq!(rects[1].size(), UVec2::new(8, 16));
        assert_eq!(layers[1].bounds, Rect::new(0.25, 0.0, 0.75, 1.0));
        assert_eq!(layers[1].color.alpha(), 0.4);
        assert_eq!(layers[1].mask, MaskMode::Read(1));
        assert_eq!(layers[1].blend, BlendMode::Multiply);
        assert_eq!((size, center), (Vec2::splat(16.0), Vec2::ZERO));
        let count = images.len();
        assert_eq!(
            build_layers(&parts, &mut images, &mut cache, 256, None)
                .expect("cached composition")
                .0,
            image
        );
        assert_eq!(
            images.len(),
            count,
            "unchanged frames must not allocate another image"
        );
    }

    #[test]
    fn group_fade_preserves_intrinsic_part_alpha_and_hides_children() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<super::super::SceneSharedState>()
            .init_resource::<AnimationState>()
            .init_resource::<Assets<Image>>()
            .add_message::<AssetEvent<Image>>()
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
            .entity_mut(root)
            .insert(super::super::CharacterRoot {
                actor_id: "bob".into(),
            });
        let region = super::super::clipping::ClipRegion {
            center: [10.0, 20.0],
            size: [40.0, 80.0],
            rotation: 30.0,
        };
        let expected_clip = region.rect().expect("valid rectangle");
        {
            use super::super::clipping::ClipCommand;
            let mut shared = app
                .world_mut()
                .resource_mut::<super::super::SceneSharedState>();
            shared.0.actor_depths.insert("bob".into(), 14.0);
            shared
                .0
                .clips
                .apply(ClipCommand::Define {
                    name: "window".into(),
                    region,
                })
                .expect("define clip");
            shared
                .0
                .clips
                .apply(ClipCommand::Actor {
                    id: "bob".into(),
                    region: Some("window".into()),
                })
                .expect("attach clip");
        }
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
        assert_eq!(composed.clip, Some(expected_clip));
        assert_eq!(
            app.world()
                .get::<Transform>(display)
                .expect("display transform")
                .translation
                .z,
            14.0
        );
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
